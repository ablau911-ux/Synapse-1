//! Frozen criteria_b2 #46; V2_issues_2 row 46: wholly off-screen region
//! published a successful black PNG. Exercise strict tools/call, then read
//! the artifact directory separately, even when the refusal expectation fails.
#![cfg(windows)]

mod g4_support;
#[allow(dead_code)]
mod support;

use serde_json::json;
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

#[tokio::test]
async fn must_fail_wholly_outside_virtual_screen_screenshot_without_publication_46()
-> anyhow::Result<()> {
    let daemon = g4_support::plain_daemon().await?;
    let mut session = g4_support::normal_session(&daemon, "b2-screen-bounds-46").await?;
    // Dynamic bounds: negative monitor origins are valid. Far beyond BOTH
    // extents avoids depending on the host's current monitor arrangement/DPI.
    let (x, y, w, h) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    assert!(w > 0 && h > 0, "interactive virtual-screen precondition");
    let path = daemon.home().join("outside-must-not-exist.png");
    assert!(!path.exists(), "artifact absent before trigger");
    let refusal = session
        .tools_call_error(
            "screenshot",
            json!({
                "operation": "capture", "path": path,
                "region": {"x": x + w + 1_000_000, "y": y + h + 1_000_000, "w": 100, "h": 100}
            }),
        )
        .await;

    assert!(
        !path.exists(),
        "#46: wholly off-screen tools/call published an artifact; response={refusal:?}"
    );
    let error = refusal?;
    let code = error["data"]["code"].as_str();
    assert!(
        code.is_some_and(|c| c.starts_with("CAPTURE_") || c == "TOOL_PARAMS_INVALID"),
        "off-screen region needs typed capture/parameter refusal: {error}"
    );
    drop(daemon.stop().await?);
    Ok(())
}
