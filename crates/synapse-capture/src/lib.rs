#![allow(unsafe_code)]

mod backend;
mod bitmap;
mod config;
mod controller;
mod coords;
mod dpi;
mod error;
mod frame;
mod platform;
mod stats;

pub use backend::{CaptureBackend, CaptureBackendPreference};
// `screen_region_to_bgra_bitmap` is cross-platform (fails loud off Windows); the
// WinRT `SoftwareBitmap` helpers in `bitmap` stay `#[cfg(windows)]`, so off
// Windows this glob re-exports only the BGRA entry point that `synapse-mcp` calls.
pub use bitmap::*;
pub use config::{CaptureConfig, CaptureTarget, ResolvedCaptureTarget};
pub use controller::{
    CaptureController, CaptureHandle, register_capture_metrics, resolve_capture_target,
    spawn_capture_loop, validate_hwnd,
};
pub use coords::*;
pub use dpi::*;
pub use error::*;
pub use frame::*;
// The clip recorder's own observer (plan section O); Windows only.
#[cfg(windows)]
pub use platform::{
    ClipObserver, ClipSource, HNS_PER_MS, MAX_SELECTION_LATENESS_INSTANTS, MIN_WINDOWS_BUILD,
    OBSERVE_SLOTS, ObservedFrame, ObservedPixels, ObserverSettings, ObserverStats,
    REQUESTED_UPDATE_INTERVAL_MS, now_hns, windows_build,
};
pub use stats::{CaptureStats, CaptureThreadPriority};

pub const CAPTURE_CHANNEL_CAPACITY: usize = 2;
pub const FRAMES_DROPPED_METRIC: &str = "synapse_capture_frames_dropped_total";
