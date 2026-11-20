//! #315: one rule for "is this raw HWND usable". Pure: the platform layer
//! supplies the `IsWindow` answer, so the decision is testable on any OS.

use crate::{A11yError, A11yResult};

/// Decides whether a raw HWND is usable. `is_window` is the platform's
/// `IsWindow` answer for `raw`.
pub fn hwnd_validity(raw: i64, is_window: bool) -> A11yResult<()> {
    if raw == 0 {
        return Err(A11yError::NoForeground {
            detail: "HWND was null".to_owned(),
        });
    }
    if is_window {
        Ok(())
    } else {
        Err(A11yError::WindowNotFound {
            detail: format!("HWND 0x{raw:x} is not a valid window"),
        })
    }
}

/// For an HWND read from the OS foreground: a window that vanished between the
/// foreground read and its own readback means "no foreground right now".
#[must_use]
pub fn as_foreground_error(error: A11yError) -> A11yError {
    match error {
        A11yError::WindowNotFound { detail } => A11yError::NoForeground { detail },
        other => other,
    }
}

#[cfg(test)]
#[path = "hwnd_check_table_315.rs"]
mod hwnd_check_table_315;
