//! `agent_query` tool integration regression (#911): real daemon binary, real
//! `RocksDB`, real MCP JSON-RPC. Plants a synthetic agent's `CF_AGENT_EVENTS`
//! journal and `CF_AGENT_TRANSCRIPTS` rows physically, launches the daemon over
//! that exact database, then drives `agent_query` through the wire and
//! reconciles every reconstructed field against the rows that were planted.
//! Ends with a physical readback proving the read-only queries wrote nothing:
//! the journal and transcript row counts equal what was planted.

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, ensure};
use serde_json::{Value, json};
use synapse_core::{
    AgentEventKind, AgentEventRecord, AgentTranscriptRecord, SCHEMA_VERSION, TranscriptParseStatus,
    TranscriptRole, TranscriptSource, TranscriptUsage,
};
use synapse_storage::{
    Db, agent_events::agent_event_key, agent_transcripts::agent_transcript_key, cf, decode_json,
};
use synapse_test_utils::stdio_mcp_client::StdioMcpClient;
use tempfile::TempDir;

const SPAWN: &str = "agent-spawn-query-1";
const SESSION: &str = "session-query-1";

fn now_ns() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    u64::try_from(nanos).unwrap_or(u64::MAX)
}

fn structured(result: &Value) -> anyhow::Result<Value> {
    result
        .get("structuredContent")
        .cloned()
        .with_context(|| format!("missing structuredContent in {result}"))
}

/// `(CF_AGENT_EVENTS rows, CF_AGENT_TRANSCRIPTS rows)` physically on disk.
fn agent_row_counts(db: &Db) -> anyhow::Result<(usize, usize)> {
    Ok((
        db.scan_cf(cf::CF_AGENT_EVENTS)?.len(),
        db.scan_cf(cf::CF_AGENT_TRANSCRIPTS)?.len(),
    ))
}

fn plant_event(
    db: &Db,
    ts_ns: u64,
    seq: u32,
    kind: AgentEventKind,
    spawn_id: Option<&str>,
    session_id: Option<&str>,
    decorate: impl FnOnce(&mut AgentEventRecord),
) -> anyhow::Result<()> {
    let mut record = AgentEventRecord::new(ts_ns, kind);
    record.spawn_id = spawn_id.map(ToOwned::to_owned);
    record.session_id = session_id.map(ToOwned::to_owned);
    decorate(&mut record);
    record
        .validate()
        .map_err(|error| anyhow::anyhow!("planted journal row valid: {error}"))?;
    let value = serde_json::to_vec(&record).context("serialize journal row")?;
    db.put_batch_pressure_bypass(cf::CF_AGENT_EVENTS, [(agent_event_key(ts_ns, seq), value)])
        .context("write journal row")?;
    Ok(())
}

#[tokio::test]
async fn must_pass_agent_query_tool_reconstructs_a_planted_agent_end_to_end() -> anyhow::Result<()>
{
    let temp = TempDir::new()?;
    let db_path = temp.path().join("db");

    // ---- Plant the source of truth, then release the RocksDB lock. ----
    let base = now_ns().saturating_sub(60_000_000_000); // ~60s ago, inside lookback
    {
        let db = Db::open(&db_path, SCHEMA_VERSION)?;
        plant_event(
            &db,
            base + 1,
            0,
            AgentEventKind::SpawnRequested,
            Some(SPAWN),
            None,
            |_| {},
        )?;
        plant_event(
            &db,
            base + 2,
            0,
            AgentEventKind::SpawnReady,
            Some(SPAWN),
            Some(SESSION),
            |_| {},
        )?;
        plant_event(
            &db,
            base + 3,
            0,
            AgentEventKind::TurnStarted,
            Some(SPAWN),
            Some(SESSION),
            |_| {},
        )?;
        plant_event(
            &db,
            base + 4,
            0,
            AgentEventKind::ToolCallStarted,
            Some(SPAWN),
            Some(SESSION),
            |record| {
                record.attributes.tool_name = Some("Grep".to_owned());
                record.attributes.tool_call_id = Some("call-grep".to_owned());
                record.payload =
                    json!({"tool_input_bytes": 19, "tool_input_sha256": "c".repeat(64)});
            },
        )?;
        plant_event(
            &db,
            base + 5,
            0,
            AgentEventKind::ToolCallFinished,
            Some(SPAWN),
            Some(SESSION),
            |record| {
                record.attributes.tool_name = Some("Grep".to_owned());
                record.attributes.tool_call_id = Some("call-grep".to_owned());
                record.payload = json!({"duration_ms": 8});
            },
        )?;
        // Currently inside an Edit tool.
        plant_event(
            &db,
            base + 6,
            0,
            AgentEventKind::ToolCallStarted,
            Some(SPAWN),
            Some(SESSION),
            |record| {
                record.attributes.tool_name = Some("Edit".to_owned());
                record.attributes.tool_call_id = Some("call-edit".to_owned());
                record.payload =
                    json!({"tool_input_bytes": 55, "tool_input_sha256": "d".repeat(64)});
            },
        )?;

        // Transcript: one assistant line with known usage.
        let mut tr = AgentTranscriptRecord::new(
            base + 4,
            SPAWN.to_owned(),
            1,
            TranscriptSource::ClaudeStreamJson,
            16,
            "a".repeat(64),
        );
        tr.status = TranscriptParseStatus::Parsed;
        tr.role = Some(TranscriptRole::Assistant);
        tr.event_kind = Some("assistant".to_owned());
        tr.model = Some("claude-fable-5".to_owned());
        tr.turn_index = Some(1);
        tr.content_summary = Some("Patching the retry loop in the client.".to_owned());
        tr.usage = Some(TranscriptUsage {
            input_tokens: Some(800),
            output_tokens: Some(150),
            cache_read_input_tokens: Some(12_000),
            cache_creation_input_tokens: Some(400),
            cache_creation_5m_input_tokens: None,
            cache_creation_1h_input_tokens: None,
            reasoning_output_tokens: None,
            total_cost_micro_usd: None,
            model_usage: Vec::new(),
        });
        tr.validate()
            .map_err(|error| anyhow::anyhow!("planted transcript row valid: {error}"))?;
        let value = serde_json::to_vec(&tr).context("serialize transcript row")?;
        db.put_batch_pressure_bypass(
            cf::CF_AGENT_TRANSCRIPTS,
            [(agent_transcript_key(SPAWN, 1), value)],
        )
        .context("write transcript row")?;
    } // db dropped -> lock released
    let planted_counts = agent_row_counts(&Db::open(&db_path, SCHEMA_VERSION)?)?;

    // ---- Launch the real daemon over that database. ----
    let db_path_string = db_path.to_string_lossy().into_owned();
    let mut client =
        StdioMcpClient::launch_and_init_with_env(None, &[("SYNAPSE_DB", db_path_string.as_str())])
            .await?;

    // ---- Drive agent_query through the wire. ----
    let response = structured(
        &client
            .tools_call("agent_query", json!({"session_id": SESSION}))
            .await?,
    )?;

    ensure!(
        response["found"] == json!(true),
        "agent must be found: {response}"
    );
    ensure!(response["spawn_id"] == json!(SPAWN), "spawn_id: {response}");
    ensure!(
        response["session_id"] == json!(SESSION),
        "session_id: {response}"
    );
    ensure!(
        response["state"] == json!("working"),
        "state must be working: {response}"
    );

    // Current = in-flight Edit; last completed = Grep (8ms).
    ensure!(
        response["current_tool_call"]["tool_name"] == json!("Edit"),
        "current tool: {response}"
    );
    ensure!(
        response["current_tool_call"]["in_flight"] == json!(true),
        "current in_flight: {response}"
    );
    ensure!(
        response["last_tool_call"]["tool_name"] == json!("Grep"),
        "last tool: {response}"
    );
    ensure!(
        response["last_tool_call"]["elapsed_ms"] == json!(8),
        "last tool elapsed: {response}"
    );

    // Recent events: all six planted, oldest first.
    let events = response["recent_events"]
        .as_array()
        .context("recent_events array")?;
    ensure!(
        events.len() == 6,
        "expected 6 events, got {}: {response}",
        events.len()
    );
    ensure!(
        events[0]["kind"] == json!("spawn_requested"),
        "first event: {response}"
    );

    // Tokens this turn reconcile exactly with the planted usage row.
    ensure!(
        response["turn"]["input_tokens"] == json!(800),
        "input tokens: {response}"
    );
    ensure!(
        response["turn"]["output_tokens"] == json!(150),
        "output tokens: {response}"
    );
    ensure!(
        response["turn"]["cache_read_input_tokens"] == json!(12_000),
        "cache read: {response}"
    );
    ensure!(
        response["turn"]["total_tokens"] == json!(800 + 150 + 12_000 + 400),
        "total tokens: {response}"
    );
    ensure!(
        response["context_window_estimate_tokens"] == json!(800 + 12_000 + 400 + 150),
        "context estimate: {response}"
    );
    ensure!(
        response["activity_summary"] == json!("Patching the retry loop in the client."),
        "activity summary: {response}"
    );

    // task is null (not fabricated); its source names #910.
    ensure!(
        response["task"] == json!(null),
        "task must be null: {response}"
    );
    ensure!(
        response["sources"]["task"]
            .as_str()
            .is_some_and(|s| s.contains("#910")),
        "task source names #910: {response}"
    );

    // An unknown session is honestly empty, not an error and not fabricated.
    let unknown = structured(
        &client
            .tools_call("agent_query", json!({"session_id": "session-nope"}))
            .await?,
    )?;
    ensure!(
        unknown["found"] == json!(false),
        "unknown session found=false: {unknown}"
    );
    ensure!(
        unknown["recent_events"]
            .as_array()
            .is_some_and(Vec::is_empty),
        "unknown session has no events: {unknown}"
    );

    let status = client.shutdown().await?;
    ensure!(status.success(), "daemon shut down cleanly");

    // ---- Physical readback: agent_query is read-only. The agent journal and
    // transcript column families hold exactly what was planted before the
    // daemon started; the queries wrote nothing. ----
    let reopened = Db::open(&db_path, SCHEMA_VERSION)?;
    let after = agent_row_counts(&reopened)?;
    ensure!(
        after == planted_counts,
        "agent_query must not write: planted (events, transcripts)={planted_counts:?}, after={after:?}"
    );
    let (rows, _more) = reopened.scan_cf_from(cf::CF_AGENT_EVENTS, &[], 1_000)?;
    for (_key, value) in &rows {
        let record: AgentEventRecord = decode_json(value).context("decode journal row")?;
        ensure!(
            record.spawn_id.as_deref() == Some(SPAWN)
                || record.session_id.as_deref() == Some(SESSION),
            "unexpected journal row after read-only queries: {record:?}"
        );
    }

    Ok(())
}

/// Missing, blank, out-of-range, or unknown `agent_query` parameters are
/// refused with `TOOL_PARAMS_INVALID` (documented bounds: max_events 1..=200,
/// lookback_ms 1..=604_800_000, deep_timeout_ms 1..=60_000 when deep), never
/// clamped or answered with a fabricated snapshot.
#[tokio::test]
async fn must_fail_agent_query_tool_rejects_invalid_params() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let db_path = temp.path().join("db");
    let db_path_string = db_path.to_string_lossy().into_owned();
    let mut client =
        StdioMcpClient::launch_and_init_with_env(None, &[("SYNAPSE_DB", db_path_string.as_str())])
            .await?;

    // Each refusal names the offending field.
    let invalid_args = [
        (json!({}), "session_id"),
        (json!({"session_id": ""}), "session_id"),
        (json!({"session_id": "   "}), "session_id"),
        (
            json!({"session_id": SESSION, "max_events": 0}),
            "max_events",
        ),
        (
            json!({"session_id": SESSION, "max_events": 201}),
            "max_events",
        ),
        (
            json!({"session_id": SESSION, "lookback_ms": 0}),
            "lookback_ms",
        ),
        (
            json!({"session_id": SESSION, "lookback_ms": 604_800_001_u64}),
            "lookback_ms",
        ),
        (
            json!({"session_id": SESSION, "deep": true, "deep_timeout_ms": 0}),
            "deep_timeout_ms",
        ),
        (
            json!({"session_id": SESSION, "deep": true, "deep_timeout_ms": 60_001}),
            "deep_timeout_ms",
        ),
        (json!({"session_id": SESSION, "max_events": -1}), "-1"),
        (
            json!({"session_id": SESSION, "spawn_id": SPAWN}),
            "spawn_id",
        ),
    ];
    for (args, field) in invalid_args {
        let error = client.tools_call_error("agent_query", args.clone()).await?;
        ensure!(
            error["data"]["code"] == json!("TOOL_PARAMS_INVALID"),
            "agent_query args {args} were not refused as TOOL_PARAMS_INVALID: {error}"
        );
        ensure!(
            error["message"]
                .as_str()
                .is_some_and(|message| message.contains(field)),
            "agent_query args {args}: refusal must name `{field}`: {error}"
        );
    }

    // Boundary values are accepted and answer honestly (nothing planted).
    let boundary = structured(
        &client
            .tools_call(
                "agent_query",
                json!({"session_id": SESSION, "max_events": 200, "lookback_ms": 604_800_000_u64}),
            )
            .await?,
    )?;
    ensure!(
        boundary["found"] == json!(false),
        "nothing planted, found=false: {boundary}"
    );
    ensure!(
        boundary["recent_events"]
            .as_array()
            .is_some_and(Vec::is_empty),
        "nothing planted, no events: {boundary}"
    );

    let status = client.shutdown().await?;
    ensure!(status.success(), "daemon shut down cleanly");
    Ok(())
}
