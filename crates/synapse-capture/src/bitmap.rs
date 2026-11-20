use synapse_core::Rect;

use crate::{
    CaptureError, CapturedBgraBitmap, CapturedWindowBgraBitmap, CroppedBgraBitmap, platform,
};

pub use platform::{GEOMETRY_EPOCH_MAX_WAIT_MS, GEOMETRY_EPOCH_QUIET_MS, GeometryEpochStatus};

// `CapturedFrame`/`CapturedSoftwareBitmap` only feed the Windows-only WinRT
// `SoftwareBitmap` helpers below.
#[cfg(windows)]
use crate::{CapturedFrame, CapturedSoftwareBitmap};

#[cfg(windows)]
/// Copies a captured frame region into a `WinRT` `SoftwareBitmap`.
///
/// # Errors
///
/// Returns [`CaptureError`] when the region is empty/outside the frame, the
/// frame format is unsupported, or the D3D/WinRT copy fails.
pub fn captured_frame_region_to_software_bitmap(
    frame: &CapturedFrame,
    region: Rect,
) -> Result<CapturedSoftwareBitmap, CaptureError> {
    platform::captured_frame_region_to_software_bitmap(frame, region)
}

#[cfg(windows)]
/// Copies a captured frame region into raw BGRA bytes.
///
/// # Errors
///
/// Returns [`CaptureError`] when the region is empty/outside the frame, the
/// frame format is unsupported, or the D3D copy fails.
pub fn captured_frame_region_to_bgra_bitmap(
    frame: &CapturedFrame,
    region: Rect,
) -> Result<CapturedBgraBitmap, CaptureError> {
    platform::captured_frame_region_to_bgra_bitmap(frame, region)
}

#[cfg(windows)]
/// Captures a screen-coordinate region into a `WinRT` `SoftwareBitmap`.
///
/// # Errors
///
/// Returns [`CaptureError`] when the region is empty, extends outside the
/// virtual screen, or the `GDI`/`WinRT` copy fails.
pub fn screen_region_to_software_bitmap(
    region: Rect,
) -> Result<CapturedSoftwareBitmap, CaptureError> {
    platform::validate_screen_capture_region(region)?;
    platform::screen_region_to_software_bitmap(region)
}

/// Captures a screen-coordinate region into raw BGRA bytes.
///
/// Available on all platforms so `synapse-mcp`'s OCR/detection callers compile
/// everywhere. The real GDI capture exists only on Windows; on non-Windows the
/// platform impl returns `Err(GraphicsApiUnsupported)` (it never fabricates
/// pixels), so callers fail loudly instead of acting on mock image data.
///
/// # Errors
///
/// Returns [`CaptureError`] when the region is empty, extends outside the
/// virtual screen, or the `GDI` capture fails (Windows), or
/// `GraphicsApiUnsupported` on any non-Windows build.
pub fn screen_region_to_bgra_bitmap(region: Rect) -> Result<CapturedBgraBitmap, CaptureError> {
    #[cfg(windows)]
    platform::validate_screen_capture_region(region)?;
    platform::screen_region_to_bgra_bitmap(region)
}

/// Captures a window-relative region into raw BGRA bytes. Windows uses passive
/// WGC `CreateForWindow` capture and reports that backend in the result.
/// Non-Windows builds fail loudly.
///
/// # Errors
///
/// Returns [`CaptureError`] when the HWND/region is invalid, the target is
/// minimized, no WGC frame arrives, or the bitmap copy
/// fails. Synapse does not automatically call `PrintWindow`, because Windows re-enters target
/// process `WM_PRINT`/`WM_PRINTCLIENT` handlers for that API.
pub fn window_region_to_bgra_bitmap(
    hwnd: i64,
    region: Rect,
    timeout_ms: u64,
) -> Result<CapturedWindowBgraBitmap, CaptureError> {
    #[cfg(windows)]
    platform::validate_window_capture_target(hwnd)?;
    platform::window_region_to_bgra_bitmap(hwnd, region, timeout_ms)
}

/// Captures the entire window using the WGC frame's native dimensions, with no
/// client/window coordinate conversion.
///
/// This is the exact path for "capture/read the whole target window" because
/// it is immune to the `GetWindowRect` vs WGC-frame mismatch caused by
/// invisible DWM resize borders (#1203).
///
/// # Errors
///
/// Returns [`CaptureError`] when the HWND is invalid, the target is minimized,
/// no WGC frame arrives, WGC returns black output, or the bitmap copy fails
/// (Windows), or `GraphicsApiUnsupported` on non-Windows builds.
pub fn window_full_frame_to_bgra_bitmap(
    hwnd: i64,
    timeout_ms: u64,
) -> Result<CapturedWindowBgraBitmap, CaptureError> {
    #[cfg(windows)]
    platform::validate_window_capture_target(hwnd)?;
    platform::window_full_frame_to_bgra_bitmap(hwnd, timeout_ms)
}

#[cfg(windows)]
pub(crate) fn validate_window_frame_content(
    bitmap: &CapturedBgraBitmap,
) -> Result<(), CaptureError> {
    // Alpha is not evidence of rendered content: opaque black is still blank.
    // Exact RGB equality preserves even one-level contrast in dark windows.
    if bitmap
        .bytes
        .chunks_exact(4)
        .all(|pixel| pixel[..3] == [0, 0, 0])
    {
        return Err(CaptureError::FrameUnavailable {
            detail: "window capture returned all-black RGB pixels; retry after the target renders"
                .to_owned(),
        });
    }
    Ok(())
}

/// Explicit `PrintWindow` capture for a window-region bitmap.
///
/// This is intentionally separate from `window_region_to_bgra_bitmap`: callers
/// must opt in after proving the target is on a bounded worker desktop, because
/// Windows asks the target process to handle `WM_PRINT`/`WM_PRINTCLIENT`.
///
/// # Errors
///
/// Returns [`CaptureError`] when the HWND/region is invalid, `PrintWindow`
/// fails, returns all-zero pixels, or the `GDI` copy fails.
pub fn window_region_to_bgra_bitmap_printwindow(
    hwnd: i64,
    region: Rect,
) -> Result<CapturedWindowBgraBitmap, CaptureError> {
    platform::window_region_to_bgra_bitmap_printwindow(hwnd, region)
}

/// Returns the full window bitmap region used by `window_region_to_bgra_bitmap`.
/// For minimized Windows targets this uses the restored placement extent rather
/// than the minimized icon rectangle.
///
/// # Errors
///
/// Returns [`CaptureError`] when the HWND cannot be resolved or the resulting
/// bitmap bounds are empty/invalid.
pub fn window_capture_region(hwnd: i64) -> Result<Rect, CaptureError> {
    platform::window_capture_region(hwnd)
}

/// Returns the screen-coordinate bounds of a whole-window WGC surface.
///
/// Callers that need screen-space result coordinates should verify this
/// readback is stable across capture before using it as the frame origin.
///
/// # Errors
///
/// Returns [`CaptureError`] when the HWND or its DWM/restored bounds cannot be
/// resolved.
pub fn window_capture_bounds(hwnd: i64) -> Result<Rect, CaptureError> {
    platform::window_capture_bounds(hwnd)
}

/// Returns the creator thread id of a window.
///
/// Windows reuses HWND values, so `(hwnd, pid)` alone is not a window-lifetime
/// identity: a destroyed and recreated window can collide with a stale one.
/// The thread id narrows that reuse window. Returns `None` when the HWND is
/// invalid, or on non-Windows builds.
#[must_use]
pub fn window_thread_id(hwnd: i64) -> Option<u32> {
    platform::window_thread_id(hwnd)
}

/// Converts a `WINDOWPLACEMENT::rcNormalPosition` rect into screen coordinates.
///
/// For top-level windows with `WS_CAPTION`, `rcNormalPosition` is relative to
/// the work area, not the screen, so a taskbar docked to the top/left shifts
/// the raw values by its extent. `work_area` is the screen-space work-area
/// rect (e.g. from `SPI_GETWORKAREA`); popup-window placements are already
/// screen-relative and must not be offset.
#[must_use]
pub const fn workspace_rect_to_screen(rect: Rect, work_area: Rect) -> Rect {
    Rect {
        x: rect.x.saturating_add(work_area.x),
        y: rect.y.saturating_add(work_area.y),
        w: rect.w,
        h: rect.h,
    }
}

impl CapturedBgraBitmap {
    /// Crops a screen-space rect out of this bitmap.
    ///
    /// The bitmap's own `region` anchors its pixel `(0,0)` in screen space;
    /// for per-window WGC frames that anchor is the DWM capture bounds, which
    /// differ from `GetWindowRect` bounds by the invisible resize border. The
    /// crop clamps to the bitmap, and empty or fully-outside rects return
    /// `None`, so a stale or oversized rect can never fabricate content.
    #[must_use]
    pub fn crop_screen_rect(&self, screen_rect: Rect) -> Option<CroppedBgraBitmap> {
        let bitmap_w = i32::try_from(self.width).unwrap_or(i32::MAX);
        let bitmap_h = i32::try_from(self.height).unwrap_or(i32::MAX);
        let left = screen_rect
            .x
            .saturating_sub(self.region.x)
            .clamp(0, bitmap_w);
        let top = screen_rect
            .y
            .saturating_sub(self.region.y)
            .clamp(0, bitmap_h);
        let right = screen_rect
            .x
            .saturating_add(screen_rect.w)
            .saturating_sub(self.region.x)
            .clamp(left, bitmap_w);
        let bottom = screen_rect
            .y
            .saturating_add(screen_rect.h)
            .saturating_sub(self.region.y)
            .clamp(top, bitmap_h);
        let crop_w = right.saturating_sub(left);
        let crop_h = bottom.saturating_sub(top);
        if crop_w <= 0 || crop_h <= 0 {
            return None;
        }
        let stride = self
            .width
            .checked_mul(4)
            .and_then(|stride| usize::try_from(stride).ok())?;
        let row_bytes = usize::try_from(crop_w).ok()?.checked_mul(4)?;
        let start_x = usize::try_from(left).ok()?.checked_mul(4)?;
        let mut bytes = Vec::with_capacity(row_bytes.checked_mul(usize::try_from(crop_h).ok()?)?);
        for row in top..bottom {
            let offset = usize::try_from(row)
                .ok()?
                .checked_mul(stride)?
                .checked_add(start_x)?;
            let row_slice = self.bytes.get(offset..offset.checked_add(row_bytes)?)?;
            bytes.extend_from_slice(row_slice);
        }
        Some(CroppedBgraBitmap {
            region: Rect {
                x: self.region.x.saturating_add(left),
                y: self.region.y.saturating_add(top),
                w: crop_w,
                h: crop_h,
            },
            width: crop_w.unsigned_abs(),
            height: crop_h.unsigned_abs(),
            bytes,
        })
    }
}

/// Converts a client-relative region to the full-window coordinate space used
/// by per-window WGC frames for this HWND.
///
/// # Errors
///
/// Returns [`CaptureError`] when the HWND cannot be resolved.
pub fn client_region_to_window_region(hwnd: i64, region: Rect) -> Result<Rect, CaptureError> {
    client_region_to_window_regions(hwnd, region).map(|resolved| resolved.window)
}

/// A client-relative region resolved against a window's capture bitmap. A
/// region that partly overhangs the client area, or the bitmap (a maximized
/// window's client area overhangs the monitor), is clipped: `window` is the
/// clipped region in bitmap coordinates and `client` the same pixels in client
/// coordinates (#233). Only a region with no overlap is refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClientWindowRegion {
    pub window: Rect,
    pub client: Rect,
}

/// Like [`client_region_to_window_region`], also returning the clipped region
/// in client coordinates so readbacks can report the pixels actually read.
///
/// # Errors
///
/// Returns [`CaptureError`] when the HWND cannot be resolved or the region has
/// no pixel inside the window bitmap.
pub fn client_region_to_window_regions(
    hwnd: i64,
    region: Rect,
) -> Result<ClientWindowRegion, CaptureError> {
    platform::client_region_to_window_regions(hwnd, region)
}

/// Returns the current geometry generation of the window: a monotonic counter
/// that ticks on every move or resize of that window, including moves that
/// return to the original position.
///
/// Windows does not expose this primitive; the capture crate builds it from a
/// process-wide `EVENT_OBJECT_LOCATIONCHANGE` `WinEvent` hook. Sample before an
/// operation and compare against [`geometry_epoch_quiescent`] after it to
/// prove the window was geometrically still throughout. Always `0` on
/// non-Windows builds.
#[must_use]
pub fn geometry_epoch(hwnd: i64) -> u64 {
    platform::geometry_epoch(hwnd)
}

/// Delivery-safe geometry-epoch sample: waits until the window's generation
/// has been stable for a short quiet window before returning it, so `WinEvent`
/// deliveries still in flight are not missed.
#[must_use]
pub fn geometry_epoch_quiescent(hwnd: i64) -> u64 {
    platform::geometry_epoch_quiescent(hwnd)
}

/// Returns the lifetime generation of an HWND value: ticks each time a window
/// owning that handle value is destroyed. Windows recycles HWND values, so
/// identity keys for per-window state must include this generation. Always `0`
/// on non-Windows builds.
#[must_use]
pub fn window_generation(hwnd: i64) -> u64 {
    platform::window_generation(hwnd)
}

/// Returns a health snapshot of the geometry-epoch registry.
#[must_use]
pub fn geometry_epoch_status() -> GeometryEpochStatus {
    platform::geometry_epoch_status()
}
