//! Frozen criteria_b2 #7; V2_issues_2 row 7: PostMessage claimed delivery,
//! but WinForms Button.Click counter stayed 0; UIA Invoke worked.
//! A raw WM_LBUTTONUP counter (g4_window) cannot detect this defect.
#![cfg(windows)]

mod g4_support;
mod g4_window;
#[allow(dead_code)]
mod support;

use std::{process::Stdio, time::Duration};

use anyhow::{Context, ensure};
use g4_support::{claim_window, no_foreground_daemon, normal_session, set_window_target};
use g4_window::{OwnedWindow, Placement, foreground_root, root_at};
use serde_json::{Value, json};

#[tokio::test]
async fn must_pass_claimed_background_native_button_point_click_activates_once_7()
-> anyhow::Result<()> {
    let daemon = no_foreground_daemon().await?;
    let mut session = normal_session(&daemon, "b2-native-button-7").await?;
    let state = daemon.home().join("native-button-state");
    std::fs::create_dir(&state)?;
    let mut fixture = tokio::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-STA",
            "-WindowStyle",
            "Hidden",
            "-File",
        ])
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/b2_native_button_7/fixture.ps1"
        ))
        .arg("-StateDirectory")
        .arg(&state)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let ready_path = state.join("ready.json");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while !ready_path.exists() {
        ensure!(
            fixture.try_wait()?.is_none(),
            "WinForms fixture exited before readiness"
        );
        ensure!(
            tokio::time::Instant::now() < deadline,
            "WinForms fixture readiness timeout"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let ready: Value = serde_json::from_slice(&std::fs::read(&ready_path)?)?;
    let hwnd = ready["hwnd"].as_i64().context("fixture HWND")?;
    // Reuse desktop fixture helper for an owned occluder; no human window input.
    let cover = OwnedWindow::open(Placement {
        x: 60,
        y: 60,
        w: 280,
        h: 200,
        topmost: true,
    });
    ensure!(
        root_at(200, 160) == Some(cover.hwnd()),
        "fixture is covered"
    );
    ensure!(foreground_root() != Some(hwnd), "fixture is non-foreground");
    set_window_target(&mut session, hwnd).await?;
    claim_window(&mut session, hwnd).await?;
    let counter_path = state.join("counter.txt");
    assert_eq!(std::fs::read_to_string(&counter_path)?, "0");
    let foreground_before = foreground_root();

    // Keep result until AFTER independent effect readback: today's no-delta
    // error must not hide the actual 0->0 defect behind an early `?`.
    let result = session
        .tools_call(
            "act",
            json!({
                "operation": "invoke",
                "action": {
                    "verb": "click", "x": 120, "y": 80,
                    "coordinate_space": "window", "clicks": 1
                }
            }),
        )
        .await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let count = loop {
        let count = std::fs::read_to_string(&counter_path)?;
        if count == "1" || tokio::time::Instant::now() >= deadline {
            break count;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    assert_eq!(foreground_root(), foreground_before, "foreground preserved");
    assert_eq!(cover.click_count(), 0, "cover must receive no click");
    assert_eq!(
        count, "1",
        "#7: native Button.Click counter must change 0->1; MCP={result:?}"
    );
    let result = result?;
    let action = &result["structuredContent"]["action"];
    assert_eq!(action["ok"], true, "{result}");
    assert_ne!(action["delivery_state"], "delivered_unverified", "{result}");
    fixture.kill().await?;
    drop(daemon.stop().await?);
    Ok(())
}
