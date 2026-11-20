//! #215 r2: accepted requests, not periodic refresh or a copied heartbeat.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
    middleware,
    routing::post,
};
use rmcp::{
    model::{ClientCapabilities, Implementation, InitializeRequestParams},
    transport::streamable_http_server::session::{SessionManager as _, local::LocalSessionManager},
};
use std::{collections::HashMap, sync::LazyLock, thread::ThreadId};
use tower::ServiceExt as _;

// Session-scoped, cross-thread probe at the real storage write call site.
// No injected result or replacement persistence implementation.
static WRITES: LazyLock<Mutex<HashMap<String, Vec<ThreadId>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub(super) fn record_write_attempt(session_id: &str) {
    let mut writes = WRITES.lock().expect("write probe");
    if let Some(threads) = writes.get_mut(session_id) {
        threads.push(std::thread::current().id());
    }
}

struct Fixture {
    db: Arc<Db>,
    app: Router,
    id: String,
    _transport: Box<dyn std::any::Any + Send>,
    _dir: tempfile::TempDir,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        WRITES.lock().expect("write probe").remove(&self.id);
    }
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(Db::open(&dir.path().join("db"), synapse_core::SCHEMA_VERSION).expect("db"));
    let manager = Arc::new(LocalSessionManager::default());
    let (id, transport) = manager.create_session().await.expect("live session");
    let id = id.to_string();
    let initialized = unix_time_ms().expect("clock") - 120_000;
    let session = SessionState::new(InitializeRequestParams::new(
        ClientCapabilities::default(),
        Implementation::new("activity-215-r2", "test"),
    ));
    let registry = Arc::new(Mutex::new(
        crate::server::session_registry::SessionRegistry::default(),
    ));
    registry
        .lock()
        .expect("registry")
        .record_initialized(&id, &session, "http", initialized);
    let row = PersistedMcpSessionState {
        stored_at_unix_ms: initialized,
        last_activity_unix_ms: None,
        state: session,
    };
    db.put_batch_pressure_bypass(
        cf::CF_KV,
        [(
            mcp_session_store_key(&id),
            synapse_storage::encode_json(&row).expect("encode"),
        )],
    )
    .expect("seed row");
    WRITES
        .lock()
        .expect("write probe")
        .insert(id.clone(), Vec::new());
    let state =
        crate::http::session::SessionCleanupState::request_state(manager, registry, Arc::default())
            .with_activity_db(Arc::clone(&db));
    let app = Router::new()
        .route("/mcp", post(|| async { "accepted" }))
        .layer(middleware::from_fn_with_state(
            state,
            crate::http::session::require_mcp_session,
        ));
    Fixture {
        _dir: dir,
        db,
        app,
        id,
        _transport: Box::new(transport),
    }
}

async fn ping(f: &Fixture) -> StatusCode {
    f.app
        .clone()
        .oneshot(
            Request::post("/mcp")
                .header("content-type", "application/json")
                .header("Mcp-Session-Id", &f.id)
                .body(Body::from(r#"{"jsonrpc":"2.0","id":215,"method":"ping"}"#))
                .expect("request"),
        )
        .await
        .expect("router")
        .status()
}

async fn wait_for_write(f: &Fixture) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let reached = !WRITES.lock().expect("write probe")[&f.id].is_empty();
            let persisted =
                f.db.get_cf_exact(cf::CF_KV, &mcp_session_store_key(&f.id))
                    .expect("separate activity read")
                    .expect("session row");
            let persisted: PersistedMcpSessionState =
                synapse_storage::decode_json(&persisted).expect("decode activity");
            if reached && persisted.last_activity_unix_ms.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("first due activity write must reach storage");
}

#[tokio::test(flavor = "current_thread")]
async fn must_pass_rapid_requests_persist_at_most_once_per_session_window_215() {
    let f = fixture().await;
    for _ in 0..8 {
        assert_eq!(ping(&f).await, StatusCode::OK);
        // Distinct timestamps expose per-request fsync, still far below even
        // the 15-second TTL's 3750ms persistence window.
        tokio::time::sleep(Duration::from_millis(3)).await;
    }
    wait_for_write(&f).await;
    let count = WRITES.lock().expect("write probe")[&f.id].len();
    assert_eq!(
        count, 1,
        "rapid accepted requests each rewrote durable session activity"
    );
    let bytes =
        f.db.get_cf_exact(cf::CF_KV, &mcp_session_store_key(&f.id))
            .expect("separate read")
            .expect("row");
    let row: PersistedMcpSessionState = synapse_storage::decode_json(&bytes).expect("row decode");
    assert!(row.last_activity_unix_ms.is_some());
}

#[tokio::test(flavor = "current_thread")]
async fn must_pass_activity_storage_runs_off_async_request_thread_215() {
    let f = fixture().await;
    let request_thread = std::thread::current().id();
    assert_eq!(ping(&f).await, StatusCode::OK);
    wait_for_write(&f).await;
    assert!(
        WRITES.lock().expect("write probe")[&f.id]
            .iter()
            .all(|thread| *thread != request_thread),
        "session WAL write ran on the async request worker"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn must_pass_activity_storage_failure_does_not_fail_accepted_request_215() {
    let f = fixture().await;
    f.db.fail_next_read_for_test(cf::CF_KV, &mcp_session_store_key(&f.id));
    assert_eq!(
        ping(&f).await,
        StatusCode::OK,
        "best-effort activity IO failure must not become session_registry_failed"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn must_pass_activity_respects_disk_pressure_shedding_215() {
    let f = fixture().await;
    let key = mcp_session_store_key(&f.id);
    let before = f.db.get_cf_exact(cf::CF_KV, &key).expect("before");
    assert_eq!(
        f.db.run_pressure_check_with_free_bytes_sample(0)
            .expect("pressure sample")
            .current_level,
        synapse_storage::DiskPressureLevel::Level4
    );
    assert!(!f.db.pressure_permits_write(cf::CF_KV));
    assert_eq!(ping(&f).await, StatusCode::OK);
    // Give the best-effort worker its opportunity, then read the physical row.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        f.db.get_cf_exact(cf::CF_KV, &key).expect("after"),
        before,
        "activity bypassed Level4 shedding"
    );
}
