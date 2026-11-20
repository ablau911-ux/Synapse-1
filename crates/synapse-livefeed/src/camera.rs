#![allow(clippy::significant_drop_tightening)]

mod changes;
mod lifecycle;
mod monitor;
mod wait;
mod watch;

pub use watch::{Watch, WatchFrame};

use std::{
    fmt,
    sync::{Arc, MutexGuard, PoisonError},
};

use synapse_core::Rect;

use crate::{
    camera::{
        changes::{build_changes_result, collect_merged_rects},
        lifecycle::{CameraShared, CameraState, lock_state, start_locked, stop_locked},
        monitor::resolve_monitor,
    },
    config::{CameraConfig, ONE_F32, ZERO_U64},
    error::{LivefeedError, Result},
    history::{current_unix_ms, encode_crop_jpeg, encode_look_jpeg, scaled_dimensions},
    tiles::translate_to_frame_rect,
};

#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub enum ScreenState {
    #[default]
    Live,
    Locked,
    Black,
    CaptureLost,
    Stopped,
}

impl ScreenState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Locked => "locked",
            Self::Black => "black",
            Self::CaptureLost => "capture_lost",
            Self::Stopped => "stopped",
        }
    }
}

impl fmt::Display for ScreenState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CameraStatus {
    pub running: bool,
    pub monitor: u32,
    pub origin_x: i32,
    pub origin_y: i32,
    pub width: u32,
    pub height: u32,
    pub latest_seq: u64,
    pub latest_ts_ms: u64,
    pub screen_state: ScreenState,
    pub frames_seen: u64,
    pub snapshots_stored: u64,
    pub history_bytes: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Look {
    pub seq: u64,
    pub ts_ms: u64,
    pub image_jpeg: Vec<u8>,
    pub scale: f32,
    pub width: u32,
    pub height: u32,
    pub screen_state: ScreenState,
    pub monitor: u32,
    pub origin_x: i32,
    pub origin_y: i32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChangeRegion {
    pub rect: Rect,
    pub crop_jpeg: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Changes {
    pub since: u64,
    pub seq: u64,
    pub ts_ms: u64,
    pub regions: Vec<ChangeRegion>,
    pub too_many: bool,
    pub since_evicted: bool,
    /// The changed area covers at least `cut_ratio_percent` of the frame: re-key with `look`.
    pub cut: bool,
    /// Only with `settle_ms`: the screen stayed quiet for the whole settle window.
    pub settled: bool,
    pub screen_state: ScreenState,
    pub monitor: u32,
    pub origin_x: i32,
    pub origin_y: i32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Zoom {
    pub image_jpeg: Vec<u8>,
    pub rect: Rect,
    pub seq: u64,
    pub screen_state: ScreenState,
    pub monitor: u32,
    pub origin_x: i32,
    pub origin_y: i32,
}

#[derive(Default)]
pub struct Camera {
    shared: Arc<CameraShared>,
}

impl fmt::Debug for Camera {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Camera")
            .field("status", &self.status())
            .finish()
    }
}

impl Drop for Camera {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

impl Camera {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts background capture on the primary monitor (or `monitor` index).
    ///
    /// # Errors
    ///
    /// Returns [`LivefeedError::BadConfig`], [`LivefeedError::AlreadyRunning`], or [`LivefeedError::CaptureFailed`].
    pub fn start(&self, cfg: CameraConfig, monitor: Option<u32>) -> Result<()> {
        cfg.validate()?;
        let _lifecycle = self.lock_lifecycle();
        if self.lock_state().running {
            return Err(LivefeedError::AlreadyRunning);
        }
        let resolved = resolve_monitor(monitor)?;
        start_locked(&self.shared, cfg, monitor, resolved)
    }

    /// Stops background capture and joins the consumer and capture threads.
    ///
    /// # Errors
    ///
    /// Returns [`LivefeedError::CaptureFailed`] if a worker thread reports a terminal error.
    pub fn stop(&self) -> Result<()> {
        let _lifecycle = self.lock_lifecycle();
        stop_locked(&self.shared)
    }

    #[must_use]
    pub fn status(&self) -> CameraStatus {
        let mut s = self.lock_state();
        s.history.evict(current_unix_ms());
        CameraStatus {
            running: s.running,
            monitor: s.monitor,
            origin_x: s.origin_x,
            origin_y: s.origin_y,
            width: s.width,
            height: s.height,
            latest_seq: s.latest_seq,
            latest_ts_ms: s.latest_ts_ms,
            screen_state: if s.running {
                s.screen_state
            } else {
                ScreenState::Stopped
            },
            frames_seen: s.frames_seen,
            snapshots_stored: s.snapshots_stored,
            history_bytes: s.history.bytes(),
        }
    }

    /// Returns the current screen as a single downscaled JPEG image plus sequence cursor.
    ///
    /// # Errors
    ///
    /// Returns [`LivefeedError::NotRunning`] or [`LivefeedError::CaptureFailed`].
    pub fn look(&self) -> Result<Look> {
        let (raw, meta, look_long_edge, quality) = {
            let mut s = self.lock_state();
            if !s.running {
                return Err(LivefeedError::NotRunning);
            }
            s.history.evict(current_unix_ms());
            if s.screen_state == ScreenState::CaptureLost {
                return Err(LivefeedError::capture_failed("capture lost"));
            }
            if s.screen_state != ScreenState::Live {
                return Ok(non_live_look(&s));
            }
            let raw = s
                .latest_raw
                .clone()
                .ok_or_else(|| LivefeedError::capture_failed("no current frame"))?;
            (
                raw,
                FrameMeta::from_state(&s),
                s.cfg.look_long_edge,
                s.cfg.jpeg_quality,
            )
        };
        let image_jpeg =
            encode_look_jpeg(&raw.bgra, raw.width, raw.height, look_long_edge, quality)?;
        let (_, _, scale) = scaled_dimensions(raw.width, raw.height, look_long_edge);
        Ok(Look {
            seq: meta.seq,
            ts_ms: meta.ts_ms,
            image_jpeg,
            scale,
            width: raw.width,
            height: raw.height,
            screen_state: ScreenState::Live,
            monitor: meta.monitor,
            origin_x: meta.origin_x,
            origin_y: meta.origin_y,
        })
    }

    /// Returns a span of time as ONE filmstrip grid image (change-gated samples at a fixed cadence).
    ///
    /// # Errors
    ///
    /// Returns [`LivefeedError::NotRunning`] or [`LivefeedError::CaptureFailed`].
    pub fn watch(&self, ms: Option<u64>, max_frames: Option<usize>) -> Result<Watch> {
        watch::watch(self, ms, max_frames, None)
    }

    /// Like [`Camera::watch`] but films only `region` (virtual-screen px), at up to native
    /// resolution, so small details (a game HUD, a progress bar) stay readable.
    ///
    /// # Errors
    ///
    /// Returns [`LivefeedError::NotRunning`], [`LivefeedError::BadRegion`] or
    /// [`LivefeedError::CaptureFailed`].
    pub fn watch_region(
        &self,
        ms: Option<u64>,
        max_frames: Option<usize>,
        region: Option<Rect>,
    ) -> Result<Watch> {
        watch::watch(self, ms, max_frames, region)
    }

    /// Returns regions that changed since `since`, optionally waiting up to `wait_ms`.
    ///
    /// # Errors
    ///
    /// Returns [`LivefeedError::NotRunning`] or [`LivefeedError::CaptureFailed`].
    pub fn changes(&self, since: u64, wait_ms: Option<u64>) -> Result<Changes> {
        self.changes_with(since, wait_ms, None)
    }

    /// Like [`Camera::changes`]; with `settle_ms` it returns once the screen has been quiet
    /// for that long (or `wait_ms` ran out) and reports `settled`.
    ///
    /// # Errors
    ///
    /// Returns [`LivefeedError::NotRunning`] or [`LivefeedError::CaptureFailed`].
    pub fn changes_with(
        &self,
        since: u64,
        wait_ms: Option<u64>,
        settle_ms: Option<u64>,
    ) -> Result<Changes> {
        let (since_dims, records, curr, cfg, meta, settled) = {
            let mut s = self.lock_state();
            if !s.running {
                return Err(LivefeedError::NotRunning);
            }
            let (next, settled) =
                wait::wait_for_changes(&self.shared, s, since, wait_ms, settle_ms)?;
            s = next;
            s.history.evict(current_unix_ms());
            if s.screen_state == ScreenState::CaptureLost {
                return Err(LivefeedError::capture_failed("capture lost"));
            }
            if s.screen_state != ScreenState::Live {
                return Ok(empty_changes(since, &s, false, settled));
            }
            if since > s.latest_seq || s.latest_seq == ZERO_U64 {
                return Ok(empty_changes(since, &s, true, settled));
            }
            if since == s.latest_seq {
                return Ok(empty_changes(since, &s, false, settled));
            }
            let Some(records) = s.history.records_after(since) else {
                return Ok(empty_changes(since, &s, true, settled));
            };
            let Some(curr) = s.latest_raw.clone() else {
                return Err(LivefeedError::capture_failed("latest raw frame missing"));
            };
            let since_dims = s.history.record_at(since).map(|r| (r.width, r.height));
            (
                since_dims,
                records.to_vec(),
                curr,
                s.cfg.clone(),
                FrameMeta::from_state(&s),
                settled,
            )
        };
        let merged =
            collect_merged_rects(since_dims, &records, curr.width, curr.height, cfg.tile_px);
        build_changes_result(since, meta, merged, &curr, &cfg, settled)
    }

    /// Returns a full-resolution JPEG crop of virtual-screen `rect` from the latest frame.
    ///
    /// # Errors
    ///
    /// Returns [`LivefeedError::NotRunning`], [`LivefeedError::BadRegion`], or [`LivefeedError::CaptureFailed`].
    pub fn zoom(&self, rect: Rect) -> Result<Zoom> {
        let (raw, frame_rect, meta, quality) = {
            let mut s = self.lock_state();
            if !s.running {
                return Err(LivefeedError::NotRunning);
            }
            s.history.evict(current_unix_ms());
            if s.screen_state == ScreenState::CaptureLost {
                return Err(LivefeedError::capture_failed("capture lost"));
            }
            if s.screen_state != ScreenState::Live {
                return Ok(non_live_zoom(rect, &s));
            }
            let latest = s
                .latest_raw
                .clone()
                .ok_or_else(|| LivefeedError::capture_failed("no frame"))?;
            let frame_rect = translate_to_frame_rect(rect, s.origin_x, s.origin_y)?;
            (
                latest,
                frame_rect,
                FrameMeta::from_state(&s),
                s.cfg.jpeg_quality,
            )
        };
        let image_jpeg = encode_crop_jpeg(&raw, frame_rect, quality)?;
        Ok(Zoom {
            image_jpeg,
            rect,
            seq: meta.seq,
            screen_state: ScreenState::Live,
            monitor: meta.monitor,
            origin_x: meta.origin_x,
            origin_y: meta.origin_y,
        })
    }

    fn lock_lifecycle(&self) -> MutexGuard<'_, ()> {
        self.shared
            .lifecycle
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn lock_state(&self) -> MutexGuard<'_, CameraState> {
        lock_state(&self.shared)
    }
}

#[derive(Copy, Clone)]
struct FrameMeta {
    seq: u64,
    ts_ms: u64,
    monitor: u32,
    origin_x: i32,
    origin_y: i32,
}

impl FrameMeta {
    const fn from_state(state: &CameraState) -> Self {
        Self {
            seq: state.latest_seq,
            ts_ms: state.latest_ts_ms,
            monitor: state.monitor,
            origin_x: state.origin_x,
            origin_y: state.origin_y,
        }
    }
}

const fn non_live_look(state: &CameraState) -> Look {
    Look {
        seq: state.latest_seq,
        ts_ms: state.latest_ts_ms,
        image_jpeg: Vec::new(),
        scale: ONE_F32,
        width: state.width,
        height: state.height,
        screen_state: state.screen_state,
        monitor: state.monitor,
        origin_x: state.origin_x,
        origin_y: state.origin_y,
    }
}

const fn non_live_zoom(rect: Rect, state: &CameraState) -> Zoom {
    Zoom {
        image_jpeg: Vec::new(),
        rect,
        seq: state.latest_seq,
        screen_state: state.screen_state,
        monitor: state.monitor,
        origin_x: state.origin_x,
        origin_y: state.origin_y,
    }
}

const fn empty_changes(
    since: u64,
    state: &CameraState,
    since_evicted: bool,
    settled: bool,
) -> Changes {
    Changes {
        since,
        seq: state.latest_seq,
        ts_ms: state.latest_ts_ms,
        regions: Vec::new(),
        too_many: false,
        since_evicted,
        cut: false,
        settled,
        screen_state: state.screen_state,
        monitor: state.monitor,
        origin_x: state.origin_x,
        origin_y: state.origin_y,
    }
}

#[cfg(test)]
#[path = "camera_addendum_tests.rs"]
mod camera_addendum_tests;

#[cfg(test)]
#[path = "camera_watch_settle_tests.rs"]
mod camera_watch_settle_tests;

#[cfg(test)]
#[path = "camera_lock_tests.rs"]
mod camera_lock_tests;
