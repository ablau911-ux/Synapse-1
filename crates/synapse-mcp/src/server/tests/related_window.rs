//! #92: deterministic foreground transition verification, without native windows.
use super::*;

fn verify(
    pid: u32,
    hwnd: i64,
    owners: Vec<i64>,
    prior_windows: Vec<i64>,
) -> Result<ActPostcondition, ErrorData> {
    let before = tests::click_signature(100, 10, "app.exe", "App", 1);
    let after = tests::click_signature(hwnd, pid, "app.exe", "DevTools", 2);
    let mut policy = ForegroundChangePolicy::reject();
    policy.related_window = Some(RelatedWindowEvidence {
        prior_windows,
        owner_chain: owners.into_iter().map(|hwnd| (hwnd, 10)).collect(),
        descendant_pids: Vec::new(),
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

// Live VBV 2026-10-13: Notepad++ creates its Find dialog once, keeps it hidden,
// and shows it on Ctrl+F. A hidden owned dialog becoming visible is the normal
// effect of the chord; the same-process owner chain is what excludes foreign
// windows. `prior_windows` lists the windows that were VISIBLE before the chord.
#[test]
fn must_pass_related_window_owned_same_pid_is_verified_with_identity_readback()
-> Result<(), ErrorData> {
    let result = verify(10, 200, vec![100], vec![100])?;
    assert_eq!(result.status, "observed_delta");
    assert_eq!(result.observed_delta, Some(true));
    assert_eq!(
        result.source_of_truth.as_deref(),
        Some("foreground_related_window")
    );
    let value = serde_json::to_value(&result)
        .map_err(|error| mcp_error(error_codes::TOOL_INTERNAL_ERROR, error.to_string()))?;
    let observed = &value["related_window_observed"];
    assert_eq!(observed["hwnd"], 200);
    assert_eq!(observed["pid"], 10);
    assert_eq!(observed["title"], "DevTools");
    assert_eq!(observed["target_hwnd"], 100);
    assert_eq!(observed["relationship"], "same_process_owner_chain");
    Ok(())
}

#[test]
fn must_pass_related_window_direct_owner_and_owner_chain_are_verified() -> Result<(), ErrorData> {
    // Two-link chain: 200 is owned by 150, which is owned by the prior foreground 100.
    let result = verify(10, 200, vec![150, 100], vec![100, 150])?;
    assert_eq!(result.status, "observed_delta");
    assert_eq!(result.observed_delta, Some(true));
    let observed = result
        .related_window_observed
        .ok_or_else(|| mcp_error(error_codes::TOOL_INTERNAL_ERROR, "no related window"))?;
    assert_eq!(observed.hwnd, 200);
    assert_eq!(observed.target_hwnd, 100);
    assert_eq!(observed.relationship, "same_process_owner_chain");
    Ok(())
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
fn must_fail_related_window_unrelated_process_still_fails() {
    assert_foreground_lost(verify(20, 200, vec![300], vec![100]));
}

#[test]
fn must_fail_related_window_preexisting_same_pid_window_still_fails() {
    assert_foreground_lost(verify(10, 200, vec![], vec![100, 200]));
}

#[test]
fn must_pass_related_window_unchanged_foreground_keeps_existing_delta_behavior()
-> Result<(), ErrorData> {
    let result = verify(10, 100, vec![], vec![100])?;
    assert!(result.related_window_observed.is_none());
    let before = tests::click_signature(100, 10, "app.exe", "App", 1);
    let result = verify_captured_action_delta(
        "act_press",
        "foreground_focused_ui_or_pixels",
        250,
        before.clone(),
        before,
        None,
        ForegroundChangePolicy::reject(),
    );
    let Err(error) = result else {
        panic!("no delta must remain unverified")
    };
    let Some(data) = error.data else {
        panic!("structured error required")
    };
    assert_eq!(data["code"], "ACTION_NO_OBSERVED_DELTA");
    Ok(())
}

#[test]
fn must_fail_related_window_native_unchanged_stays_unverified() -> Result<(), ErrorData> {
    let before = tests::click_signature(100, 10, "app.exe", "App", 1);
    let after = tests::click_signature(100, 10, "app.exe", "App", 2);
    let result = verify_captured_action_delta(
        "act_press",
        "foreground_related_window",
        250,
        before,
        after,
        None,
        ForegroundChangePolicy::reject(),
    )?;
    assert_eq!(result.status, "not_requested");
    assert_eq!(result.observed_delta, None);
    Ok(())
}

#[test]
fn must_fail_related_window_without_pre_action_inventory_is_not_accepted() {
    let before = tests::click_signature(100, 10, "app.exe", "App", 1);
    let after = tests::click_signature(200, 10, "app.exe", "DevTools", 2);
    assert_foreground_lost(verify_captured_action_delta(
        "act_press",
        "foreground_focused_ui_or_pixels",
        250,
        before,
        after,
        None,
        ForegroundChangePolicy::reject(),
    ));
}

#[test]
fn must_fail_related_window_does_not_bypass_explicit_title_expectation()
-> Result<(), Box<dyn std::error::Error>> {
    let before = tests::click_signature(100, 10, "app.exe", "App", 1);
    let after = tests::click_signature(200, 10, "app.exe", "DevTools", 2);
    let mut policy = ForegroundChangePolicy::reject();
    policy.allow = true;
    policy.expected_title_regex = Some(regex::Regex::new("^Other$")?);
    policy.related_window = Some(RelatedWindowEvidence {
        prior_windows: vec![100],
        owner_chain: vec![(100, 10)],
        descendant_pids: Vec::new(),
    });
    let result = verify_captured_action_delta(
        "act_press",
        "foreground_focused_ui_or_pixels",
        250,
        before,
        after,
        None,
        policy,
    );
    let Err(error) = result else {
        panic!("explicit policy must still match")
    };
    let Some(data) = error.data else {
        panic!("structured error required")
    };
    assert_eq!(data["code"], "ACTION_FOREGROUND_LOST");
    assert_eq!(data["reason"], "foreground_change_policy_mismatch");
    Ok(())
}

#[test]
fn must_fail_related_window_shared_frame_host_same_pid_is_rejected() {
    let before = tests::click_signature(100, 10, "ApplicationFrameHost.exe", "App A", 1);
    let after = tests::click_signature(200, 10, "ApplicationFrameHost.exe", "App B", 2);
    let mut policy = ForegroundChangePolicy::reject();
    policy.related_window = Some(RelatedWindowEvidence {
        prior_windows: vec![100],
        owner_chain: vec![],
        descendant_pids: Vec::new(),
    });
    assert_foreground_lost(verify_captured_action_delta(
        "act_press",
        "foreground_related_window",
        250,
        before,
        after,
        None,
        policy,
    ));
}

#[test]
fn must_fail_related_window_foreign_candidate_cannot_claim_target_as_owner() {
    assert_foreground_lost(verify(20, 200, vec![100], vec![100]));
}

#[test]
fn must_pass_related_window_evidence_is_explicit_and_does_not_claim_causality()
-> Result<(), Box<dyn std::error::Error>> {
    let result = verify(10, 200, vec![100], vec![100])?;
    let value = serde_json::to_value(result)?;
    assert_eq!(value["related_window_observed"]["hwnd"], 200);
    assert_eq!(value["related_window_observed"]["pid"], 10);
    assert_eq!(value["related_window_observed"]["title"], "DevTools");
    assert_eq!(
        value["related_window_observed"]["relationship"],
        "same_process_owner_chain"
    );
    assert_eq!(
        value["detail"],
        "act_press verify_delta related_window_observed after delivery; temporal observation does not establish that the chord created the window"
    );
    Ok(())
}

#[test]
fn must_fail_related_window_already_visible_owned_window_is_rejected() {
    assert_foreground_lost(verify(10, 200, vec![100], vec![100, 200]));
}

#[test]
fn must_fail_related_window_hidden_unowned_same_pid_window_is_rejected() {
    assert_foreground_lost(verify(10, 200, vec![], vec![100]));
}

#[test]
fn must_fail_related_window_owner_pid_must_match_at_every_link() {
    for owners in [vec![(100, 99)], vec![(150, 99), (100, 10)], vec![(100, 0)]] {
        let before = tests::click_signature(100, 10, "app.exe", "App", 1);
        let after = tests::click_signature(200, 10, "app.exe", "DevTools", 2);
        let mut policy = ForegroundChangePolicy::reject();
        policy.related_window = Some(RelatedWindowEvidence {
            prior_windows: vec![100],
            owner_chain: owners,
            descendant_pids: Vec::new(),
        });
        assert_foreground_lost(verify_captured_action_delta(
            "act_press",
            "foreground_related_window",
            250,
            before,
            after,
            None,
            policy,
        ));
    }
}

#[test]
fn must_pass_related_window_native_observation_skips_wait_but_explicit_delta_waits() {
    assert_eq!(
        action_delta_verification_delay("foreground_related_window", 250),
        Duration::ZERO
    );
    assert_eq!(
        action_delta_verification_delay("foreground_focused_ui_or_pixels", 250),
        Duration::from_millis(250)
    );
}
