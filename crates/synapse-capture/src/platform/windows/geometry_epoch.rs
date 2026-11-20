//! Geometry epoch registry — the per-window move/resize generation counter
//! Windows does not expose.
//!
//! There is no Windows API that returns a "geometry epoch" for an HWND: no
//! generation number that ticks every time the window moves or resizes. This
//! module builds that primitive out of what Windows does expose: one
//! process-wide, out-of-context `SetWinEventHook` on
//! `EVENT_OBJECT_LOCATIONCHANGE`, pumped on one dedicated thread that lives
//! for the whole process, ticking a monotonic per-HWND generation counter on
//! every geometry change of every top-level window on the desktop.
//!
//! The counter ticks on every *change*, not every *net* change, so a window
//! that moves away and returns leaves its bounds equal but its epoch
//! advanced. Callers sample [`geometry_epoch`] around an operation and
//! require equality to prove the window was geometrically still throughout.
//!
//! Out-of-context `WinEvent`s are posted asynchronously to the installing
//! thread's queue, so a sample taken immediately after an operation can miss
//! an event still in flight. [`geometry_epoch_quiescent`] is the
//! delivery-safe sampler: it waits until the window's counter has been
//! unchanged for [`GEOMETRY_EPOCH_QUIET_MS`] before returning, bounded by
//! [`GEOMETRY_EPOCH_MAX_WAIT_MS`] so a window under continuous motion cannot
//! stall the caller (its epoch has changed by then anyway, and the caller
//! rejects on inequality).
//!
//! The same pump thread also hooks `EVENT_OBJECT_DESTROY`. Windows recycles
//! HWND values, so state keyed by the bare handle can leak from a destroyed
//! window to its successor. A destroy ticks a per-HWND *lifetime generation*
//! ([`window_generation`]) that callers fold into their identity keys, and
//! also ticks the geometry epoch so a capture in flight across a destroy and
//! re-create rejects its frame.
//!
//! If the hook cannot be installed, every query returns 0 and
//! [`geometry_epoch_status`] reports `active: false`; callers then see
//! "nothing ever changed", which is the pre-epoch bounds-equality behavior.
//! The failure is logged once with code
//! `CAPTURE_GEOMETRY_EPOCH_HOOK_UNAVAILABLE`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{
    HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent, WINEVENTPROC,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EVENT_OBJECT_DESTROY, EVENT_OBJECT_LOCATIONCHANGE, GetMessageW, MSG, OBJID_WINDOW,
    WINEVENT_OUTOFCONTEXT,
};

/// How long a window's generation must be observed unchanged before a
/// quiescent sample is trusted (covers out-of-context event delivery latency).
pub const GEOMETRY_EPOCH_QUIET_MS: u32 = 8;

/// Upper bound on how long [`geometry_epoch_quiescent`] will wait for a
/// window under continuous motion before returning its (changed) epoch.
pub const GEOMETRY_EPOCH_MAX_WAIT_MS: u32 = 150;

/// Health snapshot of the epoch registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeometryEpochStatus {
    /// Whether the `WinEvent` hook is installed and pumping.
    pub active: bool,
    /// How many distinct windows have ticked at least once.
    pub tracked_windows: usize,
    /// Total geometry changes counted since process start.
    pub total_changes: u64,
}

struct GeometryEpochRegistry {
    /// Per-HWND monotonic generation counters, keyed by the HWND handle
    /// value. One lock is sufficient: the callback holds it only for a
    /// `HashMap` tick, and desktop geometry-change rates are low.
    epochs: Mutex<HashMap<i64, u64>>,
    /// Per-HWND destroy counters (lifetime generation): ticks when the window
    /// owning that handle value is destroyed, so a recycled HWND value reads
    /// as a different window.
    generations: Mutex<HashMap<i64, u64>>,
    total_changes: AtomicU64,
    active: AtomicBool,
}

static REGISTRY: OnceLock<GeometryEpochRegistry> = OnceLock::new();
static PUMP_THREAD: OnceLock<()> = OnceLock::new();

impl GeometryEpochRegistry {
    fn new() -> Self {
        Self {
            epochs: Mutex::new(HashMap::new()),
            generations: Mutex::new(HashMap::new()),
            total_changes: AtomicU64::new(0),
            active: AtomicBool::new(false),
        }
    }

    fn epoch(&self, hwnd: i64) -> u64 {
        self.epochs
            .lock()
            .ok()
            .and_then(|epochs| epochs.get(&hwnd).copied())
            .unwrap_or(0)
    }

    fn generation(&self, hwnd: i64) -> u64 {
        self.generations
            .lock()
            .ok()
            .and_then(|generations| generations.get(&hwnd).copied())
            .unwrap_or(0)
    }

    fn tick(&self, hwnd: i64) {
        if let Ok(mut epochs) = self.epochs.lock() {
            let generation = epochs.entry(hwnd).or_insert(0);
            *generation += 1;
            self.total_changes.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn tick_destroyed(&self, hwnd: i64) {
        if let Ok(mut generations) = self.generations.lock() {
            let generation = generations.entry(hwnd).or_insert(0);
            *generation += 1;
        }
        // A capture in flight across a destroy + re-create must also reject.
        self.tick(hwnd);
    }

    fn epoch_quiescent(&self, hwnd: i64) -> u64 {
        let started = Instant::now();
        let quiet = Duration::from_millis(u64::from(GEOMETRY_EPOCH_QUIET_MS));
        let max_wait = Duration::from_millis(u64::from(GEOMETRY_EPOCH_MAX_WAIT_MS));
        loop {
            let before = self.epoch(hwnd);
            thread::sleep(quiet);
            let after = self.epoch(hwnd);
            if after == before || started.elapsed() >= max_wait {
                return after;
            }
        }
    }

    fn status(&self) -> GeometryEpochStatus {
        GeometryEpochStatus {
            active: self.active.load(Ordering::Acquire),
            tracked_windows: self.epochs.lock().map_or(0, |epochs| epochs.len()),
            total_changes: self.total_changes.load(Ordering::Acquire),
        }
    }
}

/// `WinEvent` callback for `EVENT_OBJECT_LOCATIONCHANGE` and
/// `EVENT_OBJECT_DESTROY`. Out-of-context events are delivered on the thread
/// that installed the hook (the pump thread), and the registry is a process
/// static, so no context parameter is needed.
unsafe extern "system" fn location_change_proc(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    _id_child: i32,
    _event_thread: u32,
    _event_time: u32,
) {
    // LOCATIONCHANGE also fires for carets, scrollbars, and child objects;
    // only the window object's own geometry defines the epoch.
    if id_object != OBJID_WINDOW.0 {
        return;
    }
    let handle = hwnd.0 as i64;
    if handle == 0 {
        return;
    }
    let Some(registry) = REGISTRY.get() else {
        return;
    };
    if event == EVENT_OBJECT_DESTROY {
        registry.tick_destroyed(handle);
    } else {
        registry.tick(handle);
    }
}

fn pump_thread_main() {
    // Out-of-context WinEvents are delivered to the installing thread via
    // its message queue, so the hook must be installed here and this thread
    // must pump messages for the rest of the process lifetime.
    let hook = unsafe {
        SetWinEventHook(
            EVENT_OBJECT_LOCATIONCHANGE,
            EVENT_OBJECT_LOCATIONCHANGE,
            None,
            WINEVENTPROC::Some(location_change_proc),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        )
    };
    let destroy_hook = unsafe {
        SetWinEventHook(
            EVENT_OBJECT_DESTROY,
            EVENT_OBJECT_DESTROY,
            None,
            WINEVENTPROC::Some(location_change_proc),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        )
    };
    let Some(registry) = REGISTRY.get() else {
        return;
    };
    if destroy_hook.is_invalid() {
        tracing::error!(
            code = "CAPTURE_WINDOW_GENERATION_HOOK_UNAVAILABLE",
            "SetWinEventHook(EVENT_OBJECT_DESTROY, out-of-context) failed; window \
             generations stay 0 and recycled-HWND detection degrades to \
             pid/thread/size identity"
        );
    }
    if hook.is_invalid() {
        tracing::error!(
            code = "CAPTURE_GEOMETRY_EPOCH_HOOK_UNAVAILABLE",
            "SetWinEventHook(EVENT_OBJECT_LOCATIONCHANGE, out-of-context) failed; \
             geometry epochs stay 0 and geometry-stability checks degrade to \
             bounds-equality-only"
        );
        return;
    }
    registry.active.store(true, Ordering::Release);
    let mut message = MSG::default();
    loop {
        // BOOL: 0 = WM_QUIT, -1 = error, anything else = a message was
        // dispatched (WinEvent deliveries arrive here).
        let fetched = unsafe { GetMessageW(&raw mut message, None, 0, 0) }.0;
        if fetched <= 0 {
            break;
        }
    }
    // Reaching here means the message queue is gone (session shutdown);
    // unhook so the OS never calls into a dead thread.
    let _ = unsafe { UnhookWinEvent(hook) };
    if !destroy_hook.is_invalid() {
        let _ = unsafe { UnhookWinEvent(destroy_hook) };
    }
    registry.active.store(false, Ordering::Release);
}

fn ensure_registry() -> &'static GeometryEpochRegistry {
    let registry = REGISTRY.get_or_init(GeometryEpochRegistry::new);
    PUMP_THREAD.get_or_init(|| {
        if let Err(error) = thread::Builder::new()
            .name("synapse-geometry-epoch".to_owned())
            .spawn(pump_thread_main)
        {
            tracing::error!(
                code = "CAPTURE_GEOMETRY_EPOCH_THREAD_UNAVAILABLE",
                error = %error,
                "geometry-epoch pump thread failed to start; geometry epochs stay 0 \
                 and geometry-stability checks degrade to bounds-equality-only"
            );
        }
    });
    registry
}

/// Returns the current geometry generation of `hwnd` (0 = no change ever
/// observed for this window in this process).
///
/// Monotonic for the process lifetime; ticks on every move or resize of that
/// window, including moves that return to the original position. Prove a
/// window was geometrically still across an operation by sampling before it
/// and comparing against [`geometry_epoch_quiescent`] after it.
pub fn geometry_epoch(hwnd: i64) -> u64 {
    ensure_registry().epoch(hwnd)
}

/// Delivery-safe epoch sample for `hwnd`: waits until the window's
/// generation has been unchanged for [`GEOMETRY_EPOCH_QUIET_MS`] (bounded by
/// [`GEOMETRY_EPOCH_MAX_WAIT_MS`]), then returns it.
pub fn geometry_epoch_quiescent(hwnd: i64) -> u64 {
    ensure_registry().epoch_quiescent(hwnd)
}

/// Returns the lifetime generation of the HWND value: ticks each time a
/// window owning that handle value is destroyed (0 = none observed in this
/// process).
///
/// Windows recycles HWND values, so per-window state keyed by the bare handle
/// can leak from a destroyed window to its successor. Fold this generation
/// into identity keys; a recycled HWND value then reads as a different window.
pub fn window_generation(hwnd: i64) -> u64 {
    ensure_registry().generation(hwnd)
}

/// Returns a health snapshot of the geometry-epoch registry.
pub fn geometry_epoch_status() -> GeometryEpochStatus {
    ensure_registry().status()
}
