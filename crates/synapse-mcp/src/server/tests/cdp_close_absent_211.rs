//! #211: an explicit raw-CDP `cdp_close_tab` of a tab that is already gone
//! (the user closed it) is reclaimed instead of failing: owner row, session
//! target and target claim are cleared, and the response reports the real
//! `Target.getTargets` counts. A fake CDP endpoint (HTTP `/json/version` plus a
//! websocket answering `Target.*`) drives the real `cdp_close_tab_impl` path.
use std::sync::{Arc, Mutex, PoisonError};

use axum::{
    Json, Router,
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::Response,
    routing::get,
};
use serde_json::{Value, json};
use tempfile::TempDir;

use super::{
    CdpTargetOwner, SynapseService,
    tests::{insert_test_target_claim, seed_session_client, service_with_temp_db},
};

const HWND: i64 = 0x2111;
const TARGET: &str = "ABC211";

/// Fake browser-level CDP endpoint with a mutable target table.
struct FakeCdp {
    port: u16,
    targets: Mutex<Vec<String>>,
    fail_close: bool,
    close_calls: Mutex<u32>,
}

impl FakeCdp {
    fn targets(&self) -> Vec<String> {
        self.targets
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn close_calls(&self) -> u32 {
        *self
            .close_calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn reply(&self, request: &Value) -> Value {
        let id = request["id"].clone();
        match request["method"].as_str().unwrap_or_default() {
            "Target.getTargets" => {
                let infos: Vec<Value> = self
                    .targets()
                    .iter()
                    .map(String::as_str)
                    .map(target_info)
                    .collect();
                json!({ "id": id, "result": { "targetInfos": infos } })
            }
            "Target.closeTarget" => {
                let target_id = request["params"]["targetId"].as_str().unwrap_or_default();
                self.close(id, target_id)
            }
            _ => json!({ "id": id, "result": {} }),
        }
    }

    fn close(&self, id: Value, target_id: &str) -> Value {
        *self
            .close_calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner) += 1;
        if self.fail_close {
            return json!({ "id": id, "error": { "code": -32000, "message": "fake close failure #211" } });
        }
        self.targets
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|listed| listed != target_id);
        json!({ "id": id, "result": { "success": true } })
    }
}

fn target_info(target_id: &str) -> Value {
    json!({
        "targetId": target_id,
        "type": "page",
        "title": "",
        "url": "about:blank",
        "attached": false,
        "canAccessOpener": false,
    })
}

async fn json_version(State(fake): State<Arc<FakeCdp>>) -> Json<Value> {
    Json(json!({
        "Browser": "FakeChrome/211",
        "Protocol-Version": "1.3",
        "User-Agent": "fake-211",
        "V8-Version": "0",
        "WebKit-Version": "0",
        "webSocketDebuggerUrl": format!("ws://127.0.0.1:{}/devtools/browser/fake-211", fake.port),
    }))
}

async fn devtools(State(fake): State<Arc<FakeCdp>>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| serve_socket(socket, fake))
}

async fn serve_socket(mut socket: WebSocket, fake: Arc<FakeCdp>) {
    while let Some(Ok(message)) = socket.recv().await {
        let Message::Text(text) = message else {
            continue;
        };
        let Ok(request) = serde_json::from_str::<Value>(text.as_str()) else {
            continue;
        };
        let reply = fake.reply(&request).to_string();
        if socket.send(Message::Text(reply.into())).await.is_err() {
            break;
        }
    }
}

/// Starts the fake endpoint; returns it and its `http://127.0.0.1:<port>` URL.
async fn spawn_fake_cdp(
    targets: &[&str],
    fail_close: bool,
) -> anyhow::Result<(Arc<FakeCdp>, String)> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let fake = Arc::new(FakeCdp {
        port,
        targets: Mutex::new(targets.iter().map(|id| (*id).to_owned()).collect()),
        fail_close,
        close_calls: Mutex::new(0),
    });
    let app = Router::new()
        .route("/json/version", get(json_version))
        .route("/devtools/browser/fake-211", get(devtools))
        .with_state(Arc::clone(&fake));
    tokio::spawn(async move {
        let _served = axum::serve(listener, app).await;
    });
    Ok((fake, format!("http://127.0.0.1:{port}")))
}

fn owner(session_id: &str, endpoint: &str) -> CdpTargetOwner {
    CdpTargetOwner {
        session_id: session_id.to_owned(),
        window_hwnd: HWND,
        endpoint: endpoint.to_owned(),
        chrome_window_id: None,
        capture_window_hwnd: None,
        cdp_target_id: TARGET.to_owned(),
        requested_url: "about:blank#211".to_owned(),
        target_url: "about:blank#211".to_owned(),
        created_at_unix_ms: 1,
    }
}

fn claims_held_by(service: &SynapseService, session_id: &str) -> anyhow::Result<usize> {
    let now = crate::server::session_registry::unix_time_ms_now();
    Ok(service
        .lock_target_claims()?
        .reads(now)
        .into_iter()
        .filter(|claim| claim.owner_session_id == session_id)
        .count())
}

/// A session holding an owner row and a claim for `TARGET` on `endpoint`.
fn claimed_session(
    service: &SynapseService,
    session: &str,
    endpoint: &str,
) -> anyhow::Result<(CdpTargetOwner, String)> {
    let now = crate::server::session_registry::unix_time_ms_now();
    seed_session_client(service, session, "claude-code", now)?;
    let owner = owner(session, endpoint);
    let owner_key = service.register_cdp_target_owner(owner.clone())?;
    insert_test_target_claim(service, session, HWND, TARGET)?;
    assert_eq!(claims_held_by(service, session)?, 1);
    Ok((owner, owner_key))
}

fn error_code(error: &rmcp::ErrorData) -> Option<&str> {
    error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(Value::as_str)
}

fn owner_rows(service: &SynapseService) -> anyhow::Result<usize> {
    Ok(service
        .read_persisted_cdp_target_owners_for_target_id(TARGET)?
        .len())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_pass_closing_an_already_closed_tab_reclaims_it_with_real_counts() -> anyhow::Result<()>
{
    // The user closed TARGET; two other tabs remain in Target.getTargets.
    let (fake, endpoint) = spawn_fake_cdp(&["OTHER1", "OTHER2"], false).await?;
    let dir = TempDir::new()?;
    let service = service_with_temp_db(dir.path())?;
    let session = "i211-absent";
    let (owner, owner_key) = claimed_session(&service, session, &endpoint)?;

    let response = service
        .cdp_close_tab_impl(session, TARGET, &owner_key, owner)
        .await
        .map_err(|error| anyhow::anyhow!("absent tab must reclaim, got {error:?}"))?;

    assert!(!response.closed, "nothing was closed: {response:?}");
    assert_eq!(response.cdp_target_id, TARGET);
    assert_eq!(response.target_count_before, 2, "real count: {response:?}");
    assert_eq!(response.target_count_after, 2, "real count: {response:?}");
    assert_eq!(fake.close_calls(), 0, "an absent tab is never closed");
    assert_eq!(owner_rows(&service)?, 0, "owner row must be removed");
    assert_eq!(
        claims_held_by(&service, session)?,
        0,
        "claim must be released"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_pass_closing_a_present_tab_closes_it_with_real_counts() -> anyhow::Result<()> {
    let (fake, endpoint) = spawn_fake_cdp(&["OTHER1", TARGET], false).await?;
    let dir = TempDir::new()?;
    let service = service_with_temp_db(dir.path())?;
    let session = "i211-present";
    let (owner, owner_key) = claimed_session(&service, session, &endpoint)?;

    let response = service
        .cdp_close_tab_impl(session, TARGET, &owner_key, owner)
        .await
        .map_err(|error| anyhow::anyhow!("present tab must close, got {error:?}"))?;

    assert!(response.closed, "{response:?}");
    assert_eq!(response.target_count_before, 2, "{response:?}");
    assert_eq!(response.target_count_after, 1, "{response:?}");
    assert_eq!(fake.close_calls(), 1);
    assert_eq!(fake.targets(), ["OTHER1"]);
    assert_eq!(owner_rows(&service)?, 0);
    assert_eq!(claims_held_by(&service, session)?, 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_fail_a_present_tab_whose_close_fails_is_not_reclaimed_as_absent() -> anyhow::Result<()>
{
    // TARGET is listed, but Target.closeTarget fails for another reason.
    let (fake, endpoint) = spawn_fake_cdp(&["OTHER1", TARGET], true).await?;
    let dir = TempDir::new()?;
    let service = service_with_temp_db(dir.path())?;
    let session = "i211-close-fails";
    let (owner, owner_key) = claimed_session(&service, session, &endpoint)?;

    let outcome = service
        .cdp_close_tab_impl(session, TARGET, &owner_key, owner)
        .await;

    let error = outcome.expect_err("a failed close is an error");
    assert_eq!(
        error_code(&error),
        Some(synapse_core::error_codes::A11Y_CDP_AXTREE_FAILED),
        "{error:?}"
    );
    assert!(
        error
            .message
            .contains("cdp_close_tab Target.closeTarget/readback failed"),
        "{error:?}"
    );
    assert_eq!(fake.close_calls(), 1, "the close was attempted");
    assert_eq!(fake.targets(), ["OTHER1", TARGET], "the tab is still open");
    assert_eq!(owner_rows(&service)?, 1, "owner row must stay for retry");
    assert_eq!(
        claims_held_by(&service, session)?,
        1,
        "claim must stay held"
    );
    Ok(())
}

/// An endpoint that cannot be reached proves nothing about the tab: the owner
/// row and the claim must stay, and the error is the unreachable code, not a
/// reclaim.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_fail_unreachable_endpoint_is_not_reclaimed_as_absent() -> anyhow::Result<()> {
    // Reserve a loopback port, then close it so nothing listens there.
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        listener.local_addr()?.port()
    };
    let endpoint = format!("http://127.0.0.1:{port}");
    let dir = TempDir::new()?;
    let service = service_with_temp_db(dir.path())?;
    let session = "i211-unreachable";
    let (owner, owner_key) = claimed_session(&service, session, &endpoint)?;

    let outcome = service
        .cdp_close_tab_impl(session, TARGET, &owner_key, owner)
        .await;

    let error = outcome.expect_err("an unreachable endpoint must not reclaim");
    assert_eq!(
        error_code(&error),
        Some(synapse_core::error_codes::A11Y_CDP_UNREACHABLE),
        "{error:?}"
    );
    assert_eq!(owner_rows(&service)?, 1, "owner row must stay for retry");
    assert_eq!(
        claims_held_by(&service, session)?,
        1,
        "claim must stay held"
    );
    Ok(())
}

#[cfg(test)]
#[path = "teardown_endpoint_211.rs"]
mod teardown_endpoint_211;
