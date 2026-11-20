//! #92: a key chord whose success opens a window in a CHILD PROCESS of the
//! target (Tauri/WebView2 DevTools) is verified as `related_window_observed`,
//! with the descendant PIDs read from the LIVE process table, not injected.
//! The existing `related_window_child_process_92` tests hand the decision a
//! ready-made PID list; these drive the native collection (`sysinfo`
//! snapshot) with a real child process the test spawns, and the evidence
//! filling step `verify_action_delta` performs.
//!
//! NEEDS-SEAM (fixer extracts from `verify_action_delta`, behaviour unchanged):
//!   fn fill_related_window_evidence(
//!       evidence: &mut RelatedWindowEvidence,
//!       before: &ClickDeltaSignature,
//!       after: &ClickDeltaSignature,
//!   )
//! (owner chain of `after.foreground_hwnd`; descendant PIDs of
//! `before.foreground_pid` only for a new window of another process).

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use super::*;
use crate::m2::process_descendants::live_descendant_pids;

const TARGET_HWND: i64 = 0x100;
const NEW_HWND: i64 = 0x200;
/// PID 4 is the Windows System process: never a descendant of a test binary.
const SYSTEM_PID: u32 = 4;

/// A real child process of this test, killed on drop.
struct ChildProcess(Child);

impl Drop for ChildProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn_child() -> ChildProcess {
    let child = Command::new("cmd")
        .args(["/C", "ping -n 30 127.0.0.1 >NUL"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap_or_else(|error| panic!("spawn cmd child: {error}"));
    ChildProcess(child)
}

/// Polls the live process table until `pid` shows up under this process.
fn wait_for_descendant(pid: u32) -> Vec<u32> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let found = live_descendant_pids(std::process::id());
        if found.contains(&pid) || Instant::now() >= deadline {
            return found;
        }
        std::thread::yield_now();
    }
}

#[cfg(windows)]
#[test]
fn must_pass_live_snapshot_lists_a_real_child_process() {
    let child = spawn_child();
    let pid = child.0.id();
    let found = wait_for_descendant(pid);
    assert!(found.contains(&pid), "child {pid} missing from {found:?}");
    assert!(
        !found.contains(&std::process::id()),
        "root is not its own descendant"
    );
    assert!(!found.contains(&SYSTEM_PID));
}

#[cfg(windows)]
#[test]
fn must_fail_unknown_root_has_no_live_descendants() {
    assert_eq!(live_descendant_pids(u32::MAX), Vec::<u32>::new());
}

fn evidence(prior_windows: Vec<i64>) -> RelatedWindowEvidence {
    RelatedWindowEvidence {
        prior_windows,
        owner_chain: Vec::new(),
        descendant_pids: Vec::new(),
    }
}

fn verify(
    before: ClickDeltaSignature,
    after: ClickDeltaSignature,
    evidence: RelatedWindowEvidence,
) -> Result<ActPostcondition, ErrorData> {
    let mut policy = ForegroundChangePolicy::reject();
    policy.related_window = Some(evidence);
    verify_captured_action_delta(
        "act_press",
        "foreground_focused_ui_or_pixels",
        250,
        before,
        after,
        None,
        policy,
    )
}

fn target() -> ClickDeltaSignature {
    tests::click_signature(
        TARGET_HWND,
        std::process::id(),
        "synapse-test.exe",
        "Target",
        1,
    )
}

fn foreground_lost(result: Result<ActPostcondition, ErrorData>) -> bool {
    matches!(result, Err(error) if error.data.as_ref().is_some_and(|data| {
        data["code"] == "ACTION_FOREGROUND_LOST" && data["reason"] == "unexpected_foreground_change"
    }))
}

#[cfg(windows)]
#[test]
fn must_pass_new_window_of_a_live_child_process_is_a_related_window() {
    let child = spawn_child();
    let pid = child.0.id();
    assert!(
        wait_for_descendant(pid).contains(&pid),
        "precondition: child is live"
    );
    let before = target();
    let after = tests::click_signature(NEW_HWND, pid, "cmd.exe", "DevTools", 2);
    let mut evidence = evidence(vec![TARGET_HWND]);
    fill_related_window_evidence(&mut evidence, &before, &after);
    assert!(
        evidence.descendant_pids.contains(&pid),
        "{:?}",
        evidence.descendant_pids
    );
    let postcondition = verify(before, after, evidence).expect("child-process window is related");
    let observed = postcondition
        .related_window_observed
        .expect("related_window_observed evidence");
    assert_eq!((observed.hwnd, observed.pid), (NEW_HWND, pid));
    assert_eq!(observed.relationship, "child_process");
}

#[cfg(windows)]
#[test]
fn must_fail_new_window_of_an_unrelated_process_is_foreground_theft() {
    let _child = spawn_child();
    let before = target();
    let after = tests::click_signature(NEW_HWND, SYSTEM_PID, "System", "Other", 2);
    let mut evidence = evidence(vec![TARGET_HWND]);
    fill_related_window_evidence(&mut evidence, &before, &after);
    assert!(!evidence.descendant_pids.contains(&SYSTEM_PID));
    assert!(foreground_lost(verify(before, after, evidence)));
}

#[cfg(windows)]
#[test]
fn must_fail_pre_existing_child_window_skips_the_snapshot_and_is_refused() {
    let child = spawn_child();
    let pid = child.0.id();
    assert!(
        wait_for_descendant(pid).contains(&pid),
        "precondition: child is live"
    );
    let before = target();
    let after = tests::click_signature(NEW_HWND, pid, "cmd.exe", "DevTools", 2);
    // The window was visible before the chord: Alt+Tab, not the chord's effect.
    let mut evidence = evidence(vec![TARGET_HWND, NEW_HWND]);
    fill_related_window_evidence(&mut evidence, &before, &after);
    assert!(
        evidence.descendant_pids.is_empty(),
        "no process snapshot for an old window"
    );
    assert!(foreground_lost(verify(before, after, evidence)));
}

#[test]
fn must_pass_verify_action_delta_fills_the_evidence_through_the_seam() {
    let source = include_str!("../m2_tools.rs");
    let start = source
        .find("async fn verify_action_delta(")
        .expect("verify_action_delta present");
    let body = &source[start..];
    // m2_tools.rs may be checked out with LF or CRLF line endings.
    let end = ["\n    }\n", "\n    }\r\n"]
        .iter()
        .filter_map(|closing| body.find(closing))
        .min()
        .expect("function end");
    let body = &body[..end];
    assert!(
        body.contains("fill_related_window_evidence("),
        "verify_action_delta must use the evidence seam"
    );
}
