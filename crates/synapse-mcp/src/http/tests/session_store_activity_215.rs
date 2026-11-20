//! Regression tests for #215: the persisted-session restore window must run
//! from the session's last activity, not from `initialize`. A session that was
//! initialized more than the keep-alive ago but is still in use must survive a
//! daemon restart; a genuinely idle one must still expire.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rmcp::model::{ClientCapabilities, Implementation, InitializeRequestParams};
use synapse_core::SCHEMA_VERSION;

use super::*;

const HOUR_MS: u64 = 3_600_000;

fn state(name: &str) -> SessionState {
    SessionState::new(InitializeRequestParams::new(
        ClientCapabilities::default(),
        Implementation::new(name, "0.0.0-test"),
    ))
}

fn registry_with(
    session_id: &str,
    state: &SessionState,
    initialized_at: u64,
    last_request_at: u64,
) -> crate::server::session_registry::SharedSessionRegistry {
    let registry = Arc::new(Mutex::new(
        crate::server::session_registry::SessionRegistry::default(),
    ));
    {
        let mut guard = registry.lock().expect("registry");
        guard.record_initialized(session_id, state, "http", initialized_at);
        guard.record_request_seen(session_id, Some("tools/call:x".to_owned()), last_request_at);
    }
    registry
}

fn write_row(db: &Db, session_id: &str, state: &SessionState, stored_at: u64) {
    let encoded = synapse_storage::encode_json(&PersistedMcpSessionState {
        stored_at_unix_ms: stored_at,
        last_activity_unix_ms: None,
        state: state.clone(),
    })
    .expect("encode");
    db.put_batch_pressure_bypass(cf::CF_KV, [(mcp_session_store_key(session_id), encoded)])
        .expect("write row");
}

fn persisted(db: &Db, session_id: &str) -> PersistedMcpSessionState {
    let key = mcp_session_store_key(session_id);
    let value = db
        .get_cf_exact(cf::CF_KV, &key)
        .expect("read row")
        .expect("row exists");
    synapse_storage::decode_json::<PersistedMcpSessionState>(&value).expect("decode")
}

/// `stored_at` is the initialize time and must never move (the registry's
/// `started_at` and CDP-owner recovery compare against it).
fn stored_at(db: &Db, session_id: &str) -> u64 {
    persisted(db, session_id).stored_at_unix_ms
}

fn last_activity(db: &Db, session_id: &str) -> Option<u64> {
    persisted(db, session_id).last_activity_unix_ms
}

fn open_db() -> (tempfile::TempDir, Arc<Db>) {
    let temp = tempfile::tempdir().expect("temp dir");
    let db = Arc::new(Db::open(&temp.path().join("db"), SCHEMA_VERSION).expect("db"));
    (temp, db)
}

fn restarted_store(db: &Arc<Db>, ttl: Duration) -> SynapseMcpSessionStore {
    // A restarted daemon has an empty in-memory registry.
    SynapseMcpSessionStore::new(
        Arc::clone(db),
        Some(ttl),
        Arc::new(Mutex::new(
            crate::server::session_registry::SessionRegistry::default(),
        )),
    )
}

#[tokio::test]
async fn must_pass_active_session_older_than_keep_alive_survives_a_restart() {
    let (_temp, db) = open_db();
    let ttl = Duration::from_secs(24 * 3600);
    let now = unix_time_ms().expect("now");
    let initialized_at = now - 25 * HOUR_MS;
    let session_state = state("active-215");
    write_row(&db, "active-215", &session_state, initialized_at);
    let registry = registry_with("active-215", &session_state, initialized_at, now);
    let store = SynapseMcpSessionStore::new(Arc::clone(&db), Some(ttl), registry);

    let refreshed = store.refresh_activity(now).expect("refresh");

    assert_eq!(refreshed, 1);
    assert_eq!(last_activity(&db, "active-215"), Some(now));
    assert_eq!(
        stored_at(&db, "active-215"),
        initialized_at,
        "the initialize time must not move"
    );
    let restored = restarted_store(&db, ttl)
        .load("active-215")
        .await
        .expect("load");
    let restored = restored.expect("a session in use until the restart must be restorable");
    assert_eq!(
        restored.initialize_params.client_info.name, "active-215",
        "the restored session must be the one that was persisted"
    );
    assert_eq!(
        serde_json::to_value(&restored).expect("restored json"),
        serde_json::to_value(&session_state).expect("persisted json"),
        "restored state must equal the persisted initialize state"
    );
}

#[tokio::test]
async fn must_pass_idle_session_still_expires_after_the_keep_alive() {
    let (_temp, db) = open_db();
    let ttl = Duration::from_secs(24 * 3600);
    let now = unix_time_ms().expect("now");
    let initialized_at = now - 25 * HOUR_MS;
    let session_state = state("idle-215");
    write_row(&db, "idle-215", &session_state, initialized_at);
    // Last request 25 h ago: no activity inside the keep-alive window.
    let registry = registry_with("idle-215", &session_state, initialized_at, initialized_at);
    let store = SynapseMcpSessionStore::new(Arc::clone(&db), Some(ttl), registry);

    let refreshed = store.refresh_activity(now).expect("refresh");

    assert_eq!(refreshed, 0);
    assert_eq!(last_activity(&db, "idle-215"), None);
    let restored = restarted_store(&db, ttl)
        .load("idle-215")
        .await
        .expect("load");
    assert!(restored.is_none(), "an idle session must expire");
}

#[test]
fn must_pass_refresh_is_throttled_for_recent_activity() {
    let (_temp, db) = open_db();
    let now = unix_time_ms().expect("now");
    let initialized_at = now - 10_000;
    let session_state = state("recent-215");
    write_row(&db, "recent-215", &session_state, initialized_at);
    let registry = registry_with("recent-215", &session_state, initialized_at, now);
    let store = SynapseMcpSessionStore::new(
        Arc::clone(&db),
        Some(Duration::from_secs(24 * 3600)),
        registry,
    );

    let refreshed = store.refresh_activity(now).expect("refresh");

    assert_eq!(
        refreshed, 0,
        "activity within the throttle must not rewrite"
    );
    assert_eq!(last_activity(&db, "recent-215"), None);
}

#[test]
fn must_fail_refresh_never_resurrects_a_deleted_row_or_touches_closed_sessions() {
    let (_temp, db) = open_db();
    let now = unix_time_ms().expect("now");
    let initialized_at = now - 25 * HOUR_MS;
    let closed_state = state("closed-215");
    let gone_state = state("gone-215");
    write_row(&db, "closed-215", &closed_state, initialized_at);
    let registry = registry_with("closed-215", &closed_state, initialized_at, now);
    {
        let mut guard = registry.lock().expect("registry");
        guard.record_initialized("gone-215", &gone_state, "http", initialized_at);
        guard.record_request_seen("gone-215", None, now);
        guard.record_closed("closed-215", now);
    }
    // "gone-215" is live in the registry but its row was already deleted.
    let store = SynapseMcpSessionStore::new(
        Arc::clone(&db),
        Some(Duration::from_secs(24 * 3600)),
        registry,
    );

    let refreshed = store.refresh_activity(now).expect("refresh");

    assert_eq!(refreshed, 0);
    assert_eq!(last_activity(&db, "closed-215"), None);
    let gone_key = mcp_session_store_key("gone-215");
    assert!(
        db.get_cf_exact(cf::CF_KV, &gone_key)
            .expect("read")
            .is_none(),
        "refresh must not recreate a deleted session row"
    );
}
