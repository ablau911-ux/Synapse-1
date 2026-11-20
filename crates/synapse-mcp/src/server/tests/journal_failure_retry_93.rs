//! #93. Parent: session_continuity. NEEDS-SEAM at atomic lease-release batch.
//! Missing seam must inject before DB commit, be db+session scoped, fail once,
//! and restore on Drop. A compile error is NOT the required red evidence.
use super::*;
use crate::server::g7a_support as f;

fn orphan(db: &Db) -> (Vec<u8>, Vec<u8>) {
    let now = f::now();
    let row = PersistedSessionLease {
        schema_version: 1,
        session_id: "g7a93-owner".to_owned(),
        stored_at_unix_ms: now,
        renewed_at_unix_ms: now,
        ttl_ms: 600_000,
        expires_at_unix_ms: now + 600_000,
        owner_process_id: 0,
        owner_process_started_at_unix_ms: 0,
    };
    let key = session_lease_key(&row.session_id);
    let bytes = synapse_storage::encode_json(&row).expect("encode");
    db.put_batch_pressure_bypass(cf::CF_SESSIONS, [(key.clone(), bytes.clone())])
        .expect("seed");
    assert_eq!(f::row(db, &key), Some(bytes.clone()));
    (key, bytes)
}

#[test]
fn must_pass_orphan_release_does_not_clear_an_unrelated_new_physical_lease() {
    let _lock = crate::test_support::lease_serial("g7a93");
    let (_dir, svc) = f::service();
    let db = f::db(&svc);
    let (key, _) = orphan(&db);
    let acquired = lease::try_acquire("g7a93-new", lease::ttl_from_ms(600_000));
    assert!(matches!(acquired, lease::LeaseOutcome::Acquired(_)));
    assert_eq!(
        lease::status().owner_session_id.as_deref(),
        Some("g7a93-new")
    );
    assert_eq!(
        reconcile_orphaned_session_lease_rows(&svc.m3_state_handle(), "expired_lease_cleanup")
            .expect("sweep"),
        1
    );
    assert_eq!(f::row(&db, &key), None);
    assert_eq!(
        lease::status().owner_session_id.as_deref(),
        Some("g7a93-new")
    );
    assert_eq!(
        crate::test_support::lease_released_reasons(&db, "g7a93-owner").expect("events"),
        vec!["expired_lease_cleanup"]
    );
}

async fn wait_for_expiry() {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut poll = tokio::time::interval(std::time::Duration::from_millis(5));
    while lease::status().held {
        assert!(tokio::time::Instant::now() < deadline, "expiry deadline");
        poll.tick().await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_release_journal_write_retains_marker_then_retries_exactly_once() {
    let _lock = crate::test_support::lease_serial("g7a93");
    let (_dir, svc) = f::service();
    let db = f::db(&svc);
    let lease::LeaseOutcome::Acquired(status) =
        lease::try_acquire("g7a93-owner", lease::ttl_from_ms(1000))
    else {
        panic!("acquire")
    };
    svc.persist_session_lease_with_acquisition_event("g7a93-owner", &status)
        .expect("acquisition boundary");
    let key = session_lease_key("g7a93-owner");
    let bytes = f::row(&db, &key).expect("persisted live lease");
    assert_eq!(
        lease::status().owner_session_id.as_deref(),
        Some("g7a93-owner")
    );
    let events = db.scan_cf(cf::CF_AGENT_EVENTS).expect("before events");
    let _fault =
        super::super::agent_events::fail_next_lease_release_batch_for_test(&db, "g7a93-owner");
    wait_for_expiry().await;
    let lifecycle = svc.session_lifecycle_state().expect("lifecycle");
    lifecycle.cleanup_expired_lease_inputs_once().await;
    assert_eq!(
        f::row(&db, &key),
        Some(bytes),
        "failed atomic write retains retry identity"
    );
    assert_eq!(
        db.scan_cf(cf::CF_AGENT_EVENTS).expect("after failure"),
        events
    );
    assert!(!lease::status().held);
    for _ in 0..2 {
        lifecycle.reconcile_orphaned_lease_rows_once().await;
    }
    assert_eq!(f::row(&db, &key), None);
    assert_eq!(
        crate::test_support::lease_released_reasons(&db, "g7a93-owner").expect("events"),
        vec!["orphan_sweep"]
    );
}
