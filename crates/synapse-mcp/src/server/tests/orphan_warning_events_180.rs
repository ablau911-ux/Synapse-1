//! #180: count real emitted warning events, not the bookkeeping helper.
//! Parent: session_continuity. EXPECT-GREEN.
use super::*;
use crate::server::g7a_support as f;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tracing::{
    Event, Subscriber,
    field::{Field, Visit},
};
use tracing_subscriber::{Layer, layer::Context, prelude::*};

#[derive(Clone, Default)]
struct WarningCounter(Arc<AtomicUsize>);
#[derive(Default)]
struct CodeVisitor(bool);
impl Visit for CodeVisitor {
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
        let mut visitor = CodeVisitor::default();
        event.record(&mut visitor);
        if visitor.0 {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
}

fn sweep(svc: &SynapseService, warnings: &skipped_rows::SkippedRowWarnings) {
    assert_eq!(
        reconcile_orphaned_session_lease_rows_with_warnings(
            &svc.m3_state_handle(),
            "orphan_sweep",
            warnings
        )
        .expect("sweep"),
        0
    );
}

#[test]
fn must_pass_empty_real_sweeps_emit_zero_corrupt_row_warnings() {
    let _lock = crate::test_support::lease_serial("g7a180");
    let (_dir, svc) = f::service();
    assert_eq!(
        f::db(&svc)
            .scan_cf_prefix(cf::CF_SESSIONS, SESSION_LEASE_PREFIX.as_bytes())
            .expect("SoT")
            .len(),
        0
    );
    let count = WarningCounter::default();
    let warnings = skipped_rows::SkippedRowWarnings::default();
    tracing::subscriber::with_default(tracing_subscriber::registry().with(count.clone()), || {
        for _ in 0..5 {
            sweep(&svc, &warnings);
        }
    });
    assert_eq!(count.0.load(Ordering::SeqCst), 0);
}

#[test]
fn must_fail_corrupt_row_warns_once_retains_bytes_and_rearms_after_repair() {
    let _lock = crate::test_support::lease_serial("g7a180");
    let (_dir, svc) = f::service();
    let db = f::db(&svc);
    let key = session_lease_key("g7a180-bad");
    let bytes = b"{not-json".to_vec();
    db.put_batch_pressure_bypass(cf::CF_SESSIONS, [(key.clone(), bytes.clone())])
        .expect("seed");
    assert_eq!(f::row(&db, &key), Some(bytes.clone()));
    let events = db.scan_cf(cf::CF_AGENT_EVENTS).expect("events");
    let count = WarningCounter::default();
    let warnings = skipped_rows::SkippedRowWarnings::default();
    tracing::subscriber::with_default(tracing_subscriber::registry().with(count.clone()), || {
        for _ in 0..5 {
            sweep(&svc, &warnings);
        }
        assert_eq!(
            count.0.load(Ordering::SeqCst),
            1,
            "one real warning across five real scans"
        );
        assert_eq!(f::row(&db, &key), Some(bytes.clone()));
        db.delete_batch(cf::CF_SESSIONS, [key.clone()])
            .expect("repair synthetic row");
        sweep(&svc, &warnings);
        db.put_batch_pressure_bypass(cf::CF_SESSIONS, [(key.clone(), bytes.clone())])
            .expect("re-corrupt");
        sweep(&svc, &warnings);
        assert_eq!(count.0.load(Ordering::SeqCst), 2);
    });
    assert_eq!(f::row(&db, &key), Some(bytes));
    assert_eq!(db.scan_cf(cf::CF_AGENT_EVENTS).expect("events"), events);
}
