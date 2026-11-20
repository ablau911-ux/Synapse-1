//! #216 startup variant. Parent: session_continuity. No new seam.
use super::*;
use crate::server::g7a_support as f;

fn seed_orphan(db: &Db, id: &str) -> Vec<u8> {
    let now = f::now();
    let row = PersistedSessionLease {
        schema_version: 1,
        session_id: id.to_owned(),
        stored_at_unix_ms: now,
        renewed_at_unix_ms: now,
        ttl_ms: 600_000,
        expires_at_unix_ms: now + 600_000,
        owner_process_id: 0,
        owner_process_started_at_unix_ms: 0,
    };
    let key = session_lease_key(id);
    let bytes = synapse_storage::encode_json(&row).expect("encode");
    db.put_batch_pressure_bypass(cf::CF_SESSIONS, [(key.clone(), bytes.clone())])
        .expect("seed");
    assert_eq!(f::row(db, &key), Some(bytes));
    key
}

#[test]
fn must_pass_startup_reconciles_healthy_orphan_once() {
    let _lock = crate::test_support::lease_serial("g7a216");
    let (_dir, svc) = f::service();
    let db = f::db(&svc);
    let key = seed_orphan(&db, "g7a216-healthy");
    assert_eq!(
        crate::test_support::lease_released_reasons(&db, "g7a216-healthy").expect("events"),
        Vec::<String>::new()
    );
    for _ in 0..2 {
        svc.reconcile_persisted_session_leases_on_startup()
            .expect("startup");
    }
    assert_eq!(f::row(&db, &key), None);
    assert_eq!(
        crate::test_support::lease_released_reasons(&db, "g7a216-healthy").expect("events"),
        vec!["daemon_restart_reconciliation"]
    );
}

fn assert_corrupt_lease_isolated(bytes: Vec<u8>) {
    let _lock = crate::test_support::lease_serial("g7a216");
    let (_dir, svc) = f::service();
    let db = f::db(&svc);
    let good = seed_orphan(&db, "g7a216-z-good");
    let bad = session_lease_key("g7a216-a-corrupt");
    db.put_batch_pressure_bypass(cf::CF_SESSIONS, [(bad.clone(), bytes.clone())])
        .expect("seed bad");
    assert_eq!(f::row(&db, &bad), Some(bytes.clone()));
    svc.reconcile_persisted_session_leases_on_startup()
        .expect("bad foreign row must be skipped, not abort startup");
    assert_eq!(f::row(&db, &bad), Some(bytes));
    assert_eq!(f::row(&db, &good), None);
    assert_eq!(
        crate::test_support::lease_released_reasons(&db, "g7a216-z-good").expect("events"),
        vec!["daemon_restart_reconciliation"]
    );
}

#[test]
fn must_fail_corrupt_foreign_lease_cannot_abort_healthy_startup_reconciliation() {
    assert_corrupt_lease_isolated(b"{not-json".to_vec());
}

fn valid_looking_row(session_id: &str, schema: u32) -> Vec<u8> {
    let now = f::now();
    serde_json::to_vec(&serde_json::json!({
        "schema_version": schema, "session_id": session_id,
        "stored_at_unix_ms": now, "renewed_at_unix_ms": now,
        "ttl_ms": 600_000, "expires_at_unix_ms": now + 600_000,
        "owner_process_id": 0, "owner_process_started_at_unix_ms": 0
    }))
    .expect("row JSON")
}

#[test]
fn must_fail_foreign_schema_version_cannot_abort_healthy_startup_reconciliation() {
    assert_corrupt_lease_isolated(valid_looking_row("g7a216-a-corrupt", 2));
}

#[test]
fn must_fail_foreign_key_identity_mismatch_cannot_abort_healthy_startup_reconciliation() {
    assert_corrupt_lease_isolated(valid_looking_row("g7a216-other-session", 1));
}

#[derive(Default)]
struct StartupWarningCode(bool);

impl tracing::field::Visit for StartupWarningCode {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "code"
            && format!("{value:?}") == "\"MCP_SESSION_LEASE_STARTUP_ROW_SKIPPED\""
        {
            self.0 = true;
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "code" && value == "MCP_SESSION_LEASE_STARTUP_ROW_SKIPPED" {
            self.0 = true;
        }
    }
}

#[derive(Clone, Default)]
struct StartupWarningCounter(std::sync::Arc<std::sync::atomic::AtomicUsize>);

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for StartupWarningCounter {
    fn on_event(&self, event: &tracing::Event<'_>, _cx: tracing_subscriber::layer::Context<'_, S>) {
        let mut code = StartupWarningCode::default();
        event.record(&mut code);
        if code.0 && *event.metadata().level() == tracing::Level::WARN {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }
}

#[test]
fn must_fail_bad_generation_cannot_abort_startup_and_warns_once_95() {
    use tracing_subscriber::prelude::*;

    let _lock = crate::test_support::lease_serial("tf18-startup95");
    let (_dir, svc) = f::service();
    let db = f::db(&svc);
    let bad_id = "tf18-a-bad-generation";
    let good_id = "tf18-z-healthy-orphan";
    let bad_key = seed_orphan(&db, bad_id);
    let good_key = seed_orphan(&db, good_id);
    assert!(
        bad_key < good_key,
        "corrupt identity must sort before the healthy row"
    );
    let mut row: serde_json::Value =
        serde_json::from_slice(&f::row(&db, &bad_key).expect("bad row before mutation"))
            .expect("valid outer lease row");
    row["lease_generation"] = serde_json::json!(5);
    let corrupt = serde_json::to_vec(&row).expect("numeric generation JSON");
    assert!(
        synapse_storage::decode_json::<PersistedSessionLease>(&corrupt).is_ok(),
        "must reach the generation decoder rather than fail outer row decoding"
    );
    db.put_batch_pressure_bypass(cf::CF_SESSIONS, [(bad_key.clone(), corrupt.clone())])
        .expect("seed corrupt generation");
    assert_eq!(f::row(&db, &bad_key), Some(corrupt.clone()));
    assert!(f::row(&db, &good_key).is_some());
    let count = StartupWarningCounter::default();
    tracing::subscriber::with_default(tracing_subscriber::registry().with(count.clone()), || {
        svc.reconcile_persisted_session_leases_on_startup()
            .expect("bad generation must not abort startup reconciliation");
        assert_eq!(
            f::row(&db, &bad_key),
            Some(corrupt.clone()),
            "retain corrupt bytes"
        );
        assert_eq!(f::row(&db, &good_key), None, "process later healthy orphan");
    });
    assert_eq!(
        count.0.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "one startup warning for the corrupt generation row"
    );
    assert_eq!(
        crate::test_support::lease_released_reasons(&db, good_id).expect("good releases"),
        vec!["daemon_restart_reconciliation"]
    );
    assert!(
        crate::test_support::lease_released_reasons(&db, bad_id)
            .expect("bad releases")
            .is_empty()
    );
}
