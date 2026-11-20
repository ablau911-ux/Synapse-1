//! Regression tests for #190: foreground-tier policy-violation detection must
//! work on the row shapes `audit_action_result_*` really writes
//! (`{"response": ...}` for ok rows, `{"message","data"}` for error rows), not
//! only on flat detail objects.

use super::tests::service_with_db;
use super::*;
use tempfile::TempDir;

fn last_action_row(service: &SynapseService) -> Value {
    let runtime = service.reflex_runtime().expect("runtime");
    let rows = runtime
        .lock()
        .expect("lock")
        .storage_cf_tail_rows(synapse_storage::cf::CF_ACTION_LOG, 1)
        .expect("tail rows");
    let (_key, bytes) = rows.into_iter().next().expect("one audit row");
    synapse_storage::decode_json::<Value>(&bytes).expect("decode audit row")
}

#[test]
fn must_pass_ok_row_response_with_foreground_tier_is_flagged_for_normal_agent() {
    let dir = TempDir::new().expect("tmp");
    let service = service_with_db(dir.path());
    let response = json!({
        "backend_tier_used": "foreground_sendinput",
        "required_foreground": true,
        "tier_attempts": [{ "tier": "foreground_sendinput", "required_foreground": true }],
    });
    service
        .audit_action_result_for_session::<Value>(
            "act_click",
            &Ok(response),
            "issue190-normal-session",
        )
        .expect("write row");
    let row = last_action_row(&service);
    let tier = &row["foreground_tier"];
    assert_eq!(tier["required_foreground"], true, "row: {row}");
    assert_eq!(tier["backend_tier"], "foreground_sendinput", "row: {row}");
    assert_eq!(tier["foreground_policy_violation"], true, "row: {row}");
    assert_eq!(tier["allowed"], false, "row: {row}");
}

#[test]
fn must_pass_error_row_data_tier_attempts_are_read() {
    let dir = TempDir::new().expect("tmp");
    let service = service_with_db(dir.path());
    let error = ErrorData::internal_error(
        "click failed",
        Some(json!({
            "code": "ACTION_FAILED",
            "tier_attempts": [
                { "tier": "uia", "required_foreground": false },
                { "tier": "foreground_sendinput", "required_foreground": true },
            ],
        })),
    );
    service
        .audit_action_result_for_session::<Value>(
            "act_click",
            &Err(error),
            "issue190-normal-session",
        )
        .expect("write row");
    let row = last_action_row(&service);
    let tier = &row["foreground_tier"];
    assert_eq!(tier["required_foreground"], true, "row: {row}");
    assert_eq!(tier["backend_tier"], "foreground_sendinput", "row: {row}");
    assert_eq!(tier["foreground_policy_violation"], true, "row: {row}");
}

#[test]
fn must_fail_refused_foreground_attempt_is_not_a_policy_violation() {
    let dir = TempDir::new().expect("tmp");
    let service = service_with_db(dir.path());
    // The #187 gate refuses the SendInput tier before any input is sent.
    let error = ErrorData::internal_error(
        "click refused",
        Some(json!({
            "code": "ACTION_FOREGROUND_TIER_REFUSED",
            "tier_attempts": [
                { "tier": "uia", "required_foreground": false, "status": "failed" },
                { "tier": "foreground_sendinput", "required_foreground": true, "status": "failed" },
            ],
        })),
    );
    service
        .audit_action_result_for_session::<Value>(
            "act_click",
            &Err(error),
            "issue190-normal-session",
        )
        .expect("write row");
    let row = last_action_row(&service);
    let tier = &row["foreground_tier"];
    assert_eq!(tier["required_foreground"], false, "row: {row}");
    assert_eq!(tier["foreground_policy_violation"], false, "row: {row}");
    assert_eq!(tier["allowed"], true, "row: {row}");
}

#[test]
fn must_pass_delivered_foreground_attempt_is_still_flagged() {
    let dir = TempDir::new().expect("tmp");
    let service = service_with_db(dir.path());
    let error = ErrorData::internal_error(
        "click delivered but unverified",
        Some(json!({
            "code": "ACTION_FAILED",
            "tier_attempts": [
                { "tier": "foreground_sendinput", "required_foreground": true, "status": "delivered" },
            ],
        })),
    );
    service
        .audit_action_result_for_session::<Value>(
            "act_click",
            &Err(error),
            "issue190-normal-session",
        )
        .expect("write row");
    let row = last_action_row(&service);
    assert_eq!(
        row["foreground_tier"]["foreground_policy_violation"], true,
        "row: {row}"
    );
}

#[test]
fn must_fail_background_response_is_not_flagged() {
    let dir = TempDir::new().expect("tmp");
    let service = service_with_db(dir.path());
    let response = json!({
        "backend_tier_used": "postmessage",
        "required_foreground": false,
    });
    service
        .audit_action_result_for_session::<Value>(
            "act_click",
            &Ok(response),
            "issue190-normal-session",
        )
        .expect("write row");
    let row = last_action_row(&service);
    let tier = &row["foreground_tier"];
    assert_eq!(tier["required_foreground"], false, "row: {row}");
    assert_eq!(tier["backend_tier"], "postmessage", "row: {row}");
    assert_eq!(tier["foreground_policy_violation"], false, "row: {row}");
    assert_eq!(tier["allowed"], true, "row: {row}");
}

#[test]
fn must_fail_request_details_do_not_count_as_foreground_proof() {
    // A denied/error row nests the *request* under "request"; a request that
    // merely asks for the foreground tier must not be read as the tier used.
    let details =
        json!({ "request": { "required_foreground": true, "tier": "foreground_sendinput" } });
    assert!(!detail_required_foreground(&details));
    assert_eq!(detail_backend_tier(&details), None);

    // The real write path: an error row nesting that request must not be
    // flagged as foreground use.
    let dir = TempDir::new().expect("tmp");
    let service = service_with_db(dir.path());
    let error = ErrorData::internal_error(
        "click refused before delivery",
        Some(json!({ "code": "ACTION_FAILED" })),
    );
    service
        .audit_action_error_with_details_for_session(
            "act_click",
            &error,
            &json!({ "required_foreground": true, "tier": "foreground_sendinput" }),
            "issue190-normal-session",
        )
        .expect("write row");
    let row = last_action_row(&service);
    assert_eq!(
        row["details"]["request"]["required_foreground"], true,
        "the request must be echoed in the row: {row}"
    );
    let tier = &row["foreground_tier"];
    assert_eq!(tier["required_foreground"], false, "row: {row}");
    assert_eq!(tier["backend_tier"], Value::Null, "row: {row}");
    assert_eq!(tier["foreground_policy_violation"], false, "row: {row}");
}

#[test]
fn must_fail_claimed_target_route_click_is_not_a_policy_violation() {
    let dir = TempDir::new().expect("tmp");
    let service = service_with_db(dir.path());
    // #187 sanctions SendInput on a window this session claimed and targets.
    let response = json!({
        "backend_tier_used": "foreground",
        "required_foreground": true,
        "tier_attempts": [{
            "tier": "foreground",
            "required_foreground": true,
            "status": "delivered",
            "foreground_authorization": "claimed_target",
        }],
    });
    service
        .audit_action_result_for_session::<Value>(
            "act_click",
            &Ok(response),
            "issue190-normal-session",
        )
        .expect("write row");
    let row = last_action_row(&service);
    let tier = &row["foreground_tier"];
    assert_eq!(tier["required_foreground"], true, "row: {row}");
    assert_eq!(tier["claimed_target_route"], true, "row: {row}");
    assert_eq!(tier["foreground_policy_violation"], false, "row: {row}");
}

#[test]
fn must_pass_an_unsanctioned_attempt_beside_a_claimed_one_is_still_flagged() {
    let details = json!({
        "tier_attempts": [
            { "tier": "foreground", "required_foreground": true, "status": "delivered",
              "foreground_authorization": "claimed_target" },
            { "tier": "foreground", "required_foreground": true, "status": "delivered" },
        ],
    });
    assert!(detail_required_foreground(&details));
    assert!(!detail_foreground_claimed_target_only(&details));

    // The real write path: the row written for that response is flagged.
    let dir = TempDir::new().expect("tmp");
    let service = service_with_db(dir.path());
    service
        .audit_action_result_for_session::<Value>(
            "act_click",
            &Ok(details),
            "issue190-normal-session",
        )
        .expect("write row");
    let row = last_action_row(&service);
    let tier = &row["foreground_tier"];
    assert_eq!(tier["required_foreground"], true, "row: {row}");
    assert_eq!(tier["claimed_target_route"], false, "row: {row}");
    assert_eq!(tier["foreground_policy_violation"], true, "row: {row}");
    assert_eq!(tier["allowed"], false, "row: {row}");
}

/// A session whose tool profile allows the real foreground tier (here the
/// trusted local-model agent's default `full_capability`) records a
/// foreground-tier action as allowed, not as a policy violation.
#[test]
fn must_pass_foreground_capable_session_foreground_row_is_allowed() {
    use rmcp::{
        model::{ClientCapabilities, Implementation, InitializeRequestParams},
        transport::streamable_http_server::session::SessionState,
    };
    let dir = TempDir::new().expect("tmp");
    let service = service_with_db(dir.path());
    let session_id = "issue190-foreground-capable-session";
    let state = SessionState::new(InitializeRequestParams::new(
        ClientCapabilities::default(),
        Implementation::new("synapse-local-model-agent", "0.0.0-test"),
    ));
    {
        let mut registry = service.session_registry_ref().lock().expect("registry");
        registry.record_initialized(session_id, &state, "http", 1);
        registry.trust_local_agent(session_id);
    }
    let response = json!({
        "backend_tier_used": "foreground_sendinput",
        "required_foreground": true,
        "tier_attempts": [{ "tier": "foreground_sendinput", "required_foreground": true }],
    });
    service
        .audit_action_result_for_session::<Value>("act_click", &Ok(response), session_id)
        .expect("write row");
    let row = last_action_row(&service);
    let tier = &row["foreground_tier"];
    assert_eq!(tier["required_foreground"], true, "row: {row}");
    assert_eq!(
        tier["session_foreground_policy"], "full_capability",
        "row: {row}"
    );
    assert_eq!(tier["policy_allows_foreground"], true, "row: {row}");
    assert_eq!(tier["foreground_policy_violation"], false, "row: {row}");
    assert_eq!(tier["allowed"], true, "row: {row}");
}
