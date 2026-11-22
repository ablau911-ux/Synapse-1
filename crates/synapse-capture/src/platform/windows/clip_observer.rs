//! Clip observer: the frame handler and capture handle of the clip recorder
//! (docs/clip-recorder-plan.md, version 4.1, section O, operations O1 to O4).
//!
//! The existing screenshot paths (`capture.rs`) are not used. This observer has
//! its own Windows Graphics Capture session and its own handler:
//!
//! - O1: it refuses Windows builds below `MIN_WINDOWS_BUILD` and asks for each
//!   session setting (no cursor, no border, secondary windows, the minimum update
//!   interval, dirty regions) only where windows-capture reports it supported;
//!   every other setting stays Default.
//! - O2: the handler reads the composition time of each frame, computes the
//!   instant k it belongs to (`s_k` = `s_0` + k / f, `s_0` = the first delivered frame)
//!   and copies the frame on the graphics chip into the candidate slot of that
//!   instant. A frame for a later instant pins the candidate. The handler only
//!   issues GPU copies; it never maps.
//! - O3: it measures the longest hold time (arrival to release of the frame) and
//!   names every instant lost because no slot was free.
//! - O4: the reader first gets the identity (composition time) of the frame for
//!   an instant; it reads the pixels back only when it asks for them.
//!
//! Threading rule (C3, measured in package 0, M6): multithread protection is on
//! for the capture device; the reader maps with "do not wait" and sleeps
//! `MAP_RETRY_SLEEP_MS` between attempts, so it never holds the device context
//! across a wait.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use serde::Serialize;
use synapse_core::Rect;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_MAP_FLAG_DO_NOT_WAIT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING,
    ID3D11Device, ID3D11DeviceContext, ID3D11Multithread, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::DXGI_ERROR_WAS_STILL_DRAWING;
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
use windows::core::{Interface as _, w};
use windows_capture::capture::{CaptureControl, Context, GraphicsCaptureApiHandler};
use windows_capture::frame::Frame;
use windows_capture::graphics_capture_api::{GraphicsCaptureApi, InternalCaptureControl};
use windows_capture::monitor::Monitor;
use windows_capture::settings::{
    ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
    GraphicsCaptureItemType, MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
};
use windows_capture::window::Window;

use super::capture::union_dirty_regions;
use super::common::hwnd_from_i64;
use super::target::validate_hwnd;
use crate::CaptureError;

/// Windows 10 version 1903 (E5): windows-capture 2.0.0 needs it.
pub const MIN_WINDOWS_BUILD: u32 = 18_362;
/// The minimum update interval asked of Windows where it is supported
/// (package 0, M3: 72 frames per second on the 144 Hz panel, longest gap 13.9 ms).
pub const REQUESTED_UPDATE_INTERVAL_MS: u64 = 11;
/// How many instants the reader may run behind the open instant (package 0, M7).
pub const MAX_SELECTION_LATENESS_INSTANTS: usize = 2;
/// GPU slots: the candidate of the open instant, one pinned slot per instant the
/// reader may be behind, the pinned frame of the last read instant, and a spare
/// (package 0, M7: lateness + 3).
pub const OBSERVE_SLOTS: usize = MAX_SELECTION_LATENESS_INSTANTS + 3;
/// The reader's sleep between two "do not wait" Map attempts (package 0, M6).
const MAP_RETRY_SLEEP_MS: u64 = 1;
/// 100 ns units per millisecond (composition times are in 100 ns units).
pub const HNS_PER_MS: i64 = 10_000;
const HNS_PER_SECOND: i64 = 10_000_000;

/// What the clip records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipSource {
    Window { hwnd: i64 },
    Monitor { monitor_index: u32 },
}

/// The session settings Windows granted for this clip (O1).
#[allow(
    clippy::struct_excessive_bools,
    reason = "one flag per Windows capture setting, reported as granted"
)]
#[derive(Clone, Copy, Debug, Serialize)]
pub struct ObserverSettings {
    /// `true` where Windows cannot leave the cursor out (no cursor setting
    /// before Windows 10 2004); the viewer then masks it (E9, D5).
    pub cursor_in_picture: bool,
    /// `false` where the border setting is missing: the yellow border shows.
    pub border_hidden: bool,
    pub secondary_windows: bool,
    /// The requested minimum update interval; `None` where unsupported, when
    /// frames arrive per composition.
    pub requested_interval_ms: Option<u64>,
    pub dirty_regions: bool,
}

impl ObserverSettings {
    /// Asks windows-capture which settings this Windows supports.
    fn supported() -> Self {
        let supported = |probe: fn() -> Result<
            bool,
            windows_capture::graphics_capture_api::Error,
        >| { probe().unwrap_or(false) };
        Self {
            cursor_in_picture: !supported(GraphicsCaptureApi::is_cursor_settings_supported),
            border_hidden: supported(GraphicsCaptureApi::is_border_settings_supported),
            secondary_windows: supported(GraphicsCaptureApi::is_secondary_windows_supported),
            requested_interval_ms: supported(
                GraphicsCaptureApi::is_minimum_update_interval_supported,
            )
            .then_some(REQUESTED_UPDATE_INTERVAL_MS),
            dirty_regions: supported(GraphicsCaptureApi::is_dirty_region_supported),
        }
    }

    fn session<T>(self, item: T, shared: Arc<Shared>) -> Settings<Arc<Shared>, T>
    where
        T: TryInto<GraphicsCaptureItemType>,
    {
        Settings::new(
            item,
            if self.cursor_in_picture {
                CursorCaptureSettings::Default
            } else {
                CursorCaptureSettings::WithoutCursor
            },
            if self.border_hidden {
                DrawBorderSettings::WithoutBorder
            } else {
                DrawBorderSettings::Default
            },
            if self.secondary_windows {
                SecondaryWindowSettings::Include
            } else {
                SecondaryWindowSettings::Default
            },
            self.requested_interval_ms
                .map_or(MinimumUpdateIntervalSettings::Default, |ms| {
                    MinimumUpdateIntervalSettings::Custom(Duration::from_millis(ms))
                }),
            if self.dirty_regions {
                DirtyRegionSettings::ReportOnly
            } else {
                DirtyRegionSettings::Default
            },
            ColorFormat::Bgra8,
            shared,
        )
    }
}

/// The identity of the frame chosen for an instant (O4): its composition time,
/// size and the region Windows reported changed, when it reports one.
#[derive(Clone, Copy, Debug)]
pub struct ObservedFrame {
    pub composed_hns: i64,
    pub width: u32,
    pub height: u32,
    pub dirty: Option<Rect>,
}

/// One frame read back to the processor: tightly packed BGRA rows.
pub struct ObservedPixels {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// What the observer can prove about a clip (O3, C1).
#[derive(Clone, Debug, Default, Serialize)]
pub struct ObserverStats {
    pub frames_delivered: u64,
    /// Longest time from a frame's arrival to its release, in 100 ns units.
    pub max_hold_hns: i64,
    /// Instants lost because no slot was free.
    pub slot_overflow_instants: Vec<i64>,
}

/// The QPC clock that composition times (`SystemRelativeTime`) use, in 100 ns units.
#[must_use]
pub fn now_hns() -> i64 {
    let (mut counter, mut frequency) = (0_i64, 0_i64);
    // SAFETY: two out-parameters; both calls cannot fail on Windows XP and later.
    unsafe {
        let _ = QueryPerformanceCounter(&raw mut counter);
        let _ = QueryPerformanceFrequency(&raw mut frequency);
    }
    let scaled = i128::from(counter) * i128::from(HNS_PER_SECOND) / i128::from(frequency.max(1));
    i64::try_from(scaled).unwrap_or(i64::MAX)
}

/// The Windows build number (`CurrentBuildNumber`), `None` when unreadable.
#[must_use]
pub fn windows_build() -> Option<u32> {
    let mut buffer = [0_u16; 32];
    let mut size = u32::try_from(std::mem::size_of_val(&buffer)).ok()?;
    // SAFETY: the buffer and its byte size are passed together.
    unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion"),
            w!("CurrentBuildNumber"),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&raw mut size),
        )
    }
    .ok()
    .ok()?;
    let end = buffer
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end]).trim().parse().ok()
}

/// A frame held in a slot for an instant.
#[derive(Clone, Copy)]
struct Held {
    slot: usize,
    instant: i64,
    frame: ObservedFrame,
}

/// A GPU texture of one slot, recreated when the frame size changes.
struct SlotTexture {
    texture: ID3D11Texture2D,
    width: u32,
    height: u32,
}

/// Which slot holds the frame for which instant (O2). The handler fills the
/// candidate; the reader pins and frees.
struct SlotTable {
    instant_hns: i64,
    origin: Option<i64>,
    textures: Vec<Option<SlotTexture>>,
    free: Vec<usize>,
    candidate: Option<Held>,
    pinned: VecDeque<Held>,
    overflow_instants: Vec<i64>,
}

impl SlotTable {
    fn new(instant_hns: i64) -> Self {
        Self {
            instant_hns,
            origin: None,
            textures: (0..OBSERVE_SLOTS).map(|_| None).collect(),
            free: (0..OBSERVE_SLOTS).rev().collect(),
            candidate: None,
            pinned: VecDeque::new(),
            overflow_instants: Vec::new(),
        }
    }

    /// The first instant k with `s_k` at or after `time`.
    fn instant_at_or_after(&self, time: i64) -> i64 {
        let since = time - self.origin.unwrap_or(time);
        since.div_euclid(self.instant_hns) + i64::from(since.rem_euclid(self.instant_hns) != 0)
    }

    /// Handler side: the texture the frame is copied into, or `None` when every
    /// slot is taken (the instant is lost and named).
    fn slot_for_frame(
        &mut self,
        device: &ID3D11Device,
        format: DXGI_FORMAT,
        frame: ObservedFrame,
    ) -> Result<Option<ID3D11Texture2D>, String> {
        self.origin.get_or_insert(frame.composed_hns);
        let instant = self.instant_at_or_after(frame.composed_hns);
        if let Some(candidate) = &mut self.candidate
            && candidate.instant == instant
        {
            let dirty = merge_dirty(candidate.frame.dirty, frame.dirty);
            candidate.frame = ObservedFrame { dirty, ..frame };
            let slot = candidate.slot;
            return self.texture_for(slot, device, format, frame).map(Some);
        }
        if let Some(older) = self.candidate.take() {
            self.pinned.push_back(older);
        }
        let Some(slot) = self.free.pop() else {
            self.overflow_instants.push(instant);
            return Ok(None);
        };
        self.candidate = Some(Held {
            slot,
            instant,
            frame,
        });
        self.texture_for(slot, device, format, frame).map(Some)
    }

    fn texture_for(
        &mut self,
        slot: usize,
        device: &ID3D11Device,
        format: DXGI_FORMAT,
        frame: ObservedFrame,
    ) -> Result<ID3D11Texture2D, String> {
        let fits = self.textures[slot]
            .as_ref()
            .is_some_and(|held| (held.width, held.height) == (frame.width, frame.height));
        if !fits {
            self.textures[slot] = Some(SlotTexture {
                texture: create_texture(device, format, frame.width, frame.height, false)?,
                width: frame.width,
                height: frame.height,
            });
        }
        self.textures[slot]
            .as_ref()
            .map(|held| held.texture.clone())
            .ok_or_else(|| "slot texture missing".to_owned())
    }

    /// Reader side: the last frame composed at or before `s_k`. Pins the
    /// candidate when it belongs to `s_k` or earlier, and frees the pinned slots
    /// before the chosen one.
    fn frame_for_instant(&mut self, k: i64) -> Option<ObservedFrame> {
        if self
            .candidate
            .as_ref()
            .is_some_and(|held| held.instant <= k)
            && let Some(candidate) = self.candidate.take()
        {
            self.pinned.push_back(candidate);
        }
        let position = self.pinned.iter().rposition(|held| held.instant <= k)?;
        for _ in 0..position {
            if let Some(older) = self.pinned.pop_front() {
                self.free.push(older.slot);
            }
        }
        self.pinned.front().map(|held| held.frame)
    }

    /// Reader side: the texture of the pinned frame with this identity.
    fn pinned_texture(&self, composed_hns: i64) -> Option<ID3D11Texture2D> {
        let held = self
            .pinned
            .iter()
            .find(|held| held.frame.composed_hns == composed_hns)?;
        self.textures[held.slot]
            .as_ref()
            .map(|slot| slot.texture.clone())
    }
}

fn merge_dirty(first: Option<Rect>, second: Option<Rect>) -> Option<Rect> {
    match (first, second) {
        (Some(a), Some(b)) => union_dirty_regions(&[
            windows_capture::frame::DirtyRegion {
                x: a.x,
                y: a.y,
                width: a.w,
                height: a.h,
            },
            windows_capture::frame::DirtyRegion {
                x: b.x,
                y: b.y,
                width: b.w,
                height: b.h,
            },
        ]),
        _ => None,
    }
}

/// The capture device and its immediate context. Direct3D 11 devices are
/// free-threaded; the context is shared because multithread protection is on.
struct Gpu {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
}
// SAFETY: see the type comment.
unsafe impl Send for Gpu {}
// SAFETY: see the type comment.
unsafe impl Sync for Gpu {}

/// What the handler and the reader share.
pub struct Shared {
    table: Mutex<SlotTable>,
    gpu: OnceLock<Gpu>,
    dirty_regions: bool,
    closed: AtomicBool,
    frames_delivered: AtomicU64,
    max_hold_hns: AtomicI64,
}

impl Shared {
    fn table(&self) -> MutexGuard<'_, SlotTable> {
        self.table.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The clip's frame handler: one GPU copy per delivered frame, never a Map.
struct ClipFrameHandler {
    shared: Arc<Shared>,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
}

impl GraphicsCaptureApiHandler for ClipFrameHandler {
    type Flags = Arc<Shared>;
    type Error = String;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        let protection: ID3D11Multithread = ctx
            .device_context
            .cast()
            .map_err(|error| format!("ID3D11Multithread: {error}"))?;
        // SAFETY: switches on the device's own lock; returns the previous state.
        let _ = unsafe { protection.SetMultithreadProtected(true) };
        let _ = ctx.flags.gpu.set(Gpu {
            device: ctx.device.clone(),
            context: ctx.device_context.clone(),
        });
        Ok(Self {
            shared: ctx.flags,
            device: ctx.device,
            context: ctx.device_context,
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        _control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        let arrived = now_hns();
        let observed = ObservedFrame {
            composed_hns: frame.timestamp().map_or(arrived, |time| time.Duration),
            width: frame.width(),
            height: frame.height(),
            dirty: if self.shared.dirty_regions {
                union_dirty_regions(&frame.dirty_regions().unwrap_or_default())
            } else {
                None
            },
        };
        {
            // The table stays locked while the copy is issued, so the reader
            // never pins a slot whose copy is not yet queued.
            let mut table = self.shared.table();
            if let Some(slot) = table.slot_for_frame(&self.device, frame.desc().Format, observed)? {
                let region = D3D11_BOX {
                    left: 0,
                    top: 0,
                    front: 0,
                    right: observed.width,
                    bottom: observed.height,
                    back: 1,
                };
                // SAFETY: same format; the box lies inside both textures; the
                // device is multithread protected.
                unsafe {
                    self.context.CopySubresourceRegion(
                        &slot,
                        0,
                        0,
                        0,
                        0,
                        frame.as_raw_texture(),
                        0,
                        Some(&raw const region),
                    );
                }
            }
        }
        self.shared.frames_delivered.fetch_add(1, Ordering::Relaxed);
        self.shared
            .max_hold_hns
            .fetch_max(now_hns() - arrived, Ordering::Relaxed);
        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        self.shared.closed.store(true, Ordering::Relaxed);
        Ok(())
    }
}

/// The reader's staging texture, recreated when the frame size changes.
struct Staging {
    texture: ID3D11Texture2D,
    width: u32,
    height: u32,
}

/// The clip recorder's own capture handle (never the shared M1 controller).
pub struct ClipObserver {
    control: Option<CaptureControl<ClipFrameHandler, String>>,
    shared: Arc<Shared>,
    settings: ObserverSettings,
    staging: Option<Staging>,
}

impl ClipObserver {
    /// Starts observing `source`; instants are `instant_hns` apart (1 / f).
    ///
    /// # Errors
    ///
    /// `CAPTURE_GRAPHICS_API_UNSUPPORTED` below `MIN_WINDOWS_BUILD` or when the
    /// session cannot start; `CAPTURE_TARGET_INVALID` for a bad window or monitor.
    pub fn start(source: ClipSource, instant_hns: i64) -> Result<Self, CaptureError> {
        let build = windows_build().unwrap_or(0);
        if build < MIN_WINDOWS_BUILD {
            return Err(CaptureError::GraphicsApiUnsupported {
                detail: format!(
                    "clip recording needs Windows 10 version 1903 (build {MIN_WINDOWS_BUILD}) or later; this is build {build}"
                ),
            });
        }
        if !GraphicsCaptureApi::is_supported().unwrap_or(false) {
            return Err(CaptureError::GraphicsApiUnsupported {
                detail: "Windows Graphics Capture is not supported on this system".to_owned(),
            });
        }
        let settings = ObserverSettings::supported();
        let shared = Arc::new(Shared {
            table: Mutex::new(SlotTable::new(instant_hns.max(1))),
            gpu: OnceLock::new(),
            dirty_regions: settings.dirty_regions,
            closed: AtomicBool::new(false),
            frames_delivered: AtomicU64::new(0),
            max_hold_hns: AtomicI64::new(0),
        });
        let started = match source {
            ClipSource::Window { hwnd } => {
                validate_hwnd(hwnd)?;
                let window = Window::from_raw_hwnd(hwnd_from_i64(hwnd)?.0);
                ClipFrameHandler::start_free_threaded(settings.session(window, Arc::clone(&shared)))
            }
            ClipSource::Monitor { monitor_index } => {
                let index = usize::try_from(monitor_index.saturating_add(1)).unwrap_or(usize::MAX);
                let monitor =
                    Monitor::from_index(index).map_err(|error| CaptureError::TargetInvalid {
                        detail: error.to_string(),
                    })?;
                ClipFrameHandler::start_free_threaded(
                    settings.session(monitor, Arc::clone(&shared)),
                )
            }
        };
        let control = started.map_err(|error| CaptureError::GraphicsApiUnsupported {
            detail: format!("clip capture could not start: {error}"),
        })?;
        Ok(Self {
            control: Some(control),
            shared,
            settings,
            staging: None,
        })
    }

    #[must_use]
    pub const fn settings(&self) -> ObserverSettings {
        self.settings
    }

    /// `s_0`: the composition time of the first delivered frame.
    #[must_use]
    pub fn origin(&self) -> Option<i64> {
        self.shared.table().origin
    }

    /// `true` once Windows closed the capture item (the window closed) or the
    /// capture thread ended.
    #[must_use]
    pub fn closed(&self) -> bool {
        self.shared.closed.load(Ordering::Relaxed)
            || self
                .control
                .as_ref()
                .is_none_or(CaptureControl::is_finished)
    }

    /// O4, first step: the identity of the frame shown at instant k, without
    /// reading it back. `None` before the first frame.
    #[must_use]
    pub fn frame_for_instant(&self, k: i64) -> Option<ObservedFrame> {
        self.shared.table().frame_for_instant(k)
    }

    /// O4, second step: reads the pinned frame back to the processor, on the
    /// reader's thread. Map with "do not wait"; a busy GPU costs a 1 ms sleep.
    ///
    /// # Errors
    ///
    /// `CAPTURE_THREAD_FAILED` when the frame is no longer pinned or Map fails.
    pub fn read_back(&mut self, frame: &ObservedFrame) -> Result<ObservedPixels, CaptureError> {
        let source = self
            .shared
            .table()
            .pinned_texture(frame.composed_hns)
            .ok_or_else(|| thread_failed("the frame for this instant is no longer pinned"))?;
        let shared = Arc::clone(&self.shared);
        let gpu = shared
            .gpu
            .get()
            .ok_or_else(|| thread_failed("no capture device yet"))?;
        let staging = self.staging_for(gpu, &source, frame.width, frame.height)?;
        let region = D3D11_BOX {
            left: 0,
            top: 0,
            front: 0,
            right: frame.width,
            bottom: frame.height,
            back: 1,
        };
        // SAFETY: same format; the box lies inside both textures.
        unsafe {
            gpu.context.CopySubresourceRegion(
                &staging,
                0,
                0,
                0,
                0,
                &source,
                0,
                Some(&raw const region),
            );
        }
        let mapped = map_without_waiting(&gpu.context, &staging)?;
        let row_bytes = frame.width as usize * 4;
        let pitch = mapped.RowPitch as usize;
        let mut bgra = vec![0_u8; row_bytes * frame.height as usize];
        for (row, target) in bgra.chunks_exact_mut(row_bytes).enumerate() {
            // SAFETY: Map returned the texture's rows spaced RowPitch apart, each
            // at least row_bytes long, valid until Unmap.
            let source_row = unsafe {
                std::slice::from_raw_parts(mapped.pData.cast::<u8>().add(row * pitch), row_bytes)
            };
            target.copy_from_slice(source_row);
        }
        // SAFETY: mapped above.
        unsafe { gpu.context.Unmap(&staging, 0) };
        Ok(ObservedPixels {
            width: frame.width,
            height: frame.height,
            bgra,
        })
    }

    fn staging_for(
        &mut self,
        gpu: &Gpu,
        source: &ID3D11Texture2D,
        width: u32,
        height: u32,
    ) -> Result<ID3D11Texture2D, CaptureError> {
        if let Some(staging) = &self.staging
            && (staging.width, staging.height) == (width, height)
        {
            return Ok(staging.texture.clone());
        }
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: plain query into a description.
        unsafe { source.GetDesc(&raw mut desc) };
        let texture =
            create_texture(&gpu.device, desc.Format, width, height, true).map_err(thread_failed)?;
        self.staging = Some(Staging {
            texture: texture.clone(),
            width,
            height,
        });
        Ok(texture)
    }

    /// What the observer proved so far (O3).
    #[must_use]
    pub fn stats(&self) -> ObserverStats {
        ObserverStats {
            frames_delivered: self.shared.frames_delivered.load(Ordering::Relaxed),
            max_hold_hns: self.shared.max_hold_hns.load(Ordering::Relaxed),
            slot_overflow_instants: self.shared.table().overflow_instants.clone(),
        }
    }

    /// Stops the capture session and waits for its thread.
    #[must_use]
    pub fn stop(mut self) -> ObserverStats {
        if let Some(control) = self.control.take()
            && let Err(error) = control.stop()
        {
            tracing::debug!(code = "CLIP_OBSERVER_STOP_FAILED", error = ?error);
        }
        self.stats()
    }
}

impl Drop for ClipObserver {
    fn drop(&mut self) {
        if let Some(control) = self.control.take() {
            let _ = control.stop();
        }
    }
}

fn map_without_waiting(
    context: &ID3D11DeviceContext,
    staging: &ID3D11Texture2D,
) -> Result<D3D11_MAPPED_SUBRESOURCE, CaptureError> {
    loop {
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        let flags = D3D11_MAP_FLAG_DO_NOT_WAIT.0.cast_unsigned();
        // SAFETY: a staging texture created with processor read access.
        match unsafe { context.Map(staging, 0, D3D11_MAP_READ, flags, Some(&raw mut mapped)) } {
            Ok(()) => return Ok(mapped),
            Err(error) if error.code() == DXGI_ERROR_WAS_STILL_DRAWING => {
                std::thread::sleep(Duration::from_millis(MAP_RETRY_SLEEP_MS));
            }
            Err(error) => return Err(thread_failed(format!("Map (do not wait): {error}"))),
        }
    }
}

fn create_texture(
    device: &ID3D11Device,
    format: DXGI_FORMAT,
    width: u32,
    height: u32,
    staging: bool,
) -> Result<ID3D11Texture2D, String> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width.max(1),
        Height: height.max(1),
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: if staging {
            D3D11_USAGE_STAGING
        } else {
            D3D11_USAGE_DEFAULT
        },
        BindFlags: 0,
        CPUAccessFlags: if staging {
            D3D11_CPU_ACCESS_READ.0.cast_unsigned()
        } else {
            0
        },
        MiscFlags: 0,
    };
    let mut created = None;
    // SAFETY: a valid description and one out-parameter.
    unsafe { device.CreateTexture2D(&raw const desc, None, Some(&raw mut created)) }
        .map_err(|error| format!("CreateTexture2D: {error}"))?;
    created.ok_or_else(|| "CreateTexture2D returned no texture".to_owned())
}

fn thread_failed(detail: impl Into<String>) -> CaptureError {
    CaptureError::ThreadFailed {
        detail: detail.into(),
    }
}
