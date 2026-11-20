//! #94. Parent: server. Drives dashboard handler and actual K2 entry point.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use crate::server::g7a_support as f;
use synapse_action::lease;

fn held(svc: &crate::server::SynapseService) -> std::sync::Arc<synapse_storage::Db> {
    let acquired = lease::try_acquire("g7a94-owner", lease::ttl_from_ms(600_000));
    let lease::LeaseOutcome::Acquired(status) = acquired else {
        panic!("acquire: {acquired:?}")
    };
    svc.persist_session_lease_with_acquisition_event("g7a94-owner", &status)
        .expect("persist");
    assert_eq!(
        lease::status().owner_session_id.as_deref(),
        Some("g7a94-owner")
    );
    let db = f::db(svc);
    assert_eq!(
        crate::test_support::lease_released_reasons(&db, "g7a94-owner").expect("releases"),
        Vec::<String>::new()
    );
    db
}

#[tokio::test(flavor = "current_thread")]
async fn must_pass_k2_entrypoint_journals_displaced_owner_once() {
    let _fleet = crate::test_support::REAL_PROCESS_LOCK.lock().await;
    let _lock = crate::test_support::lease_serial("g7a94");
    let (_dir, svc) = f::service();
    let db = held(&svc);
    let before = lease::status();
    let prior = lease::force_preempt("operator_hotkey");
    assert_eq!(
        prior.as_ref().and_then(|s| s.owner_session_id.as_deref()),
        Some("g7a94-owner")
    );
    assert_eq!(
        lease::status().owner_session_id.as_deref(),
        Some(synapse_action::OPERATOR_LEASE_OWNER_SESSION_ID)
    );
    let immediate = crate::safety::OperatorHotkeyImmediateReport {
        hotkey: "synthetic-owned-K1",
        lease_before: before,
        preempted_lease: prior,
        lease_after_preempt: lease::status(),
        disable_report: crate::safety::DisableReport {
            result: "ok",
            disabled_ids: vec![],
            error_code: None,
            detail: None,
        },
        release_all_report: crate::safety::ReleaseAllReport {
            result: "ok",
            error_code: None,
            detail: None,
        },
        elapsed_ms: 0,
        within_budget: true,
    };
    for _ in 0..2 {
        let response = svc
            .operator_panic_kill_all(immediate.clone())
            .await
            .expect("K2 handler");
        // The process-wide tracker can retain other tests' synthetic sessions.
        // #94 requires reconciliation of this displaced owner, not an empty fleet.
        assert_eq!(
            response.prior_lease_owner_session_id.as_deref(),
            Some("g7a94-owner")
        );
        assert!(response.prior_lease_row_cleanup_error.is_none());
    }
    assert!(!lease::status().held);
    assert_eq!(
        crate::test_support::lease_released_reasons(&db, "g7a94-owner").expect("releases"),
        vec!["operator_hotkey_preempted"]
    );
    assert_eq!(f::row(&db, b"mcp/session-lease/v1/g7a94-owner"), None);
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_unconfirmed_dashboard_release_preserves_lease_row_and_events() {
    let _lock = crate::test_support::lease_serial("g7a94");
    let (_dir, svc) = f::service();
    let db = held(&svc);
    let sessions = db
        .scan_cf(synapse_storage::cf::CF_SESSIONS)
        .expect("before rows");
    let events = db
        .scan_cf(synapse_storage::cf::CF_AGENT_EVENTS)
        .expect("before events");
    let error = svc
        .dashboard_control_lease_force_release("g7a94-owner".to_owned(), false)
        .await
        .expect_err("confirmation required");
    assert_eq!(
        f::code(&error),
        synapse_core::error_codes::TOOL_PARAMS_INVALID
    );
    assert_eq!(
        lease::status().owner_session_id.as_deref(),
        Some("g7a94-owner")
    );
    assert_eq!(
        db.scan_cf(synapse_storage::cf::CF_SESSIONS)
            .expect("after rows"),
        sessions
    );
    assert_eq!(
        db.scan_cf(synapse_storage::cf::CF_AGENT_EVENTS)
            .expect("after events"),
        events
    );
}

#[tokio::test(flavor = "current_thread")]
async fn must_pass_dashboard_handler_closes_once_then_returns_released_false() {
    let _lock = crate::test_support::lease_serial("g7a94");
    let (_dir, svc) = f::service();
    let db = held(&svc);
    let first = svc
        .dashboard_control_lease_force_release("g7a94-owner".to_owned(), true)
        .await
        .expect("release");
    assert!(first.released);
    assert!(!lease::status().held);
    let second = svc
        .dashboard_control_lease_force_release("g7a94-owner".to_owned(), true)
        .await
        .expect("repeat");
    assert!(!second.released);
    assert_eq!(
        crate::test_support::lease_released_reasons(&db, "g7a94-owner").expect("releases"),
        vec!["dashboard_force_release"]
    );
}
