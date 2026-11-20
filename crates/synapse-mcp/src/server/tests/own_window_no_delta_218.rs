//! #218 through the REAL `verify_click_delta` on a real background window.
//!
//! The test creates its own no-activate window T (never the OS foreground) and
//! drives the click verifier exactly as the router does after a delivered
//! click: capture T's signature, then verify. No input is sent at all.
//! - T unchanged: the delivered click is reported delivered-but-unverified
//!   (`delivered:true` + retry hint, so agents do not blindly re-click), the
//!   readback names T as the observed window and the REAL OS foreground
//!   separately (never T).
//! - T's content really changed: the click verifies with `observed_delta`.

use super::*;
use crate::server::own_window_fixture::{OwnWindows, WindowSpec};

const VERIFY_MS: u32 = 50;

fn background_target() -> OwnWindows {
    OwnWindows::spawn(vec![WindowSpec {
        x: 128,
        y: 128,
        w: 320,
        h: 200,
        label: Some("unchanged label"),
    }])
}

async fn before(service: &SynapseService, hwnd: i64) -> ClickDeltaSignature {
    let signature = service
        .capture_click_delta_signature(160, Some(hwnd))
        .await
        .expect("target window signature");
    assert_eq!(
        signature.foreground_hwnd, hwnd,
        "precondition: T is the observed window"
    );
    signature
}

fn service() -> (tempfile::TempDir, SynapseService) {
    let dir = tempfile::tempdir().expect("tmp");
    let service = SynapseService::try_new_isolated_for_tests(dir.path()).expect("service");
    (dir, service)
}

#[tokio::test]
async fn must_fail_unchanged_background_target_is_delivered_but_unverified() {
    let windows = background_target();
    let hwnd = windows.hwnd(0);
    let (_dir, service) = service();
    let before = before(&service, hwnd).await;

    let Err(error) = service
        .verify_click_delta(before, VERIFY_MS, Some(hwnd))
        .await
    else {
        panic!("an unchanged target must not verify a click");
    };

    let data = error.data.as_ref().expect("structured error data");
    assert_eq!(
        data["code"],
        error_codes::ACTION_NO_OBSERVED_DELTA,
        "{data}"
    );
    assert_eq!(data["delivered"], true, "{data}");
    assert_eq!(
        data["retry_hint"],
        crate::m2::postcondition::DELIVERED_NO_DELTA_RETRY_HINT,
        "{data}"
    );
    assert_eq!(
        data["source_of_truth"],
        click_delta_source_of_truth(Some(hwnd)),
        "{data}"
    );
    let readback = &data["verify_delta"]["readback"];
    assert_eq!(readback["observed_window_hwnd"], hwnd, "{readback}");
    assert_ne!(
        readback["os_foreground_hwnd"],
        json!(hwnd),
        "the OS foreground readback must not report the background target: {readback}"
    );
    assert_eq!(readback["focus_only_change"], false, "{readback}");
}

#[tokio::test]
async fn must_pass_real_change_on_the_background_target_verifies() {
    let windows = background_target();
    let hwnd = windows.hwnd(0);
    let (_dir, service) = service();
    let before = before(&service, hwnd).await;

    windows.set_label_text(0, "label changed by the click effect");
    let postcondition = service
        .verify_click_delta(before, VERIFY_MS, Some(hwnd))
        .await
        .expect("a real element change verifies the click");

    assert_eq!(postcondition.status, "observed_delta");
    assert_eq!(
        postcondition.source_of_truth.as_deref(),
        Some(click_delta_source_of_truth(Some(hwnd)))
    );
}
