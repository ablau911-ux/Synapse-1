//! #258 round 2 (criteria addendum r2, Google rule): every gated inner route
//! refuses `USER_ACTIVE` while the human is active, before any lease, started
//! row, spawn or input. Each test fails if that route's gate call is removed:
//! the shared tool foreground lease helper (focus_window/type/press/scroll/...),
//! claimed typing, the click SendInput tier (policy wiring + delivery check),
//! act_combo and the visible GUI launch. Plus the must-not-change line: a launch
//! on a session-owned agent desktop is not gated.
//! Safety if a gate is removed: the helpers only take the lease (no input), the
//! combo uses the fail-closed `hardware` backend with the unbound F24 key, and
//! the isolated service has no `--allow-launch` policy, so nothing is spawned.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use rmcp::ErrorData;
use serde_json::{Value, json};
use synapse_core::Point;

use super::{ActClickParams, ActTypeParams, Parameters, acquire_tool_foreground_input_lease};
use crate::m4::ActComboParams;
use crate::server::SynapseService;
use crate::server::g4_unit_support::{
    SYNTHETIC_NOTEPAD_HWND, claim_window, error_code, seed_live_session, set_window_target,
    use_synthetic_notepad_foreground,
};
use crate::server::g7a_support::{audit_rows, db};
use crate::server::tool_profiles::ToolProfileKind;
use crate::server::user_active_258_support::{
    USER_ACTIVE, USER_ACTIVITY_UNKNOWN, call_process, final_rows, fixture, human_typed, inject,
    launch_refusal, notepad_launch,
};

const CLICK_POINT: Point = Point { x: 40, y: 40 };

/// The session may use the raw foreground tier, so only #258 can refuse it.
fn break_glass(service: &SynapseService, session: &str) {
    service
        .set_tool_profile_for_test(session, ToolProfileKind::BreakGlass)
        .expect("break_glass profile row");
    assert_eq!(
        service
            .tool_profile_snapshot(Some(session))
            .expect("profile readback")
            .profile,
        ToolProfileKind::BreakGlass,
        "precondition: break_glass"
    );
}

/// The route refused with `USER_ACTIVE` before delivery and took no lease.
fn assert_user_active_before_lease<T>(result: Result<T, ErrorData>, route: &str) {
    let Err(error) = result else {
        panic!("#258: {route} must refuse while the human is active (is its gate call gone?)");
    };
    assert_eq!(
        error_code(&error).as_deref(),
        Some(USER_ACTIVE),
        "{route}: {error:?}"
    );
    let data = error.data.unwrap_or(Value::Null);
    assert_eq!(data["delivery_state"], "refused_before_delivery", "{data}");
    assert!(
        !synapse_action::lease::status().held,
        "#258: {route} must refuse before taking the input lease"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_tool_foreground_lease_helper_refuses_an_active_human_before_the_lease() {
    const SESSION: &str = "g258r2-lease-helper";
    let fx = fixture(None);
    seed_live_session(&fx.service, SESSION);
    break_glass(&fx.service, SESSION);
    let _activity = human_typed(SESSION, 100);
    let context = crate::test_support::request_context_for_session(SESSION).await;

    let result = acquire_tool_foreground_input_lease(&fx.service, "act_focus_window", &context);

    assert_user_active_before_lease(result, "tool foreground lease helper");
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_claimed_typing_refuses_an_active_human_before_the_lease() {
    const SESSION: &str = "g258r2-claimed-type";
    let fx = fixture(None);
    seed_live_session(&fx.service, SESSION);
    use_synthetic_notepad_foreground(&fx.service);
    set_window_target(&fx.service, SESSION, SYNTHETIC_NOTEPAD_HWND);
    claim_window(&fx.service, SESSION, SYNTHETIC_NOTEPAD_HWND);
    let params: ActTypeParams = serde_json::from_value(json!({
        "text": "g258",
        "verify_target_window_hwnd": SYNTHETIC_NOTEPAD_HWND,
    }))
    .expect("act_type params");
    fx.service
        .ensure_claimed_input_target_for_session(
            "act_type",
            params.verify_target_window_hwnd,
            Some(SESSION),
        )
        .expect("precondition: the claimed typing target is admitted");
    let _activity = human_typed(SESSION, 100);
    let context = crate::test_support::request_context_for_session(SESSION).await;

    let result = fx.service.acquire_claimed_input_lease(
        "act_type",
        params.verify_target_window_hwnd,
        synapse_action::DEFAULT_LEASE_TTL_MS,
        &context,
    );

    assert_user_active_before_lease(result, "claimed typing");
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_click_sendinput_tier_refuses_an_active_human() {
    const SESSION: &str = "g258r2-click";
    let fx = fixture(None);
    seed_live_session(&fx.service, SESSION);
    break_glass(&fx.service, SESSION);
    let params: ActClickParams =
        serde_json::from_value(json!({"target": {"x": CLICK_POINT.x, "y": CLICK_POINT.y}}))
            .expect("act_click params");
    let context = crate::test_support::request_context_for_session(SESSION).await;
    {
        let _idle = inject(SESSION, true, None, None);
        let policy = fx
            .service
            .foreground_click_policy_for_request(&params, &context)
            .expect("click policy");
        policy
            .authorize_delivery_for_test(Some(CLICK_POINT))
            .expect("precondition: idle human, break_glass: the SendInput tier is open");
    }
    let _activity = human_typed(SESSION, 100);
    let policy = fx
        .service
        .foreground_click_policy_for_request(&params, &context)
        .expect("click policy");

    let result = policy.authorize_delivery_for_test(Some(CLICK_POINT));

    assert_user_active_before_lease(result, "click SendInput tier");
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_act_combo_refuses_an_active_human_before_the_started_row() {
    const SESSION: &str = "g258r2-combo";
    let fx = fixture(None);
    seed_live_session(&fx.service, SESSION);
    use_synthetic_notepad_foreground(&fx.service);
    let _activity = human_typed(SESSION, 100);
    let params: ActComboParams = serde_json::from_value(json!({
        "backend": "hardware",
        "steps": [{
            "at_ms": 0,
            "action": "act_press",
            "params": {"keys": ["f24"], "backend": "hardware"},
        }],
    }))
    .expect("act_combo params");
    let context = crate::test_support::request_context_for_session(SESSION).await;

    let result = fx.service.act_combo(Parameters(params), context).await;

    assert_user_active_before_lease(result, "act_combo");
    let rows: Vec<Value> = audit_rows(&fx.service)
        .into_iter()
        .filter(|row| row["tool"] == "act_combo")
        .collect();
    assert!(
        rows.iter()
            .any(|row| row["status"] == "denied" && row["error_code"] == USER_ACTIVE),
        "#258: the combo refusal is audited: {rows:?}"
    );
    assert!(
        rows.iter().all(|row| row["status"] != "started"),
        "#258: refused before the started row: {rows:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_visible_gui_launch_refuses_an_active_human_before_spawn() {
    const SESSION: &str = "g258r2-launch";
    let fx = fixture(None);
    seed_live_session(&fx.service, SESSION);
    use_synthetic_notepad_foreground(&fx.service);
    let _activity = human_typed(SESSION, 100);

    let result = call_process(&fx.service, SESSION, notepad_launch(json!({}))).await;

    // Without the gate the missing --allow-launch policy would answer instead.
    let _refused = launch_refusal(result, USER_ACTIVE);
    let finals = final_rows(&fx.service, "act_launch");
    assert_eq!(finals.len(), 1, "{finals:?}");
    assert_eq!(finals[0]["error_code"], USER_ACTIVE, "{finals:?}");
    let history = db(&fx.service)
        .scan_cf(synapse_storage::cf::CF_PROCESS_HISTORY)
        .expect("CF_PROCESS_HISTORY");
    assert!(history.is_empty(), "#258: nothing was spawned");
}

#[tokio::test(flavor = "current_thread")]
async fn must_pass_agent_desktop_launch_is_not_gated_while_human_active() {
    const SESSION: &str = "g258r2-agent-desktop";
    let fx = fixture(None);
    seed_live_session(&fx.service, SESSION);
    use_synthetic_notepad_foreground(&fx.service);
    let _activity = human_typed(SESSION, 100);
    let params = notepad_launch(json!({"desktop": "agent:session"}));

    let result = call_process(&fx.service, SESSION, params).await;

    // Not gated: a later launch check (tabbed-app rule or launch policy)
    // answers instead of the user-activity gate, and nothing is spawned.
    let Err(error) = result else {
        panic!("precondition: the isolated service cannot launch (no allow-launch policy)");
    };
    let code = error_code(&error);
    for gate_code in [USER_ACTIVE, USER_ACTIVITY_UNKNOWN] {
        assert_ne!(
            code.as_deref(),
            Some(gate_code),
            "#258: an agent-desktop launch must not be gated: {error:?}"
        );
    }
    let history = db(&fx.service)
        .scan_cf(synapse_storage::cf::CF_PROCESS_HISTORY)
        .expect("CF_PROCESS_HISTORY");
    assert!(history.is_empty(), "nothing was spawned");
}
