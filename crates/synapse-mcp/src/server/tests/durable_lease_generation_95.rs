//! #95: durable identity gates through the real K2 and orphan-sweep bodies.
use super::*;
use crate::server::g7a_support as f;
use crate::test_support::{REAL_PROCESS_LOCK, lease_released_reasons, lease_serial};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tracing::{
    Event, Subscriber,
    field::{Field, Visit},
};
use tracing_subscriber::{Layer, layer::Context, prelude::*};

#[test]
fn must_pass_real_fleet_kill_test_entrypoints_hold_shared_process_lock_94() {
    // Lock ownership is a source property; do not trigger destructive work to
    // test whether the test suite itself serializes that destructive work.
    for (file, source) in [
        (
            "agent_control/tests.rs",
            include_str!("../agent_control/tests.rs"),
        ),
        (
            "emergency_handlers_94.rs",
            include_str!("emergency_handlers_94.rs"),
        ),
        (
            "durable_lease_generation_95.rs",
            include_str!("durable_lease_generation_95.rs"),
        ),
    ] {
        for function in source.split("async fn ").skip(1) {
            let name = function.split('(').next().expect("function name");
            // This negative test intentionally refuses before taking a fleet snapshot.
            if name == "must_fail_fleet_stop_requires_confirm_token" {
                continue;
            }
            for entry in [
                concat!(".operator_panic_", "kill_all("),
                concat!(".fleet_stop_", "impl("),
            ] {
                if let Some(call) = function.find(entry) {
                    let lock = function.find("REAL_PROCESS_LOCK.lock().await");
                    assert!(
                        lock.is_some_and(|position| position < call),
                        "{file}:{name} must hold shared process lock before real fleet kill"
                    );
                }
            }
        }
    }
}

fn acquire(svc: &SynapseService, owner: &str) -> LeaseStatus {
    let outcome = lease::try_acquire(owner, lease::ttl_from_ms(600_000));
    let lease::LeaseOutcome::Acquired(status) = outcome else {
        panic!("expected isolated acquisition: {outcome:?}")
    };
    svc.persist_session_lease_with_acquisition_event(owner, &status)
        .expect("persist actual acquisition");
    status
}

fn preempt() -> crate::safety::OperatorHotkeyImmediateReport {
    let before = lease::status();
    let displaced = lease::force_preempt("operator_hotkey");
    crate::safety::OperatorHotkeyImmediateReport {
        hotkey: "synthetic-K1-95",
        lease_before: before,
        preempted_lease: displaced,
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
    }
}

/// K2's `matched_sessions_before` enumerates the process-wide agent-state
/// tracker (`agent_control.rs` `live_spawned_agent_sessions`), which also holds
/// other tests' synthetic spawned sessions, so it is not empty under a parallel
/// run (same finding as `emergency_handlers_94`). #95 is about the lease
/// generation: the plain lease owner here is never a matched fleet session and
/// the fleet read itself must succeed.
fn assert_fleet_snapshot_excludes(
    response: &crate::server::agent_control::OperatorPanicKillAllResponse,
    owner: &str,
) {
    assert!(response.matched_sessions_before_error.is_none());
    assert!(
        !response
            .matched_sessions_before
            .iter()
            .any(|session| session == owner),
        "lease owner {owner} is not a spawned fleet session"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_stale_k2_replay_preserves_new_same_owner_generation_95() {
    let _fleet = REAL_PROCESS_LOCK.lock().await;
    let _lease = lease_serial("durable95-stale-k2");
    let (_dir, svc) = f::service();
    let owner = "durable95-stale-owner";
    let old = acquire(&svc, owner);
    let stale = preempt();
    let _ = lease::force_clear("synthetic completion before delayed K2");
    let current = acquire(&svc, owner);
    assert_ne!(old.generation, current.generation);
    let db = f::db(&svc);
    let key = session_lease_key(owner);
    let row_before = f::row(&db, &key).expect("new row exists");
    let events_before = db.scan_cf(cf::CF_AGENT_EVENTS).expect("events before");

    for _ in 0..2 {
        let response = svc
            .operator_panic_kill_all(stale.clone())
            .await
            .expect("real K2");
        assert_fleet_snapshot_excludes(&response, owner);
        assert!(response.operator_lease_cleared.is_none());
        assert!(response.prior_lease_row_cleanup_error.is_none());
        let cleanup = response
            .prior_lease_row_cleanup
            .expect("identity refusal readback");
        assert!(!cleanup.row_deleted);
        assert!(!cleanup.release_event_written);
        assert_eq!(
            f::row(&db, &key),
            Some(row_before.clone()),
            "stale K2 cannot close g2"
        );
        // K2 acts on the process-wide agent tracker, which may hold another
        // test's synthetic sessions; only an event for THIS owner is a defect.
        let events_after = db.scan_cf(cf::CF_AGENT_EVENTS).expect("events after");
        let new_owner_events: Vec<_> = events_after
            .iter()
            .filter(|event| !events_before.contains(event))
            .filter(|(_, value)| String::from_utf8_lossy(value).contains(owner))
            .collect();
        assert!(
            new_owner_events.is_empty(),
            "stale K2 wrote events for {owner}: {new_owner_events:?}"
        );
        assert!(
            lease_released_reasons(&db, owner)
                .expect("releases")
                .is_empty()
        );
        let after = lease::status();
        assert!(after.held);
        assert_eq!(after.owner_session_id.as_deref(), Some(owner));
        assert_eq!(after.generation, current.generation);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn must_pass_matching_k2_generation_closes_durable_row_once_95() {
    let _fleet = REAL_PROCESS_LOCK.lock().await;
    let _lease = lease_serial("durable95-matching-k2");
    let (_dir, svc) = f::service();
    let owner = "durable95-matching-owner";
    let acquired = acquire(&svc, owner);
    let db = f::db(&svc);
    let key = session_lease_key(owner);
    let row: serde_json::Value =
        serde_json::from_slice(&f::row(&db, &key).expect("row")).expect("valid row");
    assert_eq!(
        row["lease_generation"].as_str(),
        acquired.generation.as_deref()
    );
    let immediate = preempt();
    for attempt in 0..2 {
        let response = svc
            .operator_panic_kill_all(immediate.clone())
            .await
            .expect("real K2");
        assert_fleet_snapshot_excludes(&response, owner);
        assert!(response.prior_lease_row_cleanup_error.is_none());
        let cleanup = response.prior_lease_row_cleanup.expect("cleanup readback");
        assert_eq!(cleanup.row_deleted, attempt == 0);
        assert_eq!(cleanup.release_event_written, attempt == 0);
        assert_eq!(response.operator_lease_cleared.is_some(), attempt == 0);
        assert_eq!(f::row(&db, &key), None);
        assert_eq!(
            lease_released_reasons(&db, owner).expect("releases"),
            vec!["operator_hotkey_preempted"]
        );
        assert!(!lease::status().held);
    }
}

#[test]
fn must_fail_orphan_sweep_closes_old_same_owner_row_but_keeps_current_generation_95() {
    let _lease = lease_serial("durable95-sweep");
    let (_dir, svc) = f::service();
    let owner = "durable95-sweep-owner";
    let old = acquire(&svc, owner);
    let db = f::db(&svc);
    let key = session_lease_key(owner);
    let old_bytes = f::row(&db, &key).expect("g1 row");
    let _ = lease::force_clear("synthetic g1 completion");
    let current = acquire(&svc, owner);
    assert_ne!(old.generation, current.generation);
    let current_bytes = f::row(&db, &key).expect("g2 row");
    db.put_batch_pressure_bypass(cf::CF_SESSIONS, [(key.clone(), old_bytes.clone())])
        .expect("simulate delayed g1 persistence");
    assert_eq!(f::row(&db, &key), Some(old_bytes));
    let warnings = skipped_rows::SkippedRowWarnings::default();
    assert_eq!(
        reconcile_orphaned_session_lease_rows_with_warnings(
            &svc.m3_state_handle(),
            "orphan_sweep",
            &warnings
        )
        .expect("real sweep"),
        1
    );
    assert_eq!(f::row(&db, &key), None);
    assert_eq!(
        lease_released_reasons(&db, owner).expect("releases"),
        vec!["orphan_sweep"]
    );
    assert_eq!(
        reconcile_orphaned_session_lease_rows_with_warnings(
            &svc.m3_state_handle(),
            "orphan_sweep",
            &warnings
        )
        .expect("repeat"),
        0
    );
    db.put_batch_pressure_bypass(cf::CF_SESSIONS, [(key.clone(), current_bytes.clone())])
        .expect("persist current g2 again");
    let events = db
        .scan_cf(cf::CF_AGENT_EVENTS)
        .expect("events before matching sweep");
    assert_eq!(
        reconcile_orphaned_session_lease_rows_with_warnings(
            &svc.m3_state_handle(),
            "orphan_sweep",
            &warnings
        )
        .expect("matching sweep"),
        0
    );
    assert_eq!(f::row(&db, &key), Some(current_bytes));
    assert_eq!(
        db.scan_cf(cf::CF_AGENT_EVENTS).expect("events after"),
        events
    );
    assert!(lease::status().held);
    assert_eq!(lease::status().generation, current.generation);
}

#[derive(Clone, Default)]
struct WarningCounter(Arc<AtomicUsize>);
#[derive(Default)]
struct WarningCode(bool);
impl Visit for WarningCode {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "code"
            && format!("{value:?}") == "\"MCP_SESSION_LEASE_ORPHAN_SWEEP_ROW_SKIPPED\""
        {
            self.0 = true;
        }
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "code" && value == "MCP_SESSION_LEASE_ORPHAN_SWEEP_ROW_SKIPPED" {
            self.0 = true;
        }
    }
}
impl<S: Subscriber> Layer<S> for WarningCounter {
    fn on_event(&self, event: &Event<'_>, _cx: Context<'_, S>) {
        let mut code = WarningCode::default();
        event.record(&mut code);
        if code.0 && *event.metadata().level() == tracing::Level::WARN {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
}

#[test]
fn must_fail_bad_lease_generation_warns_once_without_aborting_other_orphans_95() {
    let _lease = lease_serial("durable95-bad-generation");
    let (_dir, svc) = f::service();
    let bad_owner = "a-durable95-bad";
    let good_owner = "z-durable95-orphan";
    let _ = acquire(&svc, good_owner);
    let _ = lease::force_clear("synthetic orphan");
    let current = acquire(&svc, bad_owner);
    let db = f::db(&svc);
    let bad_key = session_lease_key(bad_owner);
    let good_key = session_lease_key(good_owner);
    assert!(bad_key < good_key, "bad row must precede healthy orphan");
    let mut row: serde_json::Value =
        serde_json::from_slice(&f::row(&db, &bad_key).expect("row")).expect("valid v1 JSON");
    row["lease_generation"] = serde_json::json!(5);
    let bad_bytes = serde_json::to_vec(&row).expect("bad generation JSON");
    assert!(
        synapse_storage::decode_json::<PersistedSessionLease>(&bad_bytes).is_ok(),
        "outer v1 row must decode to reach identity gate"
    );
    db.put_batch_pressure_bypass(cf::CF_SESSIONS, [(bad_key.clone(), bad_bytes.clone())])
        .expect("seed malformed generation");
    assert!(f::row(&db, &good_key).is_some());
    assert_eq!(f::row(&db, &bad_key), Some(bad_bytes.clone()));
    let warnings = skipped_rows::SkippedRowWarnings::default();
    let count = WarningCounter::default();
    tracing::subscriber::with_default(tracing_subscriber::registry().with(count.clone()), || {
        for attempt in 0..5 {
            let closed = reconcile_orphaned_session_lease_rows_with_warnings(
                &svc.m3_state_handle(),
                "orphan_sweep",
                &warnings,
            )
            .expect("bad lease_generation must not abort sweep");
            assert_eq!(closed, u32::from(attempt == 0));
            assert_eq!(
                f::row(&db, &good_key),
                None,
                "later healthy orphan processed"
            );
            assert_eq!(
                f::row(&db, &bad_key),
                Some(bad_bytes.clone()),
                "bad row retained for repair"
            );
        }
    });
    assert_eq!(
        count.0.load(Ordering::SeqCst),
        1,
        "one actual warning over five sweeps"
    );
    assert_eq!(
        lease_released_reasons(&db, good_owner).expect("good releases"),
        vec!["orphan_sweep"]
    );
    assert!(
        lease_released_reasons(&db, bad_owner)
            .expect("bad releases")
            .is_empty()
    );
    assert!(lease::status().held);
    assert_eq!(lease::status().generation, current.generation);
}
