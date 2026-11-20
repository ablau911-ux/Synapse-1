mod bitmap;
mod capture;
mod common;
mod coords;
mod dpi;
mod geometry_epoch;
mod readback;
mod target;

pub use bitmap::{
    captured_frame_region_to_bgra_bitmap, captured_frame_region_to_software_bitmap,
    client_region_to_window_regions, screen_region_to_bgra_bitmap,
    screen_region_to_software_bitmap, window_capture_bounds, window_capture_region,
    window_full_frame_to_bgra_bitmap, window_region_to_bgra_bitmap,
    window_region_to_bgra_bitmap_printwindow, window_thread_id,
};
pub use capture::{run_dxgi_capture, run_graphics_capture};
pub use coords::{
    screen_to_window as screen_to_window_impl, window_to_screen as window_to_screen_impl,
};
pub use dpi::{
    current_thread_priority as current_thread_priority_impl,
    init_process_dpi_awareness as init_process_dpi_awareness_impl,
    is_per_monitor_v2_dpi_aware as is_per_monitor_v2_dpi_aware_impl, set_capture_thread_priority,
};
pub use geometry_epoch::{
    GEOMETRY_EPOCH_MAX_WAIT_MS, GEOMETRY_EPOCH_QUIET_MS, GeometryEpochStatus, geometry_epoch,
    geometry_epoch_quiescent, geometry_epoch_status, window_generation,
};
pub use target::{
    validate_hwnd as validate_hwnd_impl, validate_monitor as validate_monitor_impl,
    validate_screen_capture_region, validate_window_capture_target,
};
