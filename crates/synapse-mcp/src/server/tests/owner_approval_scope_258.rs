//! #258 round 2 (criteria addendum r2 D1-D3): the owner override is per call.
//! D1: a visible GUI launch takes its own `owner_approved` + reason, bypasses
//! only for that call and is audited; its refusal names that override.
//! D2: an approved `act operation=foreground` never lends its approval to a
//! concurrent or later call of the same session.
//! D3: an inner-route `USER_ACTIVE` refusal surfaces as status=refused,
//! delivery_state=refused_before_delivery. Plus the approved call's own inner
//! gate is passed (approval pass-through).
//! Launches use the isolated service, which has no `--allow-launch` policy, so
//! nothing is ever spawned; the gate is proven by which refusal comes back.
//! Focus routes target an owned off-screen window whose activation the m2 test
//! seam forbids, so no variant (fixed, r1 or mutant) can take the owner's
//! foreground. The lease transition gate pauses an escalation mid-flight.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use serde_json::{Value, json};
use synapse_core::error_codes;

use super::SynapseService;
use crate::server::g4_unit_support::{seed_live_session, use_synthetic_notepad_foreground};
use crate::server::user_active_258_support::{
    OWNER_REASON, USER_ACTIVE, assert_proceeded, call_process, final_rows, fixture,
    foreground_focus, human_typed, inject, launch_refusal, notepad_launch, spawn_act,
    wait_for_escalation,
};

const LAUNCH_REASON: &str = "owner asked: open Notepad now while he keeps typing";
const APPROVED: &str = "\"owner_approved\":true";

/// Final `act_launch` command-audit rows as JSON text, oldest first.
fn launch_rows(service: &SynapseService) -> Vec<String> {
    final_rows(service, "act_launch")
        .iter()
        .map(Value::to_string)
        .collect()
}

/// Live session whose target is a claimed, real, never-activated window. Its
/// activation is forbidden through the m2 test seam, so the focus route runs
/// past every gate but can never move the owner's foreground. The synthetic
/// foreground keeps the action preflight deterministic.
#[cfg(windows)]
fn claimed_unfocusable_window(
    service: &SynapseService,
    session: &str,
) -> (
    crate::test_support::OwnedClaimWindow,
    crate::m2::ForbiddenActivationGuard,
) {
    let window = crate::test_support::owned_claim_window().expect("own non-activating window");
    let forbidden = crate::m2::forbid_activation_for_test(window.hwnd());
    seed_live_session(service, session);
    use_synthetic_notepad_foreground(service);
    crate::server::g4_unit_support::claim_window(service, session, window.hwnd());
    crate::server::g4_unit_support::set_window_target(service, session, window.hwnd());
    (window, forbidden)
}

fn launch_session(service: &SynapseService, session: &str) {
    seed_live_session(service, session);
    use_synthetic_notepad_foreground(service);
}

#[tokio::test(flavor = "current_thread")]
async fn must_pass_owner_approved_launch_bypasses_active_human_and_is_audited() {
    const SESSION: &str = "g258r2-launch-owner";
    let fx = fixture(None);
    launch_session(&fx.service, SESSION);
    let _activity = human_typed(SESSION, 100);
    let approved = notepad_launch(json!({"owner_approved": true, "reason": LAUNCH_REASON}));

    let result = call_process(&fx.service, SESSION, approved).await;

    // Past the gate: only the absent --allow-launch policy stops it (no spawn).
    let _denied = launch_refusal(result, error_codes::SAFETY_LAUNCH_DENIED_BY_POLICY);
    let rows = launch_rows(&fx.service);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(
        rows[0].contains(LAUNCH_REASON),
        "#258 D1: reason audited: {}",
        rows[0]
    );
    assert!(
        rows[0].contains(APPROVED),
        "#258 D1: approval audited: {}",
        rows[0]
    );
    // Per call: the next launch without its own approval is gated again.
    let next = call_process(&fx.service, SESSION, notepad_launch(json!({}))).await;
    let _refused = launch_refusal(next, USER_ACTIVE);
    let rows = launch_rows(&fx.service);
    assert!(
        !rows[1].contains(APPROVED),
        "#258 D1: no carry-over: {}",
        rows[1]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn must_pass_owner_approved_launch_bypasses_unknown_monitoring() {
    const SESSION: &str = "g258r2-launch-owner-unknown";
    let fx = fixture(None);
    launch_session(&fx.service, SESSION);
    let _activity = inject(SESSION, false, None, None);
    let approved = notepad_launch(json!({"owner_approved": true, "reason": LAUNCH_REASON}));

    let result = call_process(&fx.service, SESSION, approved).await;

    let _denied = launch_refusal(result, error_codes::SAFETY_LAUNCH_DENIED_BY_POLICY);
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_owner_approved_launch_without_reason_is_refused_before_spawn() {
    let fx = fixture(None);
    for (session, reason) in [
        ("g258r2-launch-no-reason", Value::Null),
        ("g258r2-launch-blank-reason", json!("   ")),
    ] {
        launch_session(&fx.service, session);
        // Idle human: only the missing reason can refuse this call.
        let _activity = inject(session, true, None, None);
        let params = notepad_launch(json!({"owner_approved": true, "reason": reason}));

        let result = call_process(&fx.service, session, params).await;

        let _invalid = launch_refusal(result, error_codes::TOOL_PARAMS_INVALID);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_launch_refusal_names_the_launch_override() {
    const SESSION: &str = "g258r2-launch-recovery";
    let fx = fixture(None);
    launch_session(&fx.service, SESSION);
    let _activity = human_typed(SESSION, 100);

    let result = call_process(&fx.service, SESSION, notepad_launch(json!({}))).await;

    let error = launch_refusal(result, USER_ACTIVE);
    let message = error.message.to_string();
    assert!(message.contains("owner_approved"), "{message}");
    assert!(
        !message.contains("call act operation=foreground with owner_approved"),
        "#258 D1: a launch refusal must name the launch's own override: {message}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_unapproved_launch_during_an_approved_escalation_is_refused() {
    const SESSION: &str = "g258r2-concurrent";
    let fx = fixture(None);
    launch_session(&fx.service, SESSION);
    let _activity = human_typed(SESSION, 100);
    // Hold the lease transition gate: the approved escalation stops inside its
    // own task, after its entry gate and slot claim, before its lease.
    let transition = fx.service.lease_transition_gate();
    let held = transition.lock().await;
    let approved_params = foreground_focus(json!({"owner_approved": true, "reason": OWNER_REASON}));
    let approved = spawn_act(&fx.service, SESSION, approved_params);
    wait_for_escalation(SESSION).await;

    let concurrent = call_process(&fx.service, SESSION, notepad_launch(json!({}))).await;

    let _refused = launch_refusal(concurrent, USER_ACTIVE);
    drop(held);
    assert_proceeded(
        &fx.service,
        approved.await.expect("approved escalation task"),
    );
    let later = call_process(&fx.service, SESSION, notepad_launch(json!({}))).await;
    let _refused = launch_refusal(later, USER_ACTIVE);
    let rows = launch_rows(&fx.service);
    assert_eq!(rows.len(), 2, "{rows:?}");
    for row in rows {
        assert!(
            !row.contains(APPROVED),
            "#258 D2: unapproved call row: {row}"
        );
    }
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn must_fail_inner_user_active_refusal_surfaces_as_refused_before_delivery() {
    const SESSION: &str = "g258r2-inner-refusal";
    let fx = fixture(None);
    let (_window, _forbidden) = claimed_unfocusable_window(&fx.service, SESSION);
    let idle = inject(SESSION, true, None, None);
    let transition = fx.service.lease_transition_gate();
    let held = transition.lock().await;
    let call = spawn_act(&fx.service, SESSION, foreground_focus(json!({})));
    wait_for_escalation(SESSION).await;
    // The human starts typing after the entry gate admitted the escalation.
    drop(idle);
    let _activity = human_typed(SESSION, 100);
    drop(held);

    let response = call
        .await
        .expect("escalation task")
        .expect("act returns the delegated outcome");

    let action = &response["action"];
    assert_eq!(
        action["result"]["error"]["code"], USER_ACTIVE,
        "precondition: the inner lease-helper gate refused: {response}"
    );
    assert_eq!(action["ok"], false, "{response}");
    assert_eq!(action["status"], "refused", "#258 D3: {response}");
    assert_eq!(
        action["delivery_state"], "refused_before_delivery",
        "#258 D3: {response}"
    );
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn must_pass_owner_approval_reaches_the_approved_calls_inner_gate() {
    const SESSION: &str = "g258r2-pass-through";
    let fx = fixture(None);
    let (_window, _forbidden) = claimed_unfocusable_window(&fx.service, SESSION);
    let _activity = human_typed(SESSION, 100);
    let params = foreground_focus(json!({"owner_approved": true, "reason": OWNER_REASON}));

    let response = crate::server::user_active_258_support::call_act(&fx.service, SESSION, params)
        .await
        .expect("the approved act runs");

    // Past the inner lease-helper gate: only the activation seam stopped it.
    let error = &response["action"]["result"]["error"];
    assert_eq!(
        error["code"],
        error_codes::ACTION_TARGET_INVALID,
        "#258: the approval must reach this call's inner gate: {response}"
    );
    let message = error["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(crate::m2::FORBIDDEN_ACTIVATION_MESSAGE),
        "#258: the focus step itself must have run: {response}"
    );
}
