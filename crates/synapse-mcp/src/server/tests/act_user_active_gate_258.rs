//! #258 (frozen criteria rules 1-6): `act operation=foreground` refuses with
//! `USER_ACTIVE` when physical input is younger than the quiet window
//! (`SYNAPSE_USER_ACTIVE_QUIET_MS`, default 60000, 0 = off), with
//! `USER_ACTIVITY_UNKNOWN` when monitoring is unavailable, before any lease,
//! profile change or input; `owner_approved` + reason bypasses and is audited.
//! Background target-scoped delivery is never gated.
//! Real path: `SynapseService::act` on an isolated service; activity injected
//! per session via the cfg(test) seam `user_activity::inject_user_activity_for_test`
//! (no injection = real source). Action = claimless `focus_window`: it ends
//! `TARGET_NOT_SET` before any focus, cursor or input on every code version.
//! Rule 7 stays covered by m3/tests/live_user_input.rs.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use serde_json::{Value, json};
use synapse_core::error_codes;

use super::Parameters;
use crate::server::g4_unit_support::{error_code, seed_live_session};
use crate::server::user_active_258_support::{
    HUMAN_HWND, OWNER_REASON, USER_ACTIVE, USER_ACTIVITY_UNKNOWN,
    assert_nothing_escalated_or_delivered, assert_proceeded, call_act, final_act_row, fixture,
    foreground_focus, human_typed, inject, refusal,
};
use crate::server::user_activity::{UserActivityParams, injected_user_activity_for_test};

#[tokio::test(flavor = "current_thread")]
async fn must_fail_foreground_act_within_quiet_window_is_refused_and_audited() {
    const SESSION: &str = "g258-active";
    let fx = fixture(None);
    seed_live_session(&fx.service, SESSION);
    let _activity = human_typed(SESSION, 8_860);

    let result = call_act(&fx.service, SESSION, foreground_focus(json!({}))).await;

    let data = refusal(result, USER_ACTIVE);
    assert_eq!(data["last_physical_input_age_ms"], 8_860, "{data}");
    let window = &data["user_window"];
    assert_eq!(window["hwnd"], HUMAN_HWND, "{data}");
    assert_eq!(window["process_name"], "Code.exe", "{data}");
    assert_eq!(
        window["window_title"], "Synapse - Visual Studio Code",
        "{data}"
    );
    assert_nothing_escalated_or_delivered(&fx.service);
    // Body 4 / rule 2: the refusal is visible in the act audit row.
    let row = final_act_row(&fx.service);
    assert_eq!(row["outcome"], "error", "{row}");
    assert_eq!(row["error_code"], USER_ACTIVE, "{row}");
    let audited = &row["error"]["data"];
    assert_eq!(audited["user_window"]["hwnd"], HUMAN_HWND, "{row}");
    assert_eq!(audited["last_physical_input_age_ms"], 8_860, "{row}");
    assert_eq!(
        audited["delivery_state"], "refused_before_delivery",
        "{row}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_one_ms_inside_the_default_quiet_window_is_refused() {
    const SESSION: &str = "g258-boundary-inside";
    let fx = fixture(None);
    seed_live_session(&fx.service, SESSION);
    let _activity = human_typed(SESSION, 59_999);

    let result = call_act(&fx.service, SESSION, foreground_focus(json!({}))).await;

    let data = refusal(result, USER_ACTIVE);
    assert_eq!(data["last_physical_input_age_ms"], 59_999, "{data}");
    assert_nothing_escalated_or_delivered(&fx.service);
}

#[tokio::test(flavor = "current_thread")]
async fn must_pass_default_quiet_window_elapsed_proceeds() {
    const SESSION: &str = "g258-boundary-elapsed";
    let fx = fixture(None);
    seed_live_session(&fx.service, SESSION);
    let _activity = human_typed(SESSION, 60_000);

    let result = call_act(&fx.service, SESSION, foreground_focus(json!({}))).await;

    assert_proceeded(&fx.service, result);
}

#[tokio::test(flavor = "current_thread")]
async fn must_pass_no_physical_input_since_monitor_start_proceeds() {
    const SESSION: &str = "g258-never-used";
    let fx = fixture(None);
    seed_live_session(&fx.service, SESSION);
    let _activity = inject(SESSION, true, None, None);

    let result = call_act(&fx.service, SESSION, foreground_focus(json!({}))).await;

    assert_proceeded(&fx.service, result);
}

#[tokio::test(flavor = "current_thread")]
async fn must_pass_quiet_window_is_read_from_config() {
    const INSIDE: &str = "g258-config-inside";
    const OUTSIDE: &str = "g258-config-outside";
    let fx = fixture(Some("5000"));
    seed_live_session(&fx.service, INSIDE);
    seed_live_session(&fx.service, OUTSIDE);
    {
        let _activity = human_typed(INSIDE, 4_999);
        let result = call_act(&fx.service, INSIDE, foreground_focus(json!({}))).await;
        let data = refusal(result, USER_ACTIVE);
        assert_eq!(data["last_physical_input_age_ms"], 4_999, "{data}");
        assert_nothing_escalated_or_delivered(&fx.service);
    }
    let _activity = human_typed(OUTSIDE, 5_000);

    let result = call_act(&fx.service, OUTSIDE, foreground_focus(json!({}))).await;

    assert_proceeded(&fx.service, result);
}

#[tokio::test(flavor = "current_thread")]
async fn must_pass_quiet_window_zero_disables_the_gate() {
    const SESSION: &str = "g258-quiet-zero";
    let fx = fixture(Some("0"));
    seed_live_session(&fx.service, SESSION);
    let _activity = human_typed(SESSION, 0);

    let result = call_act(&fx.service, SESSION, foreground_focus(json!({}))).await;

    assert_proceeded(&fx.service, result);
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_monitoring_unavailable_is_user_activity_unknown() {
    const SESSION: &str = "g258-monitor-off";
    let fx = fixture(None);
    seed_live_session(&fx.service, SESSION);
    let _activity = inject(SESSION, false, None, None);

    let result = call_act(&fx.service, SESSION, foreground_focus(json!({}))).await;

    let _data = refusal(result, USER_ACTIVITY_UNKNOWN);
    assert_nothing_escalated_or_delivered(&fx.service);
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_without_an_activity_source_the_gate_fails_closed() {
    const SESSION: &str = "g258-no-source";
    let fx = fixture(None);
    seed_live_session(&fx.service, SESSION);
    assert!(
        injected_user_activity_for_test(SESSION).is_none(),
        "precondition: no injected reading, the real source applies"
    );
    let status = fx
        .service
        .user_activity(Parameters(UserActivityParams::default()))
        .await
        .expect("user_activity readback")
        .0;
    assert!(
        !status.physical_input_monitoring,
        "precondition: the isolated service has no input hook"
    );

    let result = call_act(&fx.service, SESSION, foreground_focus(json!({}))).await;

    let _data = refusal(result, USER_ACTIVITY_UNKNOWN);
    assert_nothing_escalated_or_delivered(&fx.service);
}

#[tokio::test(flavor = "current_thread")]
async fn must_pass_owner_approved_with_reason_bypasses_active_human_and_audits_reason() {
    const SESSION: &str = "g258-owner";
    let fx = fixture(None);
    seed_live_session(&fx.service, SESSION);
    let _activity = human_typed(SESSION, 100);
    let params = foreground_focus(json!({"owner_approved": true, "reason": OWNER_REASON}));

    let result = call_act(&fx.service, SESSION, params).await;

    assert_proceeded(&fx.service, result);
    let row = final_act_row(&fx.service).to_string();
    assert!(
        row.contains(OWNER_REASON),
        "#258: the act audit row must carry the owner's reason: {row}"
    );
    assert!(
        row.contains("\"owner_approved\":true"),
        "#258: the act audit row must record owner_approved: {row}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn must_pass_owner_approved_with_reason_bypasses_unknown_monitoring() {
    const SESSION: &str = "g258-owner-monitor-off";
    let fx = fixture(None);
    seed_live_session(&fx.service, SESSION);
    let _activity = inject(SESSION, false, None, None);
    let params = foreground_focus(json!({"owner_approved": true, "reason": OWNER_REASON}));

    let result = call_act(&fx.service, SESSION, params).await;

    assert_proceeded(&fx.service, result);
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_owner_approved_without_reason_is_refused_before_escalation() {
    let fx = fixture(None);
    for (session, reason) in [
        ("g258-owner-no-reason", Value::Null),
        ("g258-owner-blank-reason", json!("   ")),
    ] {
        seed_live_session(&fx.service, session);
        // Idle human: only the missing reason can refuse this call.
        let _activity = inject(session, true, None, None);
        let params = foreground_focus(json!({"owner_approved": true, "reason": reason}));

        let error = call_act(&fx.service, session, params)
            .await
            .expect_err("#258: owner_approved without a reason must be refused");

        assert_eq!(
            error_code(&error).as_deref(),
            Some(error_codes::TOOL_PARAMS_INVALID),
            "{session}: {error:?}"
        );
        assert_nothing_escalated_or_delivered(&fx.service);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_owner_approved_false_is_not_a_bypass() {
    const SESSION: &str = "g258-owner-false";
    let fx = fixture(None);
    seed_live_session(&fx.service, SESSION);
    let _activity = human_typed(SESSION, 100);
    let params = foreground_focus(json!({"owner_approved": false}));

    let result = call_act(&fx.service, SESSION, params).await;

    let _data = refusal(result, USER_ACTIVE);
    assert_nothing_escalated_or_delivered(&fx.service);
}

/// Must NOT change: background SetWindowPos (no activation) on the session's
/// claimed, never-activated window stays allowed while the human is typing.
#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn must_pass_background_delivery_to_claimed_window_is_not_gated() {
    const SESSION: &str = "g258-background";
    let fx = fixture(None);
    let window = crate::test_support::owned_claim_window().expect("own non-activating window");
    seed_live_session(&fx.service, SESSION);
    crate::server::g4_unit_support::claim_window(&fx.service, SESSION, window.hwnd());
    crate::server::g4_unit_support::set_window_target(&fx.service, SESSION, window.hwnd());
    let _activity = human_typed(SESSION, 0);
    let params = json!({
        "operation": "invoke",
        "action": {"verb": "set_window_bounds", "width": 400, "height": 300},
    });

    let response = call_act(&fx.service, SESSION, params)
        .await
        .expect("#258: background delivery must not be gated");

    let action = &response["action"];
    assert_eq!(action["ok"], true, "{response}");
    assert_eq!(action["result"]["target_hwnd"], window.hwnd(), "{response}");
    assert_eq!(action["result"]["actual"]["width"], 400, "{response}");
    for code in [USER_ACTIVE, USER_ACTIVITY_UNKNOWN] {
        assert!(!response.to_string().contains(code), "{response}");
    }
}
