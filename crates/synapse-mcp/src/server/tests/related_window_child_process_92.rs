//! #92 regression: a window the action itself opens in a CHILD PROCESS of the
//! target (Tauri/WebView2 DevTools: unowned, GW_OWNER=0, `msedgewebview2.exe`)
//! is a related window, not a foreground theft. Unrelated processes, windows
//! that already existed, and shell-host roots are still refused.
use super::*;

const TARGET_PID: u32 = 10;
const DEVTOOLS_PID: u32 = 20;

fn verify_with(
    before_process: &str,
    after_pid: u32,
    prior_windows: Vec<i64>,
    descendant_pids: Vec<u32>,
) -> Result<ActPostcondition, ErrorData> {
    let before = tests::click_signature(100, TARGET_PID, before_process, "Finance", 1);
    let after = tests::click_signature(200, after_pid, "msedgewebview2.exe", "DevTools", 2);
    let mut policy = ForegroundChangePolicy::reject();
    policy.related_window = Some(RelatedWindowEvidence {
        prior_windows,
        // The DevTools window is unowned: the owner chain is empty.
        owner_chain: Vec::new(),
        descendant_pids,
    });
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

fn assert_foreground_lost(result: Result<ActPostcondition, ErrorData>) {
    let Err(error) = result else {
        panic!("foreground theft must fail")
    };
    let Some(data) = error.data else {
        panic!("structured error required")
    };
    assert_eq!(data["code"], "ACTION_FOREGROUND_LOST");
    assert_eq!(data["reason"], "unexpected_foreground_change");
}

#[test]
fn must_pass_new_unowned_window_in_a_child_process_is_related() -> Result<(), ErrorData> {
    let result = verify_with("finance.exe", DEVTOOLS_PID, vec![100], vec![DEVTOOLS_PID])?;
    assert_eq!(result.status, "observed_delta");
    let observed = result
        .related_window_observed
        .expect("child-process window must carry related_window_observed evidence");
    assert_eq!(observed.hwnd, 200);
    assert_eq!(observed.pid, DEVTOOLS_PID);
    assert_eq!(observed.target_hwnd, 100);
    assert_eq!(observed.relationship, "child_process");
    Ok(())
}

#[test]
fn must_fail_foreground_moving_to_an_unrelated_process_is_still_refused() {
    // pid 30 is not in the target's process tree.
    assert_foreground_lost(verify_with(
        "finance.exe",
        30,
        vec![100],
        vec![DEVTOOLS_PID],
    ));
}

#[test]
fn must_fail_child_process_window_that_already_existed_is_still_refused() {
    // Alt+Tab to a pre-existing window of a descendant is not the chord's effect.
    assert_foreground_lost(verify_with(
        "finance.exe",
        DEVTOOLS_PID,
        vec![100, 200],
        vec![DEVTOOLS_PID],
    ));
}

#[test]
fn must_fail_empty_process_tree_never_relates_a_foreign_process() {
    assert_foreground_lost(verify_with(
        "finance.exe",
        DEVTOOLS_PID,
        vec![100],
        Vec::new(),
    ));
}

#[test]
fn must_fail_shell_host_root_does_not_adopt_its_children() {
    // explorer.exe is the parent of most user apps; its tree proves nothing.
    assert_foreground_lost(verify_with(
        "Explorer.EXE",
        DEVTOOLS_PID,
        vec![100],
        vec![DEVTOOLS_PID],
    ));
}
