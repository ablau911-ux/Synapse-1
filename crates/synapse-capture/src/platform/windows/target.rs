use synapse_core::Rect;
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, IsIconic, IsWindow, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
    SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};
use windows_capture::monitor::Monitor;

use crate::CaptureError;

use super::common::hwnd_from_i64;

pub fn validate_hwnd(hwnd: i64) -> Result<(), CaptureError> {
    let hwnd = hwnd_from_i64(hwnd);
    if unsafe { IsWindow(Some(hwnd)) }.as_bool() {
        Ok(())
    } else {
        Err(CaptureError::TargetInvalid {
            detail: "HWND is not a live window".to_owned(),
        })
    }
}

pub fn validate_window_capture_target(hwnd: i64) -> Result<(), CaptureError> {
    validate_hwnd(hwnd)?;
    // SAFETY: IsIconic only reads the state of the validated window handle.
    if unsafe { IsIconic(hwnd_from_i64(hwnd)) }.as_bool() {
        return Err(CaptureError::TargetMinimized {
            detail: format!("hwnd {hwnd:#x} is minimized; restore it before capturing"),
        });
    }
    Ok(())
}

pub fn validate_screen_capture_region(region: Rect) -> Result<(), CaptureError> {
    // SAFETY: GetSystemMetrics reads desktop geometry without mutating it.
    let (left, top, width, height) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    let right = i64::from(left) + i64::from(width);
    let bottom = i64::from(top) + i64::from(height);
    // Reject partial overhang too: GDI may succeed with uncaptured DIB pixels.
    // Negative origins remain valid on monitors left of/above the primary.
    if width <= 0
        || height <= 0
        || region.w <= 0
        || region.h <= 0
        || region.x < left
        || region.y < top
        || i64::from(region.x) + i64::from(region.w) > right
        || i64::from(region.y) + i64::from(region.h) > bottom
    {
        return Err(CaptureError::TargetInvalid {
            detail: format!(
                "screen capture region {region:?} is not fully inside virtual screen ({left}, {top}, {width}, {height})"
            ),
        });
    }
    Ok(())
}

pub fn validate_monitor(monitor_index: u32) -> Result<(), CaptureError> {
    let windows_capture_index =
        usize::try_from(monitor_index.saturating_add(1)).map_err(|err| {
            CaptureError::TargetInvalid {
                detail: err.to_string(),
            }
        })?;
    Monitor::from_index(windows_capture_index)
        .map(|_monitor| ())
        .map_err(|err| CaptureError::TargetInvalid {
            detail: err.to_string(),
        })
}
