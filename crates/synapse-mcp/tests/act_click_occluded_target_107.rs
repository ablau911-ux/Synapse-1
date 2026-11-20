//! #107: a claimed native target stays operable through the normal
//! target-scoped click when another window is in front of it, and the click's
//! effect is read from the TARGET window's own pixels (not a screen region that
//! shows the occluding window). The latest VBV on this issue failed exactly in
//! the occluded case.
//!
//! Contract: issue #107 Acceptance ("A selected/claimed native target remains
//! operable through the normal target-scoped action path when focus
//! changes"); m2/click/point_post.rs module doc (claimed point target that is
//! not foreground -> child-window PostMessage, caller verifies the
//! target-window pixel delta; a caller-supplied HWND alone never qualifies);
//! server/m2_tools.rs `capture_coordinate_click_delta_signature` (#191: pixels
//! from the target window's own capture).
//!
//! Fixture: external WinForms Button target, fully covered by this process's
//! topmost window. Button.Click persists counter.txt and repaints the target.
//! Raw WM_LBUTTONUP receipt is insufficient: the live defect delivered messages
//! without activating the Button, leaving its counter unchanged.

#![cfg(windows)]

#[path = "g4_window/foreground_events.rs"]
mod foreground_events;
mod g4_support;
mod g4_window;
#[allow(dead_code)]
mod support;

use std::{path::PathBuf, process::Stdio, time::Duration};

use anyhow::{Context, ensure};
use g4_support::{
    act_invoke_refused, claim_window, nested_code, normal_session, plain_daemon, set_window_target,
};
use g4_window::{OwnedWindow, Placement, foreground_root, root_at};
use serde_json::{Value, json};
use synapse_core::error_codes;

const NO_DELIVERY_WINDOW: Duration = Duration::from_millis(750);
// Both external fixtures use the same fixed physical rectangle.
static DESKTOP_FIXTURE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct ButtonTarget {
    child: tokio::process::Child,
    hwnd: i64,
    counter: PathBuf,
}

impl ButtonTarget {
    async fn open(state: PathBuf) -> anyhow::Result<Self> {
        std::fs::create_dir(&state)?;
        let mut child = tokio::process::Command::new("powershell.exe")
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
                child.try_wait()?.is_none(),
                "WinForms fixture exited before readiness"
            );
            ensure!(
                tokio::time::Instant::now() < deadline,
                "WinForms readiness timeout"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let ready: Value = serde_json::from_slice(&std::fs::read(ready_path)?)?;
        Ok(Self {
            child,
            hwnd: ready["hwnd"].as_i64().context("fixture HWND")?,
            counter: state.join("counter.txt"),
        })
    }

    fn count(&self) -> anyhow::Result<u32> {
        Ok(std::fs::read_to_string(&self.counter)?.parse()?)
    }

    async fn wait_for_count(&self, expected: u32, timeout: Duration) -> anyhow::Result<u32> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let count = self.count()?;
            if count == expected || tokio::time::Instant::now() >= deadline {
                return Ok(count);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

/// Target process differs from the topmost occluder's process (this runner).
fn cover_target(target: &ButtonTarget) -> OwnedWindow {
    assert_ne!(target.child.id(), Some(std::process::id()));
    let occluder = OwnedWindow::open(Placement {
        x: 60,
        y: 60,
        w: 280,
        h: 200,
        topmost: true,
    });
    assert_eq!(
        root_at(200, 160),
        Some(occluder.hwnd()),
        "precondition: the target's centre is covered by the occluder"
    );
    assert_ne!(
        foreground_root(),
        Some(target.hwnd),
        "precondition: the target is not the OS foreground"
    );
    occluder
}

fn window_click() -> Value {
    json!({"verb": "click", "x": 120, "y": 80, "coordinate_space": "window", "clicks": 1})
}

#[tokio::test]
async fn must_pass_claimed_occluded_target_click_is_verified_from_its_own_pixels()
-> anyhow::Result<()> {
    let _desktop = DESKTOP_FIXTURE.lock().await;
    let daemon = plain_daemon().await?;
    let mut session = normal_session(&daemon, "g4-107-claimed").await?;
    let mut target = ButtonTarget::open(daemon.home().join("occluded-107")).await?;
    let occluder = cover_target(&target);
    set_window_target(&mut session, target.hwnd).await?;
    claim_window(&mut session, target.hwnd).await?;
    let before = target.count()?;
    assert_eq!(before, 0, "fresh Button.Click counter");
    let foreground_before = foreground_root();
    let foreground_events = foreground_events::ForegroundEvents::start();

    // Retain errors until AFTER physical effect readback: no-delta refusal
    // must not hide the live 0->0 failure behind an early `?`.
    let result = session
        .tools_call(
            "act",
            json!({
                "operation":"invoke", "action":window_click()
            }),
        )
        .await;
    let after = target
        .wait_for_count(before + 1, Duration::from_secs(2))
        .await?;
    let transitions = foreground_events.finish();
    assert_eq!(
        after,
        before + 1,
        "#107 Button.Click SoT {before}->{after}; MCP={result:?}; foreground={transitions:?}"
    );
    assert!(
        transitions.is_empty(),
        "#107: even transient foreground loss/restoration is forbidden: {transitions:?}"
    );
    assert_eq!(
        foreground_root(),
        foreground_before,
        "foreground preserved exactly"
    );

    let result = result?;
    let structured = &result["structuredContent"];
    let action = &structured["action"];
    assert_eq!(action["ok"], true, "{structured}");
    assert_eq!(action["status"], "ok", "{structured}");
    assert_eq!(
        action["delivery_state"], "delivered_and_pixel_verified",
        "{structured}"
    );
    assert_eq!(
        action["result"]["backend_tier_used"], "postmessage",
        "{structured}"
    );
    assert_eq!(
        action["result"]["required_foreground"], false,
        "{structured}"
    );
    assert_eq!(target.count()?, 1, "exactly one Button.Click");
    assert_eq!(
        occluder.click_count(),
        0,
        "the occluding window is never clicked"
    );
    assert_ne!(
        foreground_root(),
        Some(target.hwnd),
        "no activation of the target"
    );
    target.child.kill().await?;
    drop(daemon.stop().await?);
    Ok(())
}

/// Must NOT change: an unclaimed background target is refused before delivery.
#[tokio::test]
async fn must_fail_unclaimed_occluded_target_click_is_refused_before_delivery() -> anyhow::Result<()>
{
    let _desktop = DESKTOP_FIXTURE.lock().await;
    let daemon = plain_daemon().await?;
    let mut session = normal_session(&daemon, "g4-107-unclaimed").await?;
    let mut target = ButtonTarget::open(daemon.home().join("unclaimed-107")).await?;
    let occluder = cover_target(&target);
    set_window_target(&mut session, target.hwnd).await?;
    assert_eq!(target.count()?, 0);

    let structured = act_invoke_refused(&mut session, window_click()).await?;

    assert_eq!(structured["action"]["ok"], false, "{structured}");
    assert_eq!(
        structured["action"]["delegated_tool"], "act_click",
        "{structured}"
    );
    assert_eq!(
        nested_code(&structured).as_deref(),
        Some(error_codes::TARGET_CLAIM_NOT_FOUND),
        "{structured}"
    );
    assert_eq!(
        target.wait_for_count(1, NO_DELIVERY_WINDOW).await?,
        0,
        "no click may reach the unclaimed target"
    );
    assert_eq!(occluder.click_count(), 0);
    target.child.kill().await?;
    drop(daemon.stop().await?);
    Ok(())
}
