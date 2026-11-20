//! Shared fixtures only. Wire under server as `g7a_support` (cfg(test)).
use crate::server::SynapseService;
use rmcp::model::{ClientCapabilities, Implementation, InitializeRequestParams};
use serde_json::Value;
use std::sync::Arc;
use synapse_storage::{Db, cf};

pub(super) fn service() -> (tempfile::TempDir, SynapseService) {
    let dir = tempfile::tempdir().expect("isolated directory");
    let svc = SynapseService::try_new_isolated_for_tests(dir.path()).expect("isolated service");
    (dir, svc)
}

pub(super) fn db(svc: &SynapseService) -> Arc<Db> {
    svc.m3_state_handle()
        .lock()
        .expect("m3")
        .ensure_storage()
        .expect("storage")
}

pub(super) fn now() -> u64 {
    crate::server::session_registry::unix_time_ms_now()
}

pub(super) fn seed(svc: &SynapseService, id: &str, at: u64) {
    let state = rmcp::transport::streamable_http_server::session::SessionState::new(
        InitializeRequestParams::new(
            ClientCapabilities::default(),
            Implementation::new("claude-code", "test"),
        ),
    );
    let registry = svc.session_registry_ref();
    let mut registry = registry.lock().expect("registry");
    assert!(registry.record_initialized(id, &state, "http", at));
    let rows = registry.reads(now());
    let row = rows
        .iter()
        .find(|row| row.session_id == id)
        .expect("seed readback");
    assert_eq!(row.lifecycle, "live");
    assert_eq!(row.agent_kind, "claude");
}

pub(super) fn code(error: &rmcp::ErrorData) -> &str {
    error
        .data
        .as_ref()
        .and_then(|d| d["code"].as_str())
        .expect("typed error")
}

pub(super) fn audit_rows(svc: &SynapseService) -> Vec<Value> {
    let runtime = svc.reflex_runtime().expect("reflex runtime");
    let rows = runtime
        .lock()
        .expect("reflex lock")
        .storage_cf_tail_rows(cf::CF_ACTION_LOG, 2048)
        .expect("audit SoT");
    let mut values: Vec<Value> = rows
        .into_iter()
        .map(|(_, b)| serde_json::from_slice(&b).expect("row JSON"))
        .collect();
    values.sort_by_key(|v| (v["ts_ns"].as_u64(), v["seq"].as_u64()));
    values
}

pub(super) fn row(db: &Db, key: &[u8]) -> Option<Vec<u8>> {
    db.scan_cf_prefix(cf::CF_SESSIONS, key)
        .expect("CF_SESSIONS")
        .into_iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v)
}
