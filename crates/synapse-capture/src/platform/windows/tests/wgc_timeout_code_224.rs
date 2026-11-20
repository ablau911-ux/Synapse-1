//! Regression for #224: a WGC frame timeout on a window (for example a
//! minimized one) must surface its own error code, not
//! `CAPTURE_PRINTWINDOW_DISABLED`.

use synapse_core::{Rect, error_codes};

use super::wgc_failure_to_capture_error;
use crate::CaptureError;

const REGION: Rect = Rect {
    x: 0,
    y: 0,
    w: 10,
    h: 10,
};

#[test]
fn must_pass_wgc_frame_timeout_keeps_its_own_code() {
    let timeout = CaptureError::WgcFrameTimeout {
        detail: "timed out after 2000 ms waiting for WGC window frame".to_owned(),
    };
    let mapped = wgc_failure_to_capture_error(0x10, REGION, timeout);
    assert_eq!(mapped.code(), error_codes::CAPTURE_WGC_FRAME_TIMEOUT);
    assert_ne!(mapped.code(), error_codes::CAPTURE_PRINTWINDOW_DISABLED);
    assert!(mapped.to_string().contains("waiting for WGC window frame"));
}

#[test]
fn must_fail_other_wgc_failures_stay_printwindow_disabled() {
    let other = CaptureError::ThreadFailed {
        detail: "channel disconnected".to_owned(),
    };
    let mapped = wgc_failure_to_capture_error(0x10, REGION, other);
    assert_eq!(mapped.code(), error_codes::CAPTURE_PRINTWINDOW_DISABLED);
    assert!(mapped.to_string().contains("channel disconnected"));
}
