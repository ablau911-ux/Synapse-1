#[cfg(windows)]
use std::{ffi::c_void, mem::size_of};

#[cfg(windows)]
use crate::config::{
    DESKTOP_READOBJECTS_ACCESS, DPI_AWARENESS_PER_MONITOR_V2, MONITORINFOF_PRIMARY, WIN32_TRUE,
    ZERO_I32, ZERO_ISIZE, ZERO_U32,
};
#[cfg(not(windows))]
use crate::config::{ZERO_I32, ZERO_U32};
use crate::error::{LivefeedError, Result};

#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ResolvedMonitor {
    pub(crate) index: u32,
    pub(crate) origin_x: i32,
    pub(crate) origin_y: i32,
    pub(crate) primary: bool,
}

#[cfg(windows)]
#[repr(C)]
#[derive(Copy, Clone, Default)]
struct WinRect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[cfg(windows)]
#[repr(C)]
#[derive(Copy, Clone, Default)]
struct WinMonitorInfo {
    cb_size: u32,
    rc_monitor: WinRect,
    rc_work: WinRect,
    dw_flags: u32,
}

#[cfg(windows)]
#[allow(unsafe_code)]
#[link(name = "user32")]
// SAFETY: `SetThreadDpiAwarenessContext`, `EnumDisplayMonitors`, `GetMonitorInfoW`,
// `OpenInputDesktop`, and `CloseDesktop` are standard Win32 user32 exports.
unsafe extern "system" {
    fn SetThreadDpiAwarenessContext(context: isize) -> isize;
    fn EnumDisplayMonitors(
        hdc: *mut c_void,
        clip: *const WinRect,
        callback: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut WinRect, isize) -> i32,
        data: isize,
    ) -> i32;
    fn GetMonitorInfoW(monitor: *mut c_void, info: *mut WinMonitorInfo) -> i32;
    fn OpenInputDesktop(flags: u32, inherit: i32, desired_access: u32) -> *mut c_void;
    fn CloseDesktop(desktop: *mut c_void) -> i32;
}

#[cfg(windows)]
#[allow(unsafe_code)]
unsafe extern "system" fn collect_monitor(
    handle: *mut c_void,
    _hdc: *mut c_void,
    _rect: *mut WinRect,
    data: isize,
) -> i32 {
    if data == ZERO_ISIZE {
        return ZERO_I32;
    }
    let mut info = WinMonitorInfo {
        cb_size: u32::try_from(size_of::<WinMonitorInfo>()).unwrap_or(ZERO_U32),
        ..WinMonitorInfo::default()
    };
    // SAFETY: `handle` is a live HMONITOR from `EnumDisplayMonitors`, `info` is stack-initialized,
    // and `data` points to the caller's stack `Vec<ResolvedMonitor>` for the synchronous call.
    unsafe {
        if GetMonitorInfoW(handle, &raw mut info) == ZERO_I32 {
            return ZERO_I32;
        }
        let monitors = &mut *(data as *mut Vec<ResolvedMonitor>);
        let index = u32::try_from(monitors.len()).unwrap_or(u32::MAX);
        monitors.push(ResolvedMonitor {
            index,
            origin_x: info.rc_monitor.left,
            origin_y: info.rc_monitor.top,
            primary: (info.dw_flags & MONITORINFOF_PRIMARY) != ZERO_U32,
        });
    }
    WIN32_TRUE
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn enumerate_monitors() -> Result<Vec<ResolvedMonitor>> {
    let mut monitors = Vec::<ResolvedMonitor>::new();
    // SAFETY: `EnumDisplayMonitors` invokes `collect_monitor` synchronously before returning,
    // while `monitors` remains exclusively borrowed on this stack frame.
    let ok = unsafe {
        let prev = SetThreadDpiAwarenessContext(DPI_AWARENESS_PER_MONITOR_V2);
        let res = EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            collect_monitor,
            (&raw mut monitors) as isize,
        );
        if prev != ZERO_ISIZE {
            let _ = SetThreadDpiAwarenessContext(prev);
        }
        res
    };
    if ok == ZERO_I32 || monitors.is_empty() {
        return Err(LivefeedError::capture_failed(
            "failed to enumerate display monitors",
        ));
    }
    Ok(monitors)
}

#[cfg(windows)]
pub(crate) fn resolve_monitor(requested: Option<u32>) -> Result<ResolvedMonitor> {
    let monitors = enumerate_monitors()?;
    if let Some(target_idx) = requested {
        return monitors
            .into_iter()
            .find(|m| m.index == target_idx)
            .ok_or_else(|| {
                LivefeedError::capture_failed(format!("monitor {target_idx} not found"))
            });
    }
    monitors
        .iter()
        .copied()
        .find(|m| m.primary)
        .or_else(|| monitors.first().copied())
        .ok_or_else(|| LivefeedError::capture_failed("no primary monitor found"))
}

#[cfg(not(windows))]
pub(crate) fn resolve_monitor(requested: Option<u32>) -> Result<ResolvedMonitor> {
    let _ = LivefeedError::NotRunning;
    Ok(ResolvedMonitor {
        index: requested.unwrap_or(ZERO_U32),
        origin_x: ZERO_I32,
        origin_y: ZERO_I32,
        primary: requested.is_none(),
    })
}

#[cfg(windows)]
#[allow(unsafe_code)]
pub(crate) fn is_session_locked() -> bool {
    // SAFETY: `OpenInputDesktop` takes no pointers and returns null or a valid desktop handle.
    let desktop = unsafe { OpenInputDesktop(ZERO_U32, ZERO_I32, DESKTOP_READOBJECTS_ACCESS) };
    if desktop.is_null() {
        return true;
    }
    // SAFETY: `desktop` is the valid non-null handle returned by `OpenInputDesktop`.
    let _ = unsafe { CloseDesktop(desktop) };
    false
}

#[cfg(not(windows))]
pub(crate) const fn is_session_locked() -> bool {
    false
}
