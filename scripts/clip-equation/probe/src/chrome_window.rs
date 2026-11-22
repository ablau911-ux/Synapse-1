//! Package 0b: a Chrome `--app` window the probe opens itself, with its own
//! throw-away profile in a temporary folder. The probe never sends input to
//! it: each page drives itself with JavaScript. Drop closes the window, ends
//! that Chrome instance and deletes the profile.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetForegroundWindow, GetWindowTextW, HWND_BOTTOM, IsWindowVisible,
    PostMessageW, SWP_NOACTIVATE, SWP_NOSIZE, SetWindowPos, WM_CLOSE,
};
use windows::core::BOOL;

use crate::Result;

const CHROME: &str = r"C:\Program Files\Google\Chrome\Application\chrome.exe";
const CHROME_CLASS: &str = "Chrome_WidgetWin_1";
const FIND_TIMEOUT: Duration = Duration::from_secs(20);
const EXIT_TIMEOUT: Duration = Duration::from_secs(6);
/// Where Chrome first shows the window: right of a 1920-pixel-wide screen,
/// so nothing appears in front of the owner's windows before it is moved.
const OFFSCREEN: (i32, i32) = (2600, 0);

pub struct ChromeWindow {
    child: Child,
    hwnd: isize,
    profile: PathBuf,
}

impl ChromeWindow {
    /// Opens `page` (a local file plus an optional query) as an app window at
    /// `(x, y)` with outer size `width` × `height`; waits for the window whose
    /// title is `title` that was not there before.
    pub fn open(
        page: &Path,
        query: &str,
        title: &str,
        position: (i32, i32),
        size: (i32, i32),
    ) -> Result<Self> {
        let before: HashSet<isize> = chrome_windows().into_iter().map(|(hwnd, _)| hwnd).collect();
        let profile = std::env::temp_dir().join(format!("clip0b-chrome-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&profile);
        let url = format!(
            "file:///{}{query}",
            page.to_string_lossy().replace('\\', "/")
        );
        // Owner rule (2026-11-22): the window opens off the screen and is then
        // moved into place BEHIND every other window, never in front.
        let foreground_before = unsafe { GetForegroundWindow() }.0 as isize;
        let child = Command::new(CHROME)
            .arg(format!("--app={url}"))
            .arg(format!("--user-data-dir={}", profile.display()))
            .arg(format!("--window-position={},{}", OFFSCREEN.0, OFFSCREEN.1))
            .arg(format!("--window-size={},{}", size.0, size.1))
            .args([
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-extensions",
                "--disable-sync",
                "--disable-background-networking",
                "--disable-component-update",
                "--disable-backgrounding-occluded-windows",
                "--disable-renderer-backgrounding",
                "--disable-background-timer-throttling",
                "--disable-features=CalculateNativeWinOcclusion",
                "--force-device-scale-factor=1",
            ])
            .spawn()
            .map_err(|error| format!("start Chrome: {error}"))?;
        let mut window = Self {
            child,
            hwnd: 0,
            profile,
        };
        let deadline = Instant::now() + FIND_TIMEOUT;
        while Instant::now() < deadline {
            if let Some((hwnd, _)) = chrome_windows()
                .into_iter()
                .find(|(hwnd, text)| !before.contains(hwnd) && text == title)
            {
                window.hwnd = hwnd;
                // Into place at the bottom of the z-order, without activation
                // (owner rule: behind the owner's windows; the occlusion flags
                // above keep a covered window rendering).
                // SAFETY: the probe's own window; no focus change.
                unsafe {
                    SetWindowPos(
                        HWND(hwnd as *mut _),
                        Some(HWND_BOTTOM),
                        position.0,
                        position.1,
                        0,
                        0,
                        SWP_NOSIZE | SWP_NOACTIVATE,
                    )
                }
                .map_err(crate::win("SetWindowPos"))?;
                let foreground_after = unsafe { GetForegroundWindow() }.0 as isize;
                if foreground_after == hwnd || foreground_after != foreground_before {
                    println!("chrome_window note=foreground changed (before {foreground_before:#x}, after {foreground_after:#x}, probe window {hwnd:#x})");
                }
                return Ok(window);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Err(format!(
            "no Chrome window titled '{title}' within {FIND_TIMEOUT:?}"
        ))
    }

    pub const fn raw_hwnd(&self) -> isize {
        self.hwnd
    }
}

impl Drop for ChromeWindow {
    fn drop(&mut self) {
        if self.hwnd != 0 {
            // SAFETY: a close request to the probe's own window.
            let _ = unsafe {
                PostMessageW(
                    Some(HWND(self.hwnd as *mut _)),
                    WM_CLOSE,
                    WPARAM(0),
                    LPARAM(0),
                )
            };
        }
        let deadline = Instant::now() + EXIT_TIMEOUT;
        let mut exited = false;
        while Instant::now() < deadline {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                exited = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if !exited {
            // Still running, so the PID is still this Chrome instance: end it
            // with its renderer and GPU processes. Never after it exited (the
            // PID could then belong to another program).
            let _ = Command::new("taskkill")
                .args(["/PID", &self.child.id().to_string(), "/T", "/F"])
                .output();
        }
        let _ = self.child.wait();
        for _ in 0..20 {
            if std::fs::remove_dir_all(&self.profile).is_ok() || !self.profile.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
}

/// Visible top-level Chrome windows with their titles.
fn chrome_windows() -> Vec<(isize, String)> {
    unsafe extern "system" fn collect(hwnd: HWND, list: LPARAM) -> BOOL {
        // SAFETY: `list` is the Vec passed below, alive for the whole EnumWindows call.
        let list = unsafe { &mut *(list.0 as *mut Vec<(isize, String)>) };
        // SAFETY: read-only queries on a window handle given by EnumWindows.
        unsafe {
            if IsWindowVisible(hwnd).as_bool() {
                let mut class = [0_u16; 64];
                let length = GetClassNameW(hwnd, &mut class) as usize;
                if String::from_utf16_lossy(&class[..length]) == CHROME_CLASS {
                    let mut text = [0_u16; 256];
                    let length = GetWindowTextW(hwnd, &mut text) as usize;
                    list.push((hwnd.0 as isize, String::from_utf16_lossy(&text[..length])));
                }
            }
        }
        BOOL(1)
    }
    let mut list: Vec<(isize, String)> = Vec::new();
    // SAFETY: the callback only writes into `list`, which outlives the call.
    let _ = unsafe { EnumWindows(Some(collect), LPARAM(&raw mut list as isize)) };
    list
}
