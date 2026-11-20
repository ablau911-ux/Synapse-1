#![allow(unsafe_code)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use synapse_core::Rect;
use windows::Win32::Foundation::{LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext,
};
use windows::core::BOOL;

#[derive(Clone, Copy, Debug)]
pub struct Monitor {
    pub index: u32,
    pub rect: Rect,
    pub primary: bool,
}

unsafe extern "system" fn collect(handle: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..MONITORINFO::default()
    };
    // SAFETY: live enumerated handle, initialized output, and the Vec passed by monitors().
    unsafe {
        if !GetMonitorInfoW(handle, &raw mut info).as_bool() {
            return BOOL(0);
        }
        let monitors = &mut *(data.0 as *mut Vec<Monitor>);
        let rect = info.rcMonitor;
        monitors.push(Monitor {
            index: monitors.len() as u32,
            rect: Rect {
                x: rect.left,
                y: rect.top,
                w: rect.right - rect.left,
                h: rect.bottom - rect.top,
            },
            primary: info.dwFlags & 1 != 0,
        });
    }
    BOOL(1)
}

pub fn monitors() -> Vec<Monitor> {
    let mut monitors = Vec::<Monitor>::new();
    // SAFETY: EnumDisplayMonitors calls collect synchronously; Vec remains live.
    let result = unsafe {
        let previous = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let result = EnumDisplayMonitors(
            None,
            None,
            Some(collect),
            LPARAM((&raw mut monitors) as isize),
        );
        SetThreadDpiAwarenessContext(previous);
        result
    };
    assert!(result.as_bool(), "enumerate real monitor layout");
    assert_eq!(monitors.iter().filter(|monitor| monitor.primary).count(), 1);
    monitors
}
