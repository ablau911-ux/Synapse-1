#![allow(clippy::significant_drop_tightening)]

use std::{
    sync::{
        Arc, Condvar, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use synapse_capture::{
    CaptureConfig, CaptureHandle, CaptureTarget, CapturedFrame, spawn_capture_loop,
};

use crate::{
    camera::{
        ScreenState,
        monitor::{ResolvedMonitor, is_session_locked},
    },
    config::{CameraConfig, MS_PER_SECOND, ONE_U32, ONE_U64, POLL_INTERVAL_MS, ZERO_U64},
    error::{LivefeedError, Result},
    history::{ChangeRecord, History, RawBgraFrame, current_unix_ms, extract_tight_bgra},
    tiles::{self, full_frame_rect, is_black_bgra},
};

#[derive(Default)]
pub(crate) struct CameraState {
    pub(crate) generation: u64,
    pub(crate) running: bool,
    pub(crate) monitor: u32,
    pub(crate) origin_x: i32,
    pub(crate) origin_y: i32,
    pub(crate) cfg: CameraConfig,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) latest_seq: u64,
    pub(crate) latest_ts_ms: u64,
    pub(crate) screen_state: ScreenState,
    pub(crate) frames_seen: u64,
    pub(crate) snapshots_stored: u64,
    pub(crate) history: History,
    pub(crate) latest_raw: Option<RawBgraFrame>,
    pub(crate) stop_flag: Option<Arc<AtomicBool>>,
    pub(crate) consumer_join: Option<JoinHandle<()>>,
    pub(crate) capture_handle: Option<CaptureHandle>,
}

#[derive(Default)]
pub(crate) struct CameraShared {
    pub(crate) lifecycle: Mutex<()>,
    pub(crate) state: Mutex<CameraState>,
    pub(crate) condvar: Condvar,
}

pub(crate) fn start_locked(
    shared: &Arc<CameraShared>,
    cfg: CameraConfig,
    monitor: Option<u32>,
    resolved: ResolvedMonitor,
) -> Result<()> {
    let capture_handle = spawn_capture_loop(build_capture_config(&cfg, monitor))?;
    let stop_flag = Arc::new(AtomicBool::new(false));
    let generation = install_initial_state(shared, &cfg, resolved, &stop_flag);
    let consumer_join = match spawn_consumer(
        Arc::clone(shared),
        &capture_handle,
        stop_flag,
        cfg.clone(),
        generation,
    ) {
        Ok(join) => join,
        Err(err) => {
            lock_state(shared).capture_handle = Some(capture_handle);
            let _ = stop_locked(shared);
            return Err(err);
        }
    };
    {
        let mut state = lock_state(shared);
        state.consumer_join = Some(consumer_join);
        state.capture_handle = Some(capture_handle);
    }
    wait_for_initial_frame(shared, generation, cfg.start_timeout_ms)
}

fn install_initial_state(
    shared: &CameraShared,
    cfg: &CameraConfig,
    resolved: ResolvedMonitor,
    stop_flag: &Arc<AtomicBool>,
) -> u64 {
    let initial_screen_state = if is_session_locked() {
        ScreenState::Locked
    } else {
        ScreenState::Live
    };
    let mut state = lock_state(shared);
    let next_gen = state.generation.saturating_add(ONE_U64);
    let prev_seq = state.latest_seq;
    *state = CameraState {
        generation: next_gen,
        latest_seq: prev_seq,
        screen_state: initial_screen_state,
        running: true,
        monitor: resolved.index,
        origin_x: resolved.origin_x,
        origin_y: resolved.origin_y,
        history: History::from_config(cfg),
        cfg: cfg.clone(),
        stop_flag: Some(Arc::clone(stop_flag)),
        ..CameraState::default()
    };
    next_gen
}

fn wait_for_initial_frame(shared: &CameraShared, generation: u64, timeout_ms: u64) -> Result<()> {
    let ready = {
        let state = lock_state(shared);
        let state = wait_while(shared, state, timeout_ms, |s| {
            s.running
                && s.generation == generation
                && s.screen_state != ScreenState::CaptureLost
                && s.screen_state != ScreenState::Locked
                && s.frames_seen == ZERO_U64
        });
        state.running
            && state.generation == generation
            && state.screen_state != ScreenState::CaptureLost
            && (state.frames_seen > ZERO_U64 || state.screen_state == ScreenState::Locked)
    };
    if !ready {
        stop_locked(shared)?;
        return Err(LivefeedError::capture_failed(
            "capture failed to produce an initial frame within start_timeout_ms",
        ));
    }
    Ok(())
}

pub(crate) fn lock_state(shared: &CameraShared) -> MutexGuard<'_, CameraState> {
    shared.state.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(crate) fn wait_while<'a, F>(
    shared: &'a CameraShared,
    mut state: MutexGuard<'a, CameraState>,
    wait_ms: u64,
    pred: F,
) -> MutexGuard<'a, CameraState>
where
    F: Fn(&CameraState) -> bool,
{
    let deadline = Instant::now() + Duration::from_millis(wait_ms);
    while pred(&state) {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        let (next, _) = shared
            .condvar
            .wait_timeout(state, deadline.saturating_duration_since(now))
            .unwrap_or_else(PoisonError::into_inner);
        state = next;
    }
    state
}

pub(crate) fn build_capture_config(cfg: &CameraConfig, monitor: Option<u32>) -> CaptureConfig {
    let target = monitor.map_or(CaptureTarget::Primary, |monitor_index| {
        CaptureTarget::Monitor { monitor_index }
    });
    let interval_ms = (MS_PER_SECOND / u64::from(cfg.fps.max(ONE_U32))).max(ONE_U64);
    CaptureConfig {
        target,
        min_update_interval_ms: interval_ms,
        cursor_visible: true,
        secondary_windows: true,
        dirty_region_only: false,
        ..CaptureConfig::default().with_env_backend()
    }
}

pub(crate) fn spawn_consumer(
    shared: Arc<CameraShared>,
    capture_handle: &CaptureHandle,
    stop_flag: Arc<AtomicBool>,
    cfg: CameraConfig,
    generation: u64,
) -> Result<JoinHandle<()>> {
    let rx = capture_handle.receiver();
    let poll = Duration::from_millis(POLL_INTERVAL_MS);
    thread::Builder::new()
        .name("synapse-livefeed".to_owned())
        .spawn(move || {
            while !stop_flag.load(Ordering::Relaxed) {
                match rx.recv_timeout(poll) {
                    Ok(frame) => process_frame(&shared, &frame, &cfg, generation),
                    Err(err) if err.is_disconnected() => {
                        if !stop_flag.load(Ordering::Relaxed) {
                            mark_capture_lost(&shared, generation);
                        }
                        break;
                    }
                    Err(_) => on_poll_timeout(&shared, generation),
                }
            }
        })
        .map_err(|_e| LivefeedError::capture_failed("failed to spawn livefeed thread"))
}

pub(crate) fn stop_locked(shared: &CameraShared) -> Result<()> {
    let (stop_flag, consumer_join, capture_handle) = {
        let mut state = lock_state(shared);
        state.running = false;
        state.latest_raw = None;
        shared.condvar.notify_all();
        (
            state.stop_flag.take(),
            state.consumer_join.take(),
            state.capture_handle.take(),
        )
    };
    join_workers(stop_flag, consumer_join, capture_handle)
}

pub(crate) fn join_workers(
    stop_flag: Option<Arc<AtomicBool>>,
    consumer_join: Option<JoinHandle<()>>,
    capture_handle: Option<CaptureHandle>,
) -> Result<()> {
    if let Some(flag) = stop_flag {
        flag.store(true, Ordering::Relaxed);
    }
    let consumer_res = match consumer_join {
        Some(join) => join
            .join()
            .map_err(|_e| LivefeedError::capture_failed("livefeed consumer panicked")),
        None => Ok(()),
    };
    let capture_res = match capture_handle {
        Some(handle) => handle.stop().map_err(LivefeedError::from),
        None => Ok(()),
    };
    consumer_res.and(capture_res)
}

fn on_poll_timeout(shared: &CameraShared, generation: u64) {
    let locked = is_session_locked();
    let now_ms = current_unix_ms();
    let mut state = lock_state(shared);
    if !state.running || state.generation != generation {
        return;
    }
    state.history.evict(now_ms);
    if apply_lock_state(&mut state, locked) {
        shared.condvar.notify_all();
    }
}

/// Lock transitions; returns whether the screen state changed (waiters must be woken).
/// Locking keeps the current frame; unlocking a static desktop (WGC sends no new frame)
/// makes that kept frame current again.
pub(crate) fn apply_lock_state(state: &mut CameraState, locked: bool) -> bool {
    if locked {
        if state.screen_state == ScreenState::Locked {
            return false;
        }
        state.screen_state = ScreenState::Locked;
        return true;
    }
    if state.screen_state == ScreenState::Locked && state.latest_raw.is_some() {
        state.screen_state = ScreenState::Live;
        return true;
    }
    false
}

fn mark_capture_lost(shared: &CameraShared, generation: u64) {
    let now_ms = current_unix_ms();
    let mut state = lock_state(shared);
    if !state.running || state.generation != generation {
        return;
    }
    state.history.evict(now_ms);
    state.screen_state = ScreenState::CaptureLost;
    state.latest_raw = None;
    shared.condvar.notify_all();
}

fn process_frame(
    shared: &CameraShared,
    frame: &CapturedFrame,
    cfg: &CameraConfig,
    generation: u64,
) {
    let Some((bgra, row_stride)) = extract_tight_bgra(frame) else {
        return;
    };
    let now_ms = current_unix_ms();
    if is_session_locked() {
        record_non_live_frame(
            shared,
            generation,
            now_ms,
            frame.width,
            frame.height,
            ScreenState::Locked,
        );
        return;
    }
    if is_black_bgra(&bgra) {
        record_non_live_frame(
            shared,
            generation,
            now_ms,
            frame.width,
            frame.height,
            ScreenState::Black,
        );
        return;
    }
    let raw = RawBgraFrame {
        width: frame.width,
        height: frame.height,
        row_stride,
        bgra,
    };
    store_live_frame(shared, generation, now_ms, cfg, raw);
}

fn record_non_live_frame(
    shared: &CameraShared,
    generation: u64,
    now_ms: u64,
    width: u32,
    height: u32,
    state_kind: ScreenState,
) {
    let mut state = lock_state(shared);
    if !state.running || state.generation != generation {
        return;
    }
    state.history.evict(now_ms);
    state.frames_seen = state.frames_seen.saturating_add(ONE_U64);
    state.width = width;
    state.height = height;
    state.screen_state = state_kind;
    if state_kind != ScreenState::Locked {
        state.latest_raw = None;
    }
    shared.condvar.notify_all();
}

fn store_live_frame(
    shared: &CameraShared,
    generation: u64,
    now_ms: u64,
    cfg: &CameraConfig,
    raw: RawBgraFrame,
) {
    let mut state = lock_state(shared);
    if !state.running || state.generation != generation {
        return;
    }
    state.history.evict(now_ms);
    let rects = match state.latest_raw.as_ref() {
        None => vec![full_frame_rect(raw.width, raw.height)],
        Some(prev) if prev.width != raw.width || prev.height != raw.height => {
            vec![full_frame_rect(raw.width, raw.height)]
        }
        Some(prev) => tiles::diff(
            &prev.bgra,
            &raw.bgra,
            raw.width,
            raw.height,
            raw.row_stride,
            cfg.tile_px,
        ),
    };
    state.frames_seen = state.frames_seen.saturating_add(ONE_U64);
    state.width = raw.width;
    state.height = raw.height;
    state.screen_state = ScreenState::Live;
    if !rects.is_empty() {
        let seq = state.latest_seq.saturating_add(ONE_U64);
        state.latest_seq = seq;
        state.latest_ts_ms = now_ms;
        state.snapshots_stored = state.snapshots_stored.saturating_add(ONE_U64);
        state
            .history
            .push(ChangeRecord::new(seq, now_ms, raw.width, raw.height, rects));
        state.latest_raw = Some(raw);
    }
    shared.condvar.notify_all();
}
