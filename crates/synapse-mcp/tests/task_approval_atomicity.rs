//! Regression coverage for concurrent task, approval, and mailbox mutations (#31).
//!
//! The test uses distinct Streamable HTTP MCP sessions against one daemon and
//! verifies public facade outcomes plus the final physical `CF_KV` rows.

use std::{
    net::TcpListener,
    path::Path,
    process::Stdio,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use anyhow::{Context, ensure};
use futures_util::future::join_all;
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use serde_json::{Value, json};
use synapse_core::SCHEMA_VERSION;
use synapse_storage::{Db, cf};
use tokio::process::{Child, Command};

const TOKEN: &str = "syn31-atomicity-test-token";
const PROTOCOL_VERSION: &str = "2025-06-18";
const RACE_ROUNDS: usize = 16;
const CONTENDERS: usize = 8;

struct HttpMcpClient {
    endpoint: String,
    token: String,
    session_id: String,
    client: reqwest::Client,
    request_id: AtomicU64,
}

impl HttpMcpClient {
    async fn connect(endpoint: String, token: &str) -> anyhow::Result<Self> {
        let client = reqwest::Client::new();
        let initialize = client
            .post(&endpoint)
            .bearer_auth(token)
            .header(ACCEPT, "application/json, text/event-stream")
            .header(CONTENT_TYPE, "application/json")
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {"name": "task-approval-atomicity-test", "version": "1.0"}
                }
            }))
            .send()
            .await
            .context("initialize isolated MCP daemon")?;
        let status = initialize.status();
        let session_id = initialize
            .headers()
            .get("mcp-session-id")
            .context("initialize response missing Mcp-Session-Id")?
            .to_str()
            .context("decode Mcp-Session-Id")?
            .to_owned();
        let initialize_body = decode_mcp_response(initialize, "initialize").await?;
        ensure!(
            status.is_success() && initialize_body.get("error").is_none(),
            "initialize failed: status={status} body={initialize_body}"
        );

        let initialized = client
            .post(&endpoint)
            .bearer_auth(token)
            .header(ACCEPT, "application/json, text/event-stream")
            .header(CONTENT_TYPE, "application/json")
            .header("Mcp-Session-Id", &session_id)
            .header("MCP-Protocol-Version", PROTOCOL_VERSION)
            .json(&json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized",
                "params": {}
            }))
            .send()
            .await
            .context("send initialized notification")?;
        ensure!(
            initialized.status().is_success(),
            "initialized notification failed: {}",
            initialized.status()
        );

        Ok(Self {
            endpoint,
            token: token.to_owned(),
            session_id,
            client,
            request_id: AtomicU64::new(2),
        })
    }

    async fn rpc(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        let body = self.rpc_body(method, params).await?;
        if let Some(error) = body.get("error") {
            anyhow::bail!("MCP {method} error: {error}");
        }
        body.get("result")
            .cloned()
            .with_context(|| format!("MCP {method} missing result: {body}"))
    }

    /// The whole JSON-RPC response body of one request (HTTP status checked).
    async fn rpc_body(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        let response = self
            .client
            .post(&self.endpoint)
            .bearer_auth(&self.token)
            .header(ACCEPT, "application/json, text/event-stream")
            .header(CONTENT_TYPE, "application/json")
            .header("Mcp-Session-Id", &self.session_id)
            .header("MCP-Protocol-Version", PROTOCOL_VERSION)
            .json(&json!({
                "jsonrpc": "2.0",
                "id": self.request_id.fetch_add(1, Ordering::Relaxed),
                "method": method,
                "params": params,
            }))
            .send()
            .await
            .with_context(|| format!("send MCP {method}"))?;
        let status = response.status();
        let body = decode_mcp_response(response, method).await?;
        ensure!(status.is_success(), "MCP {method} HTTP {status}: {body}");
        Ok(body)
    }

    async fn tools_list(&self) -> anyhow::Result<Value> {
        self.rpc("tools/list", json!({})).await
    }

    async fn tool_call(&self, name: &str, arguments: Value) -> anyhow::Result<Value> {
        let result = self
            .rpc("tools/call", json!({"name": name, "arguments": arguments}))
            .await?;
        ensure!(
            result.get("isError") != Some(&Value::Bool(true)),
            "tools/call {name} returned an error result: {result}"
        );
        Ok(result)
    }

    /// Contended call: `Ok(Ok(result))` on success, `Ok(Err(error))` when the
    /// daemon refused it with a JSON-RPC error. Transport failures, timeouts
    /// and `isError` results are the outer `Err`, so a race loser can never be
    /// mistaken for a correct refusal.
    async fn tool_call_outcome(
        &self,
        name: &str,
        arguments: Value,
    ) -> anyhow::Result<Result<Value, Value>> {
        let body = self
            .rpc_body("tools/call", json!({"name": name, "arguments": arguments}))
            .await?;
        if let Some(error) = body.get("error") {
            return Ok(Err(error.clone()));
        }
        let result = body
            .get("result")
            .cloned()
            .with_context(|| format!("MCP tools/call {name} missing result: {body}"))?;
        ensure!(
            result.get("isError") != Some(&Value::Bool(true)),
            "tools/call {name} returned an error result: {result}"
        );
        Ok(Ok(result))
    }
}

/// Splits contended outcomes into successes, requiring every loser to be a
/// daemon refusal with `expected_code` (and `message_fragment` when given).
fn winners_with_exact_refusals(
    outcomes: Vec<anyhow::Result<Result<Value, Value>>>,
    expected_code: &str,
    message_fragment: Option<&str>,
) -> anyhow::Result<Vec<Value>> {
    let mut successes = Vec::new();
    for outcome in outcomes {
        match outcome? {
            Ok(result) => successes.push(result),
            Err(error) => {
                ensure!(
                    error["data"]["code"] == json!(expected_code),
                    "race loser must be refused with {expected_code}, got {error}"
                );
                if let Some(fragment) = message_fragment {
                    ensure!(
                        error["message"]
                            .as_str()
                            .is_some_and(|message| message.contains(fragment)),
                        "race loser refusal must say `{fragment}`, got {error}"
                    );
                }
            }
        }
    }
    Ok(successes)
}

async fn decode_mcp_response(response: reqwest::Response, method: &str) -> anyhow::Result<Value> {
    let body = response
        .text()
        .await
        .with_context(|| format!("read MCP {method} response"))?;
    let trimmed = body.trim();
    if trimmed.starts_with('{') {
        return serde_json::from_str(trimmed)
            .with_context(|| format!("decode JSON MCP {method} response"));
    }

    let normalized = trimmed.replace("\r\n", "\n");
    for frame in normalized.split("\n\n") {
        let data = frame
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .map(str::trim_start)
            .collect::<Vec<_>>()
            .join("\n");
        if data.starts_with('{') {
            return serde_json::from_str(&data)
                .with_context(|| format!("decode SSE MCP {method} response"));
        }
    }

    anyhow::bail!("MCP {method} response did not contain a JSON-RPC frame")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn must_fail_task_claim_and_approval_decision_have_one_concurrent_winner()
-> anyhow::Result<()> {
    let directory = tempfile::Builder::new()
        .prefix("synapse-task-approval-atomicity")
        .tempdir()?;
    let db_path = directory.path().join("db");
    let bind = free_loopback_bind()?;
    let endpoint = format!("http://{bind}/mcp");
    let mut daemon = start_daemon(directory.path(), &db_path, &bind)?;

    let test_result: anyhow::Result<()> = async {
        wait_for_health(&bind).await?;
        let reader = HttpMcpClient::connect(endpoint.clone(), TOKEN).await?;
        assert_public_facades(&reader.tools_list().await?)?;
        let workers =
            join_all((0..CONTENDERS).map(|_index| HttpMcpClient::connect(endpoint.clone(), TOKEN)))
                .await
                .into_iter()
                .collect::<anyhow::Result<Vec<_>>>()?;

        for round in 0..RACE_ROUNDS {
            assert_single_task_claim_winner(&reader, &workers, round).await?;
            assert_single_task_update_winner(&reader, &workers, round).await?;
            assert_single_approval_decision_winner(&reader, &workers, round).await?;
            assert_single_mailbox_drain(&reader, &workers[0], round).await?;
        }
        Ok(())
    }
    .await;

    let stop_result = stop_daemon(&mut daemon).await;
    test_result?;
    stop_result?;
    verify_physical_rows(&db_path)?;
    Ok(())
}

/// Uncontended baseline for the same facades: one session claims a fresh task,
/// accepts one approval, and drains one mailbox message. Each mutation succeeds
/// once, its returned readback matches a separate read, and the final physical
/// `CF_KV` rows hold exactly that state.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn must_pass_uncontended_task_claim_approval_decision_and_mailbox_drain() -> anyhow::Result<()>
{
    let directory = tempfile::Builder::new()
        .prefix("synapse-task-approval-baseline")
        .tempdir()?;
    let db_path = directory.path().join("db");
    let bind = free_loopback_bind()?;
    let endpoint = format!("http://{bind}/mcp");
    let mut daemon = start_daemon(directory.path(), &db_path, &bind)?;

    let test_result: anyhow::Result<()> = async {
        wait_for_health(&bind).await?;
        let reader = HttpMcpClient::connect(endpoint.clone(), TOKEN).await?;
        let worker = HttpMcpClient::connect(endpoint.clone(), TOKEN).await?;
        assert_public_facades(&reader.tools_list().await?)?;

        let task_id = "syn31-baseline-task";
        reader
            .tool_call(
                "task",
                json!({
                    "operation": "create",
                    "create": {
                        "task_id": task_id,
                        "title": "Synthetic uncontended task claim",
                        "template_id": "syn31-no-dispatch-template"
                    }
                }),
            )
            .await?;
        let claimed = worker
            .tool_call(
                "task",
                json!({
                    "operation": "claim",
                    "claim": {"task_id": task_id, "session_id": "syn31-baseline-worker"}
                }),
            )
            .await?;
        ensure!(
            claimed.pointer("/structuredContent/claim/task/attempts/0/session_id")
                == Some(&json!("syn31-baseline-worker")),
            "claim must return the claimer as owner: {claimed}"
        );
        let persisted = reader
            .tool_call(
                "task",
                json!({"operation": "get", "get": {"task_id": task_id}}),
            )
            .await?;
        let task = persisted
            .pointer("/structuredContent/get/task")
            .with_context(|| format!("task get response missing task: {persisted}"))?;
        ensure!(
            task["state"] == json!("in_progress")
                && task["attempts"].as_array().map(Vec::len) == Some(1)
                && task["attempts"][0]["session_id"] == json!("syn31-baseline-worker"),
            "claimed task must persist one in-progress attempt for the claimer: {task}"
        );

        let created = reader
            .tool_call(
                "approval",
                json!({
                    "operation": "request",
                    "request": {
                        "kind": "suggestion",
                        "title": "Synthetic uncontended approval decision",
                        "body": "Isolated regression input",
                        "notify": false,
                        "suppress_popup": true
                    }
                }),
            )
            .await?;
        let approval_id = created
            .pointer("/structuredContent/request/item/approval_id")
            .and_then(Value::as_str)
            .context("approval request missing approval_id")?
            .to_owned();
        let decided = worker
            .tool_call(
                "approval",
                json!({
                    "operation": "decide",
                    "decide": {"approval_id": approval_id, "decision": "accept"}
                }),
            )
            .await?;
        ensure!(
            decided.pointer("/structuredContent/decide/after_status") == Some(&json!("accepted")),
            "accept must report after_status=accepted: {decided}"
        );
        let listed = reader
            .tool_call(
                "approval",
                json!({"operation": "list", "list": {"include_terminal": true, "limit": 200}}),
            )
            .await?;
        let persisted_status = listed
            .pointer("/structuredContent/list/items")
            .and_then(Value::as_array)
            .and_then(|items| {
                items.iter().find(|row| {
                    row.pointer("/item/approval_id") == Some(&Value::String(approval_id.clone()))
                })
            })
            .and_then(|row| row.pointer("/item/status"))
            .and_then(Value::as_str)
            .with_context(|| {
                format!("approval {approval_id} absent from list readback: {listed}")
            })?;
        ensure!(
            persisted_status == "accepted",
            "approval {approval_id} persisted {persisted_status}, expected accepted"
        );

        assert_single_mailbox_drain(&reader, &worker, 0).await?;
        Ok(())
    }
    .await;

    let stop_result = stop_daemon(&mut daemon).await;
    test_result?;
    stop_result?;

    let db =
        Db::open(&db_path, SCHEMA_VERSION).context("open isolated RocksDB after daemon exit")?;
    let task_rows = db
        .scan_cf_prefix(cf::CF_KV, b"agent-task/v1/task/")
        .context("scan physical task rows")?;
    ensure!(
        task_rows.len() == 1,
        "expected 1 physical task row, got {}",
        task_rows.len()
    );
    let task: Value = serde_json::from_slice(&task_rows[0].1).context("decode task row")?;
    ensure!(
        task["state"] == json!("in_progress")
            && task["attempts"].as_array().map(Vec::len) == Some(1),
        "physical task row must hold the single claim: {task}"
    );
    let approval_rows = db
        .scan_cf_prefix(cf::CF_KV, b"approval/v1/item/")
        .context("scan physical approval rows")?;
    ensure!(
        approval_rows.len() == 1,
        "expected 1 physical approval row, got {}",
        approval_rows.len()
    );
    let approval: Value =
        serde_json::from_slice(&approval_rows[0].1).context("decode approval row")?;
    ensure!(
        approval["status"] == json!("accepted"),
        "physical approval row must be accepted: {approval}"
    );
    Ok(())
}

async fn assert_single_mailbox_drain(
    recipient: &HttpMcpClient,
    sender: &HttpMcpClient,
    round: usize,
) -> anyhow::Result<()> {
    sender
        .tool_call(
            "agent",
            json!({
                "operation": "send",
                "send": {
                    "to_session": recipient.session_id,
                    "kind": "finding",
                    "payload": {"round": round},
                    "request_receipt": true
                }
            }),
        )
        .await?;

    let outcomes = join_all((0..CONTENDERS).map(|_| {
        recipient.tool_call(
            "agent",
            json!({"operation": "inbox", "inbox": {"drain": true}}),
        )
    }))
    .await;
    let mut deliveries = Vec::new();
    for outcome in outcomes {
        let response = outcome?;
        let inbox = response
            .pointer("/structuredContent/inbox")
            .with_context(|| format!("agent inbox response missing readback: {response}"))?;
        if let Some(messages) = inbox["messages"].as_array() {
            deliveries.extend(messages.iter().cloned());
        }
    }
    ensure!(
        deliveries.len() == 1,
        "mailbox round {round} delivered {} copies of one row: {deliveries:?}",
        deliveries.len()
    );

    let after = recipient
        .tool_call(
            "agent",
            json!({"operation": "inbox", "inbox": {"drain": false}}),
        )
        .await?;
    ensure!(
        after.pointer("/structuredContent/inbox/returned_count") == Some(&json!(0)),
        "mailbox round {round} still contains the drained row: {after}"
    );
    Ok(())
}

async fn assert_single_task_claim_winner(
    reader: &HttpMcpClient,
    workers: &[HttpMcpClient],
    round: usize,
) -> anyhow::Result<()> {
    let task_id = format!("syn31-task-{round:02}");
    reader
        .tool_call(
            "task",
            json!({
                "operation": "create",
                "create": {
                    "task_id": task_id,
                    "title": "Synthetic concurrent task claim",
                    "template_id": "syn31-no-dispatch-template"
                }
            }),
        )
        .await?;

    let outcomes = join_all(workers.iter().enumerate().map(|(worker, client)| {
        client.tool_call_outcome(
            "task",
            json!({
                "operation": "claim",
                "claim": {"task_id": task_id, "session_id": format!("syn31-worker-{worker}")}
            }),
        )
    }))
    .await;
    let successes = winners_with_exact_refusals(
        outcomes,
        synapse_core::error_codes::AGENT_TASK_INVALID_TRANSITION,
        None,
    )?;
    ensure!(
        successes.len() == 1,
        "task {task_id} must produce one claim success, got {}",
        successes.len()
    );

    let persisted = reader
        .tool_call(
            "task",
            json!({"operation": "get", "get": {"task_id": task_id}}),
        )
        .await?;
    let task = persisted
        .pointer("/structuredContent/get/task")
        .with_context(|| format!("task get response missing task: {persisted}"))?;
    let attempts = task["attempts"]
        .as_array()
        .context("persisted task attempts must be an array")?;
    ensure!(
        attempts.len() == 1 && task["state"] == json!("in_progress"),
        "task {task_id} must persist one in-progress attempt: {task}"
    );
    let persisted_owner = attempts[0]["session_id"]
        .as_str()
        .context("persisted task attempt missing session_id")?;
    let returned_owner = successes[0]
        .pointer("/structuredContent/claim/task/attempts/0/session_id")
        .and_then(Value::as_str)
        .with_context(|| format!("claim response missing owner: {}", successes[0]))?;
    ensure!(
        returned_owner == persisted_owner,
        "task {task_id} returned owner {returned_owner} but persisted {persisted_owner}"
    );
    Ok(())
}

async fn assert_single_approval_decision_winner(
    reader: &HttpMcpClient,
    workers: &[HttpMcpClient],
    round: usize,
) -> anyhow::Result<()> {
    let created = reader
        .tool_call(
            "approval",
            json!({
                "operation": "request",
                "request": {
                    "kind": "suggestion",
                    "title": format!("Synthetic concurrent approval decision {round}"),
                    "body": "Isolated regression input",
                    "notify": false,
                    "suppress_popup": true
                }
            }),
        )
        .await?;
    let approval_id = created
        .pointer("/structuredContent/request/item/approval_id")
        .and_then(Value::as_str)
        .context("approval request missing approval_id")?
        .to_owned();

    let outcomes = join_all(workers.iter().enumerate().map(|(worker, client)| {
        client.tool_call_outcome(
            "approval",
            json!({
                "operation": "decide",
                "decide": {
                    "approval_id": approval_id,
                    "decision": if worker % 2 == 0 { "accept" } else { "decline" }
                }
            }),
        )
    }))
    .await;
    let successes = winners_with_exact_refusals(
        outcomes,
        synapse_core::error_codes::TOOL_PARAMS_INVALID,
        Some("cannot change terminal approval"),
    )?;
    ensure!(
        successes.len() == 1,
        "approval {approval_id} must produce one decision success, got {}",
        successes.len()
    );

    let listed = reader
        .tool_call(
            "approval",
            json!({"operation": "list", "list": {"include_terminal": true, "limit": 200}}),
        )
        .await?;
    let items = listed
        .pointer("/structuredContent/list/items")
        .and_then(Value::as_array)
        .with_context(|| format!("approval list missing items: {listed}"))?;
    let persisted = items
        .iter()
        .find(|row| row.pointer("/item/approval_id") == Some(&Value::String(approval_id.clone())))
        .and_then(|row| row.get("item"))
        .with_context(|| format!("approval {approval_id} absent from separate list readback"))?;
    let returned_status = successes[0]
        .pointer("/structuredContent/decide/after_status")
        .and_then(Value::as_str)
        .with_context(|| format!("decision response missing after_status: {}", successes[0]))?;
    let persisted_status = persisted["status"]
        .as_str()
        .context("persisted approval missing status")?;
    ensure!(
        returned_status == persisted_status,
        "approval {approval_id} returned {returned_status} but persisted {persisted_status}"
    );
    Ok(())
}

async fn assert_single_task_update_winner(
    reader: &HttpMcpClient,
    workers: &[HttpMcpClient],
    round: usize,
) -> anyhow::Result<()> {
    let task_id = format!("syn31-update-{round:02}");
    reader
        .tool_call(
            "task",
            json!({
                "operation": "create",
                "create": {
                    "task_id": task_id,
                    "title": "Synthetic concurrent task update",
                    "template_id": "syn31-no-dispatch-template"
                }
            }),
        )
        .await?;
    reader
        .tool_call(
            "task",
            json!({
                "operation": "claim",
                "claim": {"task_id": task_id, "session_id": "syn31-update-owner"}
            }),
        )
        .await?;

    // These two legal exits distinguish a serialized transition chain from two
    // stale writes based on the same pending attempt. `review -> cancelled` is
    // legal, but its existing succeeded attempt must not be rewritten as failed.
    let outcomes = join_all(workers.iter().take(2).enumerate().map(|(worker, client)| {
        client.tool_call_outcome(
            "task",
            json!({
                "operation": "update",
                "update": {
                    "task_id": task_id,
                    "state": if worker % 2 == 0 { "review" } else { "cancelled" },
                    "reason": format!("synthetic update {worker}")
                }
            }),
        )
    }))
    .await;
    let successes = winners_with_exact_refusals(
        outcomes,
        synapse_core::error_codes::AGENT_TASK_INVALID_TRANSITION,
        None,
    )?;
    ensure!(
        !successes.is_empty() && successes.len() <= 2,
        "task {task_id} must return one or two serialized update successes, got {}",
        successes.len(),
    );

    let persisted = reader
        .tool_call(
            "task",
            json!({"operation": "get", "get": {"task_id": task_id}}),
        )
        .await?;
    let task = persisted
        .pointer("/structuredContent/get/task")
        .with_context(|| format!("task get response missing update task: {persisted}"))?;
    let returned_states = successes
        .iter()
        .map(|response| {
            response
                .pointer("/structuredContent/update/task/state")
                .and_then(Value::as_str)
                .with_context(|| format!("update response missing state: {response}"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let persisted_state = task["state"]
        .as_str()
        .context("persisted update task missing state")?;
    let attempt_outcome = task["attempts"]
        .as_array()
        .and_then(|attempts| attempts.first())
        .and_then(|attempt| attempt["outcome"].as_str())
        .context("persisted update task missing settled attempt")?;
    if returned_states.len() == 2 {
        ensure!(
            returned_states.contains(&"review") && returned_states.contains(&"cancelled"),
            "task {task_id} returned invalid serialized update states: {returned_states:?}"
        );
        ensure!(
            persisted_state == "cancelled" && attempt_outcome == "succeeded",
            "task {task_id} must retain the reviewed attempt after review -> cancelled, persisted {persisted_state} with {attempt_outcome}"
        );
    } else {
        let returned_state = returned_states[0];
        let expected_outcome = if returned_state == "review" {
            "succeeded"
        } else {
            "failed"
        };
        ensure!(
            returned_state == persisted_state && attempt_outcome == expected_outcome,
            "task {task_id} returned {returned_state}, persisted {persisted_state} with {attempt_outcome}"
        );
    }
    Ok(())
}

fn assert_public_facades(response: &Value) -> anyhow::Result<()> {
    let tools = response["tools"]
        .as_array()
        .context("tools/list missing tools array")?;
    for name in ["task", "approval", "agent"] {
        let tool = tools
            .iter()
            .find(|tool| tool["name"] == json!(name))
            .with_context(|| format!("tools/list missing public {name} facade"))?;
        ensure!(
            tool["inputSchema"]["type"] == json!("object")
                && tool["inputSchema"]["properties"]["operation"].is_object(),
            "public {name} schema must expose a typed operation object: {tool}"
        );
    }
    Ok(())
}

fn start_daemon(directory: &Path, db_path: &Path, bind: &str) -> anyhow::Result<Child> {
    let db = db_path.to_string_lossy().into_owned();
    Command::new(env!("CARGO_BIN_EXE_synapse-mcp"))
        .args([
            "--mode",
            "http",
            "--bind",
            bind,
            "--db",
            &db,
            "--log-level",
            "warn",
        ])
        .env("APPDATA", directory)
        .env("LOCALAPPDATA", directory)
        .env("SYNAPSE_LOG_DIR", directory)
        .env("SYNAPSE_BEARER_TOKEN", TOKEN)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("start isolated synapse-mcp daemon")
}

async fn wait_for_health(bind: &str) -> anyhow::Result<()> {
    let endpoint = format!("http://{bind}/health");
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let result = client.get(&endpoint).bearer_auth(TOKEN).send().await;
        if result
            .as_ref()
            .is_ok_and(|response| response.status().is_success())
        {
            return Ok(());
        }
        if tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        }
        match result {
            Ok(response) => anyhow::bail!("isolated daemon health returned {}", response.status()),
            Err(error) => return Err(error).context("reach isolated daemon health"),
        }
    }
}

async fn stop_daemon(daemon: &mut Child) -> anyhow::Result<()> {
    let _result = daemon.start_kill();
    tokio::time::timeout(Duration::from_secs(5), daemon.wait())
        .await
        .context("timed out stopping isolated daemon")?
        .context("wait for isolated daemon exit")?;
    Ok(())
}

fn verify_physical_rows(db_path: &Path) -> anyhow::Result<()> {
    let db =
        Db::open(db_path, SCHEMA_VERSION).context("open isolated RocksDB after daemon exit")?;
    let task_rows = db
        .scan_cf_prefix(cf::CF_KV, b"agent-task/v1/task/")
        .context("scan physical task rows")?;
    ensure!(
        task_rows.len() == RACE_ROUNDS * 2,
        "expected {} physical task rows, got {}",
        RACE_ROUNDS * 2,
        task_rows.len()
    );
    for (_key, value) in task_rows {
        let task: Value = serde_json::from_slice(&value).context("decode physical task row")?;
        ensure!(
            task["attempts"]
                .as_array()
                .is_some_and(|attempts| attempts.len() == 1),
            "physical task row must contain one attempt: {task}"
        );
    }

    let approval_rows = db
        .scan_cf_prefix(cf::CF_KV, b"approval/v1/item/")
        .context("scan physical approval rows")?;
    ensure!(
        approval_rows.len() == RACE_ROUNDS,
        "expected {RACE_ROUNDS} physical approval rows, got {}",
        approval_rows.len()
    );
    for (_key, value) in approval_rows {
        let approval: Value =
            serde_json::from_slice(&value).context("decode physical approval row")?;
        ensure!(
            matches!(approval["status"].as_str(), Some("accepted" | "declined")),
            "physical approval row must be terminal: {approval}"
        );
    }
    Ok(())
}

fn free_loopback_bind() -> anyhow::Result<String> {
    let listener = TcpListener::bind("127.0.0.1:0").context("allocate loopback port")?;
    let bind = listener
        .local_addr()
        .context("read allocated loopback port")?
        .to_string();
    drop(listener);
    Ok(bind)
}
