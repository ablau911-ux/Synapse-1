//! Shared harness for the `require_mcp_session` middleware regression tests
//! (#13 #28 #29): a router whose inner `/mcp` handler only counts how many
//! requests got past the middleware.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use axum::{Router, body::Body, http::Request, middleware, routing::post};
use rmcp::transport::streamable_http_server::session::{
    SessionManager as _, local::LocalSessionManager,
};

use super::{SessionCleanupState, SessionRequestState, require_mcp_session};

type Transports = Arc<std::sync::Mutex<Vec<Box<dyn std::any::Any + Send>>>>;

pub(super) struct Harness {
    pub app: Router,
    pub manager: Arc<LocalSessionManager>,
    reached: Arc<AtomicUsize>,
}

impl Harness {
    pub fn new() -> Self {
        let manager = Arc::new(LocalSessionManager::default());
        let state = SessionCleanupState::request_state(
            Arc::clone(&manager),
            Arc::default(),
            Arc::default(),
        );
        Self::build(manager, state, false)
    }

    pub fn build(
        manager: Arc<LocalSessionManager>,
        state: SessionRequestState,
        create_sessions: bool,
    ) -> Self {
        let reached = Arc::new(AtomicUsize::new(0));
        let hits = Arc::clone(&reached);
        let handler_manager = Arc::clone(&manager);
        let transports: Transports = Arc::default();
        let app = Router::new()
            .route(
                "/mcp",
                post(move || {
                    let hits = Arc::clone(&hits);
                    let manager = Arc::clone(&handler_manager);
                    let transports = Arc::clone(&transports);
                    async move {
                        hits.fetch_add(1, Ordering::SeqCst);
                        if create_sessions {
                            tokio::time::sleep(Duration::from_millis(20)).await;
                            let (_id, transport) = manager.create_session().await.unwrap();
                            transports.lock().unwrap().push(Box::new(transport));
                        }
                        "ok"
                    }
                }),
            )
            .layer(middleware::from_fn_with_state(state, require_mcp_session));
        Self {
            app,
            manager,
            reached,
        }
    }

    /// Requests that got past the middleware into the inner handler.
    pub fn reached(&self) -> usize {
        self.reached.load(Ordering::SeqCst)
    }
}

pub(super) fn post_mcp(session: Option<&str>, content_type: &str, body: &str) -> Request<Body> {
    let mut builder = Request::post("/mcp").header("content-type", content_type);
    if let Some(session) = session {
        builder = builder.header("Mcp-Session-Id", session);
    }
    builder.body(Body::from(body.to_owned())).unwrap()
}

pub(super) const INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
pub(super) const PING: &str = r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#;
