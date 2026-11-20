//! Workstation lock / secure-desktop detection (#9).
//!
//! While the workstation is locked (or a secure desktop such as UAC owns input)
//! no ordinary process can take the foreground: `SetForegroundWindow` is refused
//! and synthetic input lands on the lock surface, not the target. Callers use
//! this to classify the refusal as transient instead of a generic failure.

use crate::platform;

/// Image names of the processes that own the locked / secure input surface.
const LOCK_SURFACE_IMAGES: &[&str] = &["lockapp.exe", "logonui.exe"];

/// True when `image_path` (a full path or bare image name) is a lock-surface
/// process such as `LockApp.exe` (Windows lock screen) or `LogonUI.exe`.
#[must_use]
pub fn is_lock_surface_process(image_path: &str) -> bool {
    let name = image_path
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(image_path)
        .trim();
    LOCK_SURFACE_IMAGES
        .iter()
        .any(|image| name.eq_ignore_ascii_case(image))
}

/// True when the interactive input desktop is unavailable to this process
/// (workstation locked / secure desktop) or the foreground window belongs to a
/// lock-surface process. Always false on non-Windows platforms.
#[must_use]
pub fn workstation_locked() -> bool {
    platform::workstation_locked()
}

#[cfg(test)]
#[path = "tests/lock_state_9.rs"]
mod lock_state_9;
