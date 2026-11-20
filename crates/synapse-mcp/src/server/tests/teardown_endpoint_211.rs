//! Wire as CHILD of existing tests/cdp_close_absent_211.rs to reuse fake CDP.
//! No production include or duplicated CDP implementation. EXPECT-GREEN.
#![cfg(windows)]
use super::*;
use crate::server::g7a_support as f;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_pass_teardown_reclaims_absent_raw_target_and_is_idempotent() {
    let _lock = crate::test_support::lease_serial("g7a211");
    let (fake, endpoint) = spawn_fake_cdp(&["OTHER1", "OTHER2"], false)
        .await
        .expect("fake");
    let dir = TempDir::new().expect("temp");
    let svc = service_with_temp_db(dir.path()).expect("service");
    claimed_session(&svc, "g7a211-absent", &endpoint).expect("seed");
    assert_eq!(owner_rows(&svc).expect("before owner rows"), 1);
    assert_eq!(fake.targets(), vec!["OTHER1", "OTHER2"]);
    let lifecycle = svc.session_lifecycle_state().expect("lifecycle");
    let report = lifecycle
        .teardown_session("g7a211-absent", "g7a211")
        .await
        .expect("teardown");
    assert_eq!(report.failure_count, 0);
    assert_eq!(report.cdp.already_absent, 1);
    assert_eq!(report.cdp.persisted_rows_deleted, 1);
    assert_eq!(owner_rows(&svc).expect("owner rows"), 0);
    assert_eq!(claims_held_by(&svc, "g7a211-absent").expect("claims"), 0);
    let again = lifecycle
        .teardown_session("g7a211-absent", "g7a211")
        .await
        .expect("repeat");
    assert_eq!(again.cdp.owned_before, 0);
    assert_eq!(fake.close_calls(), 0);
    assert_eq!(fake.targets(), vec!["OTHER1", "OTHER2"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_fail_teardown_protocol_failure_retains_durable_owner_for_retry() {
    let _lock = crate::test_support::lease_serial("g7a211");
    let (fake, endpoint) = spawn_fake_cdp(&[TARGET, "OTHER1"], true)
        .await
        .expect("fake");
    let dir = TempDir::new().expect("temp");
    let svc = service_with_temp_db(dir.path()).expect("service");
    claimed_session(&svc, "g7a211-error", &endpoint).expect("seed");
    let before = svc
        .read_persisted_cdp_target_owners_for_target_id(TARGET)
        .expect("rows");
    assert_eq!(before.len(), 1);
    let before = serde_json::to_value(before).expect("owner bytes");
    let error = svc
        .session_lifecycle_state()
        .expect("lifecycle")
        .teardown_session("g7a211-error", "g7a211")
        .await
        .expect_err("protocol failure must surface");
    assert_eq!(
        f::code(&error),
        synapse_core::error_codes::TOOL_INTERNAL_ERROR
    );
    assert_eq!(
        error.data.as_ref().expect("data")["report"]["cdp"]["failed"],
        1
    );
    let after = svc
        .read_persisted_cdp_target_owners_for_target_id(TARGET)
        .expect("rows");
    assert_eq!(serde_json::to_value(after).expect("owner bytes"), before);
    assert_eq!(fake.targets(), vec![TARGET, "OTHER1"]);
    assert_eq!(fake.close_calls(), 1);
}
