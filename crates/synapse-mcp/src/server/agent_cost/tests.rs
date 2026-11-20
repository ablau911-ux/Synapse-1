//! Storage-backed tests for the #901 cost tools. These exercise the real
//! `CF_KV` price table and real `CF_AGENT_TRANSCRIPTS` rows through the same
//! daemon storage handle the tools use — no mock storage, no mock usage. Each
//! test writes physical rows and verifies the rollup reconciles with them.

use std::{num::NonZeroUsize, path::Path, path::PathBuf, sync::Arc};

use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::server::agent_transcripts::ingest_spawn_dir_once;
use crate::{m2::M2ServiceConfig, m3::M3ServiceConfig, m4::M4ServiceConfig};
use synapse_core::{
    AgentTranscriptRecord, TranscriptParseStatus, TranscriptRole, TranscriptSource, TranscriptUsage,
};
use synapse_storage::{Db, agent_transcripts::agent_transcript_key, cf};

const CLAUDE_REAL_STREAM: &str = include_str!("../../../tests/fixtures/claude_stream_real.jsonl");
fn service_with_db(path: &Path) -> SynapseService {
    SynapseService::try_with_m2_shutdown_reason_and_m3_config(
        CancellationToken::new(),
        "test",
        CancellationToken::new(),
        &M2ServiceConfig::default(),
        M3ServiceConfig::from_cli_parts(
            Some(path.join("db")),
            Some(path.to_path_buf()),
            false,
            "127.0.0.1:0".to_owned(),
            NonZeroUsize::new(4).expect("nonzero"),
            false,
            true,
            None,
            false,
            None,
        ),
        M4ServiceConfig::default(),
    )
    .expect("construct service")
}

fn db_of(service: &SynapseService) -> Arc<Db> {
    service.agent_cost_db().expect("open storage")
}

/// Builds a parsed transcript row and writes it physically.
#[allow(clippy::too_many_arguments)]
fn write_row(
    db: &Db,
    spawn_id: &str,
    line_no: u64,
    ts_ns: u64,
    source: TranscriptSource,
    event_kind: &str,
    model: Option<&str>,
    role: TranscriptRole,
    usage: Option<TranscriptUsage>,
) {
    let mut record = AgentTranscriptRecord::new(
        ts_ns,
        spawn_id.to_owned(),
        line_no,
        source,
        16,
        "a".repeat(64),
    );
    record.status = TranscriptParseStatus::Parsed;
    record.role = Some(role);
    record.event_kind = Some(event_kind.to_owned());
    record.model = model.map(ToOwned::to_owned);
    record.usage = usage;
    record.validate().expect("synthetic row must be valid");
    let value = serde_json::to_vec(&record).expect("serialize row");
    // Mirror the #900 ingester, which writes transcript rows via the
    // pressure-bypass path so they are immediately durable (plain put_batch
    // enqueues through the async batcher and is not visible until flush).
    db.put_batch_pressure_bypass(
        cf::CF_AGENT_TRANSCRIPTS,
        [(agent_transcript_key(spawn_id, line_no), value)],
    )
    .expect("write transcript row");
}

fn claude_usage(
    input: u64,
    output: u64,
    cr: u64,
    cc: u64,
    cost_micro: Option<u64>,
) -> TranscriptUsage {
    TranscriptUsage {
        input_tokens: Some(input),
        output_tokens: Some(output),
        cache_read_input_tokens: Some(cr),
        cache_creation_input_tokens: Some(cc),
        cache_creation_5m_input_tokens: None,
        cache_creation_1h_input_tokens: None,
        reasoning_output_tokens: None,
        total_cost_micro_usd: cost_micro,
        model_usage: Vec::new(),
    }
}

fn codex_usage(input: u64, output: u64, cached: u64, reasoning: u64) -> TranscriptUsage {
    TranscriptUsage {
        input_tokens: Some(input),
        output_tokens: Some(output),
        cache_read_input_tokens: Some(cached),
        cache_creation_input_tokens: None,
        cache_creation_5m_input_tokens: None,
        cache_creation_1h_input_tokens: None,
        reasoning_output_tokens: Some(reasoning),
        total_cost_micro_usd: None,
        model_usage: Vec::new(),
    }
}

fn fable_price() -> AgentCostPricePutParams {
    AgentCostPricePutParams {
        model_id: "claude-fable-5".to_owned(),
        provider: Some("anthropic".to_owned()),
        input_usd_per_mtok: 3.0,
        output_usd_per_mtok: 15.0,
        cache_read_usd_per_mtok: 0.30,
        cache_creation_usd_per_mtok: 3.75,
        cache_creation_5m_usd_per_mtok: None,
        cache_creation_1h_usd_per_mtok: None,
    }
}

fn cost_params(spawn: Option<&str>, since: Option<u64>, until: Option<u64>) -> AgentCostParams {
    AgentCostParams {
        spawn_id: spawn.map(ToOwned::to_owned),
        since_ns: since,
        until_ns: until,
        include_per_turn: false,
        group_by: Vec::new(),
    }
}

fn cost_params_per_turn(spawn: Option<&str>) -> AgentCostParams {
    AgentCostParams {
        spawn_id: spawn.map(ToOwned::to_owned),
        since_ns: None,
        until_ns: None,
        include_per_turn: true,
        group_by: Vec::new(),
    }
}

fn error_field<'a>(error: &'a ErrorData, field: &str) -> Option<&'a str> {
    error
        .data
        .as_ref()
        .and_then(|data| data.get(field))
        .and_then(serde_json::Value::as_str)
}

#[test]
fn must_pass_cost_facade_accepts_matching_operation_payload() {
    let ok = CostParams {
        operation: "summarize".to_owned(),
        summarize: Some(cost_params(Some("agent-spawn-cost-facade"), None, None)),
        price_list: None,
        price_put: None,
        price_delete: None,
    };
    assert_eq!(
        validate_cost_params(&ok).expect("matching summarize payload accepted"),
        CostOperation::Summarize
    );
}

#[test]
fn must_fail_cost_facade_requires_matching_operation_payload() {
    let missing = CostParams {
        operation: "summarize".to_owned(),
        summarize: None,
        price_list: None,
        price_put: None,
        price_delete: None,
    };
    let mismatched = CostParams {
        operation: "price_list".to_owned(),
        summarize: Some(cost_params(Some("agent-spawn-cost-facade"), None, None)),
        price_list: None,
        price_put: None,
        price_delete: None,
    };
    let extra = CostParams {
        operation: "price_list".to_owned(),
        summarize: Some(cost_params(Some("agent-spawn-cost-facade"), None, None)),
        price_list: Some(AgentCostPriceListParams {}),
        price_put: None,
        price_delete: None,
    };
    for (label, params, operation) in [
        ("missing", missing, "summarize"),
        ("mismatched", mismatched, "price_list"),
        ("extra", extra, "price_list"),
    ] {
        let error = validate_cost_params(&params).expect_err(label);
        assert_eq!(
            error_field(&error, "code"),
            Some(synapse_core::error_codes::TOOL_PARAMS_INVALID),
            "{label}: {error:?}"
        );
        assert_eq!(
            error_field(&error, "operation"),
            Some(operation),
            "{label}: the refusal must name the requested operation: {error:?}"
        );
        assert!(
            error.message.contains(&format!(
                "operation={operation} requires exactly one matching operation payload"
            )),
            "{label}: {}",
            error.message
        );
    }

    let invalid_operation = CostParams {
        operation: "not_real".to_owned(),
        summarize: Some(cost_params(Some("agent-spawn-cost-facade"), None, None)),
        price_list: None,
        price_put: None,
        price_delete: None,
    };
    let error = validate_cost_params(&invalid_operation).expect_err("invalid operation rejected");
    assert_eq!(
        error_field(&error, "operation"),
        Some("not_real"),
        "invalid operation error must carry the bad operation: {error:?}"
    );
    assert!(
        error.message.contains("operation=not_real is invalid"),
        "{}",
        error.message
    );
}

// ---------------------------------------------------------------------------
// Price table CRUD against physical CF_KV rows
// ---------------------------------------------------------------------------

#[test]
fn must_pass_price_put_list_delete_roundtrip_physical_rows() {
    let temp = TempDir::new().expect("tempdir");
    let service = service_with_db(temp.path());
    let db = db_of(&service);

    let mut params = fable_price();
    params.model_id = "  Claude-Fable-5 ".to_owned();
    let put = service
        .agent_cost_price_put_impl(params)
        .expect("put price");
    assert_eq!(put.price.model_id, "claude-fable-5");
    assert_eq!(put.price.input_micro_usd_per_mtok, 3_000_000);
    assert_eq!(put.price.cache_read_micro_usd_per_mtok, 300_000);
    assert_eq!(put.price.cache_creation_micro_usd_per_mtok, 3_750_000);

    // FSV: the row physically exists under the expected key and decodes back.
    let key = price_row_key("claude-fable-5");
    let stored = get_exact_kv_row(&db, &key)
        .expect("read")
        .expect("row present");
    let decoded: ModelPrice = serde_json::from_slice(&stored).expect("decode physical row");
    assert_eq!(decoded, put.price);
    assert_eq!(put.storage_readback.value_len_bytes, stored.len() as u64);

    let list = service.agent_cost_price_list_impl().expect("list");
    assert_eq!(list.count, 1);
    assert_eq!(list.prices[0].model_id, "claude-fable-5");

    let del = service
        .agent_cost_price_delete_impl(AgentCostPriceDeleteParams {
            model_id: "claude-fable-5".to_owned(),
        })
        .expect("delete");
    assert!(del.existed);
    assert!(
        get_exact_kv_row(&db, &key).expect("read").is_none(),
        "price row must be physically gone after delete"
    );

    let del_again = service
        .agent_cost_price_delete_impl(AgentCostPriceDeleteParams {
            model_id: "claude-fable-5".to_owned(),
        })
        .expect("delete idempotent");
    assert!(!del_again.existed);
}

/// Drives the real `price_put` path: a negative or non-finite rate in any field
/// is refused with TOOL_PARAMS_INVALID naming that field, and no price row is
/// written.
#[test]
fn must_fail_put_rejects_negative_and_nonfinite_rates() {
    let temp = TempDir::new().expect("tempdir");
    let service = service_with_db(temp.path());
    let db = db_of(&service);
    let key = price_row_key("claude-fable-5");

    type Setter = fn(&mut AgentCostPricePutParams, f64);
    let fields: [(&str, Setter); 4] = [
        ("input_usd_per_mtok", |p, v| p.input_usd_per_mtok = v),
        ("output_usd_per_mtok", |p, v| p.output_usd_per_mtok = v),
        ("cache_read_usd_per_mtok", |p, v| {
            p.cache_read_usd_per_mtok = v
        }),
        ("cache_creation_usd_per_mtok", |p, v| {
            p.cache_creation_usd_per_mtok = v;
        }),
    ];
    for (field, set) in fields {
        for (bad, reason) in [
            (-1.0, "must be >= 0"),
            (f64::NAN, "must be a finite number"),
            (f64::INFINITY, "must be a finite number"),
            (f64::NEG_INFINITY, "must be a finite number"),
        ] {
            let mut params = fable_price();
            set(&mut params, bad);
            let error = service
                .agent_cost_price_put_impl(params)
                .expect_err("invalid rate must be refused");
            assert_eq!(
                error_field(&error, "code"),
                Some(synapse_core::error_codes::TOOL_PARAMS_INVALID),
                "{field}={bad}: {error:?}"
            );
            assert!(
                error
                    .message
                    .starts_with(&format!("MODEL_PRICE_INVALID: {field} {reason}")),
                "{field}={bad}: {}",
                error.message
            );
            assert!(
                get_exact_kv_row(&db, &key).expect("read").is_none(),
                "{field}={bad}: no price row may be written"
            );
        }
    }
}

/// Boundary: a zero rate is valid (free tier), fractional rates round to micro-USD.
#[test]
fn must_pass_put_accepts_zero_and_fractional_rates() {
    let temp = TempDir::new().expect("tempdir");
    let service = service_with_db(temp.path());
    let mut params = fable_price();
    params.input_usd_per_mtok = 0.0;
    params.output_usd_per_mtok = 0.25;
    let put = service
        .agent_cost_price_put_impl(params)
        .expect("put price");
    assert_eq!(put.price.input_micro_usd_per_mtok, 0);
    assert_eq!(put.price.output_micro_usd_per_mtok, 250_000);
}

// ---------------------------------------------------------------------------
// Claude rollup: result row is authoritative; assistant rows never summed
// ---------------------------------------------------------------------------

#[test]
fn must_pass_claude_bills_result_row_not_partial_assistant_rows() {
    let temp = TempDir::new().expect("tempdir");
    let service = service_with_db(temp.path());
    let db = db_of(&service);
    service
        .agent_cost_price_put_impl(fable_price())
        .expect("price");

    let spawn = "agent-spawn-claude1";
    write_row(
        &db,
        spawn,
        1,
        100,
        TranscriptSource::ClaudeStreamJson,
        "system/init",
        Some("claude-fable-5"),
        TranscriptRole::System,
        None,
    );
    write_row(
        &db,
        spawn,
        2,
        110,
        TranscriptSource::ClaudeStreamJson,
        "assistant",
        Some("claude-fable-5"),
        TranscriptRole::Assistant,
        Some(claude_usage(3684, 6, 20866, 4563, None)),
    );
    write_row(
        &db,
        spawn,
        3,
        120,
        TranscriptSource::ClaudeStreamJson,
        "assistant",
        Some("claude-fable-5"),
        TranscriptRole::Assistant,
        Some(claude_usage(2, 73, 20959, 8899, None)),
    );
    write_row(
        &db,
        spawn,
        4,
        130,
        TranscriptSource::ClaudeStreamJson,
        "result/success",
        Some("claude-fable-5"),
        TranscriptRole::Result,
        Some(claude_usage(4118, 1906, 283040, 22189, Some(864_631))),
    );

    let out = service
        .agent_cost_impl(cost_params(Some(spawn), None, None))
        .expect("rollup");

    assert_eq!(out.scanned_rows, 4);
    assert_eq!(out.per_spawn.len(), 1);
    let s = &out.per_spawn[0];
    assert_eq!(s.status, "complete");
    assert_eq!(s.authoritative_line_no, Some(4));
    // Billed usage == the result row exactly, NOT the sum of assistant rows.
    assert_eq!(s.usage.input_tokens, 4118);
    assert_eq!(s.usage.output_tokens, 1906);
    assert_eq!(s.usage.cache_read_tokens, 283_040);
    assert_eq!(s.usage.cache_creation_tokens, 22_189);

    // Hand-computed micro-USD (rates per Mtok):
    //   input        = 4118   * 3_000_000 /1e6 = 12_354
    //   output       = 1906   * 15_000_000/1e6 = 28_590
    //   cache_read   = 283040 * 300_000   /1e6 = 84_912
    //   cache_create = 22189  * 3_750_000 /1e6 = 83_208 (floor 83208.75)
    //   total = 209_064
    match &s.cost {
        CostOutcome::Priced { cost } => {
            assert_eq!(cost.input_micro_usd, 12_354);
            assert_eq!(cost.output_micro_usd, 28_590);
            assert_eq!(cost.cache_read_micro_usd, 84_912);
            assert_eq!(cost.cache_creation_micro_usd, 83_208);
            assert_eq!(cost.total_micro_usd, 209_064);
        }
        CostOutcome::Unpriced { .. } => panic!("model is priced"),
    }
    assert_eq!(s.source_reported_micro_usd, Some(864_631));
    assert_eq!(s.reconciliation_delta_micro_usd, Some(864_631 - 209_064));

    assert_eq!(out.fleet.spawns_complete, 1);
    assert_eq!(out.fleet.computed_micro_usd, 209_064);
    assert_eq!(out.fleet.source_reported_micro_usd, 864_631);
    assert!(out.fleet.unpriced_models.is_empty());

    // Per-model rollup mirrors the single spawn.
    assert_eq!(out.per_model.len(), 1);
    assert_eq!(out.per_model[0].model, "claude-fable-5");
    assert_eq!(out.per_model[0].computed_micro_usd, Some(209_064));
}

// ---------------------------------------------------------------------------
// Codex rollup: cumulative max, cached subtracted from input
// ---------------------------------------------------------------------------

#[test]
fn must_pass_codex_takes_cumulative_max_and_subtracts_cache() {
    let temp = TempDir::new().expect("tempdir");
    let service = service_with_db(temp.path());
    let db = db_of(&service);
    service
        .agent_cost_price_put_impl(AgentCostPricePutParams {
            model_id: "gpt-5.2".to_owned(),
            provider: Some("openai".to_owned()),
            input_usd_per_mtok: 1.75,
            output_usd_per_mtok: 14.0,
            cache_read_usd_per_mtok: 0.175,
            cache_creation_usd_per_mtok: 0.0,
            cache_creation_5m_usd_per_mtok: None,
            cache_creation_1h_usd_per_mtok: None,
        })
        .expect("price");

    let spawn = "agent-spawn-codex1";
    write_row(
        &db,
        spawn,
        1,
        200,
        TranscriptSource::CodexExecJson,
        "thread.started",
        Some("gpt-5.2"),
        TranscriptRole::System,
        None,
    );
    write_row(
        &db,
        spawn,
        2,
        210,
        TranscriptSource::CodexExecJson,
        "turn.completed",
        Some("gpt-5.2"),
        TranscriptRole::Result,
        Some(codex_usage(50_000, 1_000, 30_000, 500)),
    );
    write_row(
        &db,
        spawn,
        3,
        220,
        TranscriptSource::CodexExecJson,
        "turn.completed",
        Some("gpt-5.2"),
        TranscriptRole::Result,
        Some(codex_usage(144_733, 2_110, 103_296, 1_380)),
    );

    let out = service
        .agent_cost_impl(cost_params(Some(spawn), None, None))
        .expect("rollup");
    let s = &out.per_spawn[0];
    assert_eq!(s.status, "complete");
    assert_eq!(s.usage.input_tokens, 144_733 - 103_296);
    assert_eq!(s.usage.cache_read_tokens, 103_296);
    assert_eq!(s.usage.cache_creation_tokens, 0);
    assert_eq!(s.usage.output_tokens, 2_110);

    // Hand-computed micro-USD:
    //   input      = 41437  * 1_750_000 /1e6 = 72_514 (floor 72514.75)
    //   output     = 2110   * 14_000_000/1e6 = 29_540
    //   cache_read = 103296 * 175_000   /1e6 = 18_076 (floor 18076.8)
    //   total = 120_130
    match &s.cost {
        CostOutcome::Priced { cost } => {
            assert_eq!(cost.input_micro_usd, 72_514);
            assert_eq!(cost.output_micro_usd, 29_540);
            assert_eq!(cost.cache_read_micro_usd, 18_076);
            assert_eq!(cost.total_micro_usd, 120_130);
        }
        CostOutcome::Unpriced { .. } => panic!("priced"),
    }
    assert_eq!(s.source_reported_micro_usd, None);
}

#[test]
fn must_pass_codex_app_server_usage_uses_cumulative_token_updates() {
    let temp = TempDir::new().expect("tempdir");
    let service = service_with_db(temp.path());
    let db = db_of(&service);
    service
        .agent_cost_price_put_impl(AgentCostPricePutParams {
            model_id: "gpt-5.2".to_owned(),
            provider: Some("openai".to_owned()),
            input_usd_per_mtok: 1.75,
            output_usd_per_mtok: 14.0,
            cache_read_usd_per_mtok: 0.175,
            cache_creation_usd_per_mtok: 0.0,
            cache_creation_5m_usd_per_mtok: None,
            cache_creation_1h_usd_per_mtok: None,
        })
        .expect("price");

    let spawn = "agent-spawn-codex-app-server-cost";
    write_row(
        &db,
        spawn,
        1,
        200,
        TranscriptSource::CodexAppServerJsonRpc,
        "codex_app_server/turn/started",
        Some("gpt-5.2"),
        TranscriptRole::System,
        None,
    );
    write_row(
        &db,
        spawn,
        2,
        210,
        TranscriptSource::CodexAppServerJsonRpc,
        "codex_app_server/thread/tokenUsage/updated",
        Some("gpt-5.2"),
        TranscriptRole::Result,
        Some(codex_usage(50_000, 1_000, 30_000, 500)),
    );
    write_row(
        &db,
        spawn,
        3,
        220,
        TranscriptSource::CodexAppServerJsonRpc,
        "codex_app_server/thread/tokenUsage/updated",
        Some("gpt-5.2"),
        TranscriptRole::Result,
        Some(codex_usage(144_733, 2_110, 103_296, 1_380)),
    );

    let out = service
        .agent_cost_impl(cost_params(Some(spawn), None, None))
        .expect("rollup");
    let s = &out.per_spawn[0];
    assert_eq!(s.status, "complete");
    assert_eq!(s.source.as_deref(), Some("codex_app_server_json_rpc"));
    assert_eq!(s.usage.input_tokens, 144_733 - 103_296);
    assert_eq!(s.usage.cache_read_tokens, 103_296);
    assert_eq!(s.usage.output_tokens, 2_110);
}

#[test]
fn must_pass_local_model_usage_sums_per_turn_finished_rows() {
    let temp = TempDir::new().expect("tempdir");
    let service = service_with_db(temp.path());
    let db = db_of(&service);
    service
        .agent_cost_price_put_impl(AgentCostPricePutParams {
            model_id: "gemma4:e4b".to_owned(),
            provider: Some("local".to_owned()),
            input_usd_per_mtok: 0.0,
            output_usd_per_mtok: 0.0,
            cache_read_usd_per_mtok: 0.0,
            cache_creation_usd_per_mtok: 0.0,
            cache_creation_5m_usd_per_mtok: None,
            cache_creation_1h_usd_per_mtok: None,
        })
        .expect("price local model at zero");

    let spawn = "agent-spawn-local-cost";
    write_row(
        &db,
        spawn,
        1,
        100,
        TranscriptSource::LocalModelJson,
        "local.thread.started",
        Some("gemma4:e4b"),
        TranscriptRole::System,
        None,
    );
    write_row(
        &db,
        spawn,
        2,
        110,
        TranscriptSource::LocalModelJson,
        "local.turn.finished",
        Some("gemma4:e4b"),
        TranscriptRole::Result,
        Some(TranscriptUsage {
            input_tokens: Some(100),
            output_tokens: Some(20),
            ..TranscriptUsage::default()
        }),
    );
    write_row(
        &db,
        spawn,
        3,
        120,
        TranscriptSource::LocalModelJson,
        "local.turn.finished",
        Some("gemma4:e4b"),
        TranscriptRole::Result,
        Some(TranscriptUsage {
            input_tokens: Some(30),
            output_tokens: Some(10),
            ..TranscriptUsage::default()
        }),
    );

    let out = service
        .agent_cost_impl(cost_params(Some(spawn), None, None))
        .expect("rollup");
    let s = &out.per_spawn[0];
    assert_eq!(s.status, "complete");
    assert_eq!(s.source.as_deref(), Some("local_model_json"));
    assert_eq!(s.authoritative_line_no, Some(3));
    assert_eq!(s.usage.input_tokens, 130);
    assert_eq!(s.usage.output_tokens, 30);
    assert_eq!(s.total_tokens, 160);
    match &s.cost {
        CostOutcome::Priced { cost } => assert_eq!(cost.total_micro_usd, 0),
        CostOutcome::Unpriced { .. } => panic!("local model is priced"),
    }
}

// ---------------------------------------------------------------------------
// Edge cases
// ---------------------------------------------------------------------------

#[test]
fn must_fail_unpriced_model_surfaces_without_guessing() {
    let temp = TempDir::new().expect("tempdir");
    let service = service_with_db(temp.path());
    let db = db_of(&service);
    let spawn = "agent-spawn-unpriced";
    write_row(
        &db,
        spawn,
        1,
        300,
        TranscriptSource::ClaudeStreamJson,
        "result/success",
        Some("mystery-model"),
        TranscriptRole::Result,
        Some(claude_usage(100, 200, 0, 0, Some(42))),
    );

    let out = service
        .agent_cost_impl(cost_params(Some(spawn), None, None))
        .expect("rollup");
    let s = &out.per_spawn[0];
    assert_eq!(s.total_tokens, 300);
    match &s.cost {
        CostOutcome::Unpriced { model_id } => assert_eq!(model_id, "mystery-model"),
        CostOutcome::Priced { .. } => panic!("must be unpriced"),
    }
    assert_eq!(out.fleet.computed_micro_usd, 0);
    assert_eq!(out.fleet.unpriced_models, vec!["mystery-model".to_owned()]);
    assert_eq!(s.source_reported_micro_usd, Some(42));
}

#[test]
fn must_fail_running_agent_with_no_terminal_row_is_incomplete_not_billed() {
    let temp = TempDir::new().expect("tempdir");
    let service = service_with_db(temp.path());
    let db = db_of(&service);
    let spawn = "agent-spawn-running";
    write_row(
        &db,
        spawn,
        1,
        400,
        TranscriptSource::ClaudeStreamJson,
        "assistant",
        Some("claude-fable-5"),
        TranscriptRole::Assistant,
        Some(claude_usage(10, 2, 0, 0, None)),
    );

    let out = service
        .agent_cost_impl(cost_params(Some(spawn), None, None))
        .expect("rollup");
    let s = &out.per_spawn[0];
    assert_eq!(s.status, "no_terminal_usage");
    assert_eq!(s.total_tokens, 0);
    assert_eq!(out.fleet.spawns_incomplete, 1);
    assert_eq!(out.fleet.spawns_complete, 0);
}

#[test]
fn must_fail_time_window_excludes_rows_outside_range() {
    let temp = TempDir::new().expect("tempdir");
    let service = service_with_db(temp.path());
    let db = db_of(&service);
    let spawn = "agent-spawn-window";
    write_row(
        &db,
        spawn,
        1,
        500,
        TranscriptSource::ClaudeStreamJson,
        "result/success",
        Some("mystery"),
        TranscriptRole::Result,
        Some(claude_usage(100, 100, 0, 0, None)),
    );

    let out = service
        .agent_cost_impl(cost_params(Some(spawn), Some(600), Some(700)))
        .expect("rollup");
    // The row is physically scanned but filtered out of the window, so the
    // spawn contributes nothing and is not listed.
    assert_eq!(out.scanned_rows, 1);
    assert!(
        out.per_spawn.is_empty(),
        "out-of-window row must not appear"
    );
    assert_eq!(out.fleet.spawns_total, 0);

    // Widening the window to include ts=500 surfaces the spawn and bills it.
    let included = service
        .agent_cost_impl(cost_params(Some(spawn), Some(400), Some(600)))
        .expect("rollup");
    assert_eq!(included.per_spawn.len(), 1);
    assert_eq!(included.per_spawn[0].status, "complete");
}

/// Window edges: `since_ns` is inclusive and `until_ns` is exclusive.
#[test]
fn must_pass_time_window_edges_since_inclusive_until_exclusive() {
    let temp = TempDir::new().expect("tempdir");
    let service = service_with_db(temp.path());
    let db = db_of(&service);
    let spawn = "agent-spawn-window-edges";
    write_row(
        &db,
        spawn,
        1,
        500,
        TranscriptSource::ClaudeStreamJson,
        "result/success",
        Some("mystery"),
        TranscriptRole::Result,
        Some(claude_usage(100, 100, 0, 0, None)),
    );

    // since == ts: included.
    let at_since = service
        .agent_cost_impl(cost_params(Some(spawn), Some(500), Some(501)))
        .expect("rollup");
    assert_eq!(
        at_since.per_spawn.len(),
        1,
        "a row at since_ns is in window"
    );
    assert_eq!(at_since.per_spawn[0].total_tokens, 200);

    // until == ts: excluded.
    let at_until = service
        .agent_cost_impl(cost_params(Some(spawn), Some(499), Some(500)))
        .expect("rollup");
    assert!(
        at_until.per_spawn.is_empty(),
        "a row at until_ns is out of window"
    );
    assert_eq!(at_until.fleet.spawns_total, 0);
}

#[test]
fn must_pass_fleet_scan_aggregates_multiple_spawns_and_models() {
    let temp = TempDir::new().expect("tempdir");
    let service = service_with_db(temp.path());
    let db = db_of(&service);
    service
        .agent_cost_price_put_impl(fable_price())
        .expect("price");

    // Two Claude spawns on the priced model, one Codex spawn unpriced.
    write_row(
        &db,
        "agent-spawn-a",
        1,
        100,
        TranscriptSource::ClaudeStreamJson,
        "result/success",
        Some("claude-fable-5"),
        TranscriptRole::Result,
        Some(claude_usage(1_000_000, 0, 0, 0, Some(1))),
    );
    write_row(
        &db,
        "agent-spawn-b",
        1,
        100,
        TranscriptSource::ClaudeStreamJson,
        "result/success",
        Some("claude-fable-5"),
        TranscriptRole::Result,
        Some(claude_usage(2_000_000, 0, 0, 0, Some(2))),
    );
    write_row(
        &db,
        "agent-spawn-c",
        1,
        100,
        TranscriptSource::CodexExecJson,
        "turn.completed",
        Some("gpt-5.2"),
        TranscriptRole::Result,
        Some(codex_usage(500, 0, 0, 0)),
    );

    let out = service
        .agent_cost_impl(cost_params(None, None, None))
        .expect("fleet rollup");
    assert_eq!(out.scanned_rows, 3);
    assert_eq!(out.fleet.spawns_total, 3);
    assert_eq!(out.fleet.spawns_complete, 3);
    // Claude input $3/Mtok: 1M -> 3_000_000 micro, 2M -> 6_000_000 micro.
    assert_eq!(out.fleet.computed_micro_usd, 9_000_000);
    assert_eq!(out.fleet.source_reported_micro_usd, 3);
    assert_eq!(out.fleet.unpriced_models, vec!["gpt-5.2".to_owned()]);
    // Per-model: fable aggregates the two spawns.
    let fable = out
        .per_model
        .iter()
        .find(|m| m.model == "claude-fable-5")
        .expect("fable present");
    assert_eq!(fable.spawns, 2);
    assert_eq!(fable.computed_micro_usd, Some(9_000_000));
}

#[test]
fn must_fail_empty_store_yields_zero_rollup() {
    let temp = TempDir::new().expect("tempdir");
    let service = service_with_db(temp.path());
    let out = service
        .agent_cost_impl(cost_params(None, None, None))
        .expect("rollup");
    assert_eq!(out.scanned_rows, 0);
    assert_eq!(out.fleet.spawns_total, 0);
    assert_eq!(out.fleet.computed_micro_usd, 0);
    assert!(out.per_spawn.is_empty());
}

// ---------------------------------------------------------------------------
// End-to-end FSV against the REAL captured CLI fixtures (no synthetic usage).
// Ingest through the real #900 parser, then reconcile agent_cost against the
// physical rows and the CLI's own reported cost.
// ---------------------------------------------------------------------------

fn plant_spawn_dir(root: &Path, spawn_id: &str, source: TranscriptSource, stdout: &str) -> PathBuf {
    let log_dir = root.join(spawn_id);
    std::fs::create_dir_all(&log_dir).expect("create spawn dir");
    let marker = match source {
        TranscriptSource::ClaudeStreamJson => "claude-mcp-config.json",
        TranscriptSource::CodexExecJson => "codex-notify.ps1",
        TranscriptSource::CodexAppServerJsonRpc => "codex-app-server-runner.ps1",
        TranscriptSource::LocalModelJson => "local-model-runner.json",
        TranscriptSource::ClaudeSessionJsonl => {
            unreachable!("cost spawn-dir helper does not handle ClaudeSessionJsonl")
        }
    };
    std::fs::write(log_dir.join(marker), b"{}").expect("marker");
    std::fs::write(log_dir.join("stdout.jsonl"), stdout).expect("stdout");
    std::fs::write(
        log_dir.join("completion-status.json"),
        br#"{"schema_version":1,"status":"ok"}"#,
    )
    .expect("completion");
    log_dir
}

#[test]
fn must_fail_range_with_since_ge_until_is_rejected() {
    let temp = TempDir::new().expect("tempdir");
    let service = service_with_db(temp.path());
    let err = service
        .agent_cost_impl(cost_params(None, Some(10), Some(10)))
        .expect_err("must reject");
    assert!(
        err.message.contains("AGENT_COST_RANGE_INVALID"),
        "{:?}",
        err.message
    );
}

// ---------------------------------------------------------------------------
// Per-turn cost series (#950) — real ingestion path, synthetic known I/O
// ---------------------------------------------------------------------------

#[test]
fn must_pass_per_turn_codex_cumulative_delta_reconciles_to_spawn_total() {
    let temp = TempDir::new().expect("tempdir");
    let service = service_with_db(temp.path());
    let db = db_of(&service);

    // Price the synthetic model: $1/Mtok input, $2/Mtok output, $0.1/Mtok cache-read.
    service
        .agent_cost_price_put_impl(AgentCostPricePutParams {
            model_id: "synthetic-codex-turn".to_owned(),
            provider: Some("local".to_owned()),
            input_usd_per_mtok: 1.0,
            output_usd_per_mtok: 2.0,
            cache_read_usd_per_mtok: 0.1,
            cache_creation_usd_per_mtok: 0.0,
            cache_creation_5m_usd_per_mtok: None,
            cache_creation_1h_usd_per_mtok: None,
        })
        .expect("price");

    // Synthetic 3-turn Codex stream. turn.completed usage is CUMULATIVE, so the
    // per-turn series is the consecutive delta. Cumulatives chosen so the deltas
    // are clean and each turn keeps cached <= input:
    //   T1 cum (in=1000, cached=200, out=100) -> delta billable in=800  cr=200  out=100
    //   T2 cum (in=3000, cached=900, out=300) -> delta billable in=1300 cr=700  out=200
    //   T3 cum (in=6000, cached=2400,out=750) -> delta billable in=1500 cr=1500 out=450
    // Session total billable: in=3600 cr=2400 out=750 (== sum of turns).
    let stdout = concat!(
        "{\"type\":\"thread.started\",\"thread_id\":\"th_synth\"}\n",
        "{\"type\":\"turn.started\"}\n",
        "{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":1000,\"cached_input_tokens\":200,\"output_tokens\":100,\"reasoning_output_tokens\":0}}\n",
        "{\"type\":\"turn.started\"}\n",
        "{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":3000,\"cached_input_tokens\":900,\"output_tokens\":300,\"reasoning_output_tokens\":0}}\n",
        "{\"type\":\"turn.started\"}\n",
        "{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":6000,\"cached_input_tokens\":2400,\"output_tokens\":750,\"reasoning_output_tokens\":0}}\n",
    );
    let root = TempDir::new().expect("spawn root");
    let spawn = "agent-spawn-codexturns";
    let log_dir = plant_spawn_dir(root.path(), spawn, TranscriptSource::CodexExecJson, stdout);
    // Codex exec --json carries no model id; the spawn manifest is authoritative.
    std::fs::write(
        log_dir.join("spawn-manifest.json"),
        br#"{"model":"synthetic-codex-turn"}"#,
    )
    .expect("manifest");

    let outcome = ingest_spawn_dir_once(&db, spawn, &log_dir, false).expect("ingest");
    assert_eq!(
        outcome.new_invalid_rows, 0,
        "synthetic Codex stream must parse fully"
    );

    let out = service
        .agent_cost_impl(cost_params_per_turn(Some(spawn)))
        .expect("agent_cost per-turn");
    assert_eq!(out.per_spawn.len(), 1);
    let spawn_cost = &out.per_spawn[0];
    assert_eq!(spawn_cost.status, "complete");

    // Spawn total reconciles with the cumulative last row.
    assert_eq!(spawn_cost.usage.input_tokens, 3600);
    assert_eq!(spawn_cost.usage.cache_read_tokens, 2400);
    assert_eq!(spawn_cost.usage.output_tokens, 750);

    // Per-turn series: exact deltas.
    let turns = spawn_cost.turns.as_ref().expect("per-turn series present");
    assert_eq!(turns.len(), 3);
    let expect = [
        (1u64, 800u64, 200u64, 100u64),
        (2, 1300, 700, 200),
        (3, 1500, 1500, 450),
    ];
    for (turn, (ti, input, cache_read, output)) in turns.iter().zip(expect) {
        assert_eq!(turn.turn_index, ti, "turn_index");
        assert_eq!(turn.usage.input_tokens, input, "turn {ti} input");
        assert_eq!(
            turn.usage.cache_read_tokens, cache_read,
            "turn {ti} cache_read"
        );
        assert_eq!(turn.usage.output_tokens, output, "turn {ti} output");
        assert!(
            matches!(turn.output_basis, TurnOutputBasis::Exact),
            "codex output is exact"
        );
    }

    // Reconciliation summary.
    let summary = spawn_cost.turns_summary.as_ref().expect("turns summary");
    assert_eq!(summary.method, "codex_cumulative_delta");
    assert_eq!(summary.turn_count, 3);
    assert!(summary.reconciles, "codex per-turn must reconcile exactly");
    assert_eq!(
        summary.turns_usage_sum, spawn_cost.usage,
        "sum of per-turn usage == spawn total"
    );

    // Cost reconciliation: per-turn micro-USD costs sum to the spawn cost.
    //   T1: 800*1 + 200*0.1 + 100*2 = 1020
    //   T2: 1300*1 + 700*0.1 + 200*2 = 1770
    //   T3: 1500*1 + 1500*0.1 + 450*2 = 2550   sum = 5340
    let turn_costs: Vec<u64> = turns
        .iter()
        .map(|t| match &t.cost {
            CostOutcome::Priced { cost } => cost.total_micro_usd,
            CostOutcome::Unpriced { model_id } => panic!("turn unpriced: {model_id}"),
        })
        .collect();
    assert_eq!(turn_costs, vec![1020, 1770, 2550]);
    let CostOutcome::Priced { cost } = &spawn_cost.cost else {
        panic!("spawn must be priced");
    };
    assert_eq!(cost.total_micro_usd, 5340);
    assert_eq!(turn_costs.iter().sum::<u64>(), cost.total_micro_usd);
}

#[test]
fn must_pass_per_turn_omitted_by_default_and_present_only_when_requested() {
    let temp = TempDir::new().expect("tempdir");
    let service = service_with_db(temp.path());
    let db = db_of(&service);
    let stdout = concat!(
        "{\"type\":\"thread.started\",\"thread_id\":\"th_x\"}\n",
        "{\"type\":\"turn.started\"}\n",
        "{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":500,\"cached_input_tokens\":0,\"output_tokens\":50,\"reasoning_output_tokens\":0}}\n",
    );
    let root = TempDir::new().expect("spawn root");
    let spawn = "agent-spawn-codex-single";
    let log_dir = plant_spawn_dir(root.path(), spawn, TranscriptSource::CodexExecJson, stdout);
    ingest_spawn_dir_once(&db, spawn, &log_dir, false).expect("ingest");

    // Default: no turns field.
    let out = service
        .agent_cost_impl(cost_params(Some(spawn), None, None))
        .expect("agent_cost default");
    assert!(out.per_spawn[0].turns.is_none(), "turns omitted by default");
    assert!(out.per_spawn[0].turns_summary.is_none());

    // include_per_turn: a single-turn session yields a single reconciling turn.
    let out = service
        .agent_cost_impl(cost_params_per_turn(Some(spawn)))
        .expect("agent_cost per-turn");
    let turns = out.per_spawn[0].turns.as_ref().expect("turns present");
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].turn_index, 1);
    assert_eq!(turns[0].usage.input_tokens, 500);
    assert_eq!(turns[0].usage.output_tokens, 50);
    assert!(out.per_spawn[0].turns_summary.as_ref().unwrap().reconciles);
}

#[test]
fn must_fail_per_turn_codex_nonmonotonic_cumulative_is_a_loud_error() {
    // Edge case (invalid input): Codex cumulative totals MUST be monotonic.
    // A decrease means the transcript is corrupt — surfaced loudly, never
    // clamped into a bogus negative-delta "turn".
    let temp = TempDir::new().expect("tempdir");
    let service = service_with_db(temp.path());
    let db = db_of(&service);
    let stdout = concat!(
        "{\"type\":\"thread.started\",\"thread_id\":\"th_bad\"}\n",
        "{\"type\":\"turn.started\"}\n",
        "{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":5000,\"cached_input_tokens\":0,\"output_tokens\":500,\"reasoning_output_tokens\":0}}\n",
        "{\"type\":\"turn.started\"}\n",
        "{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":3000,\"cached_input_tokens\":0,\"output_tokens\":600,\"reasoning_output_tokens\":0}}\n",
    );
    let root = TempDir::new().expect("spawn root");
    let spawn = "agent-spawn-codexbad";
    let log_dir = plant_spawn_dir(root.path(), spawn, TranscriptSource::CodexExecJson, stdout);
    ingest_spawn_dir_once(&db, spawn, &log_dir, false).expect("ingest");

    // Without per-turn the rollup still works (elementwise max).
    service
        .agent_cost_impl(cost_params(Some(spawn), None, None))
        .expect("session rollup tolerates the rows");

    // With per-turn the corrupt cumulative is surfaced as a loud error.
    let err = service
        .agent_cost_impl(cost_params_per_turn(Some(spawn)))
        .expect_err("non-monotonic cumulative must error");
    assert!(
        err.message.contains("AGENT_COST_TURN_NONMONOTONIC"),
        "{:?}",
        err.message
    );
}

#[test]
fn must_pass_per_turn_claude_real_fixture_exposes_per_message_with_partial_output_flag() {
    // Edge case (different source, REAL capture): Claude per-turn is per-message
    // (deduped by distinct message id, highest streaming line wins). Input/cache
    // are reliable; output is a partial streaming snapshot, so the series is
    // flagged partial_snapshot and does NOT claim to reconcile to the
    // authoritative session `result` total.
    let temp = TempDir::new().expect("tempdir");
    let service = service_with_db(temp.path());
    let db = db_of(&service);
    let root = TempDir::new().expect("spawn root");
    let spawn = "agent-spawn-claudeturns";
    let log_dir = plant_spawn_dir(
        root.path(),
        spawn,
        TranscriptSource::ClaudeStreamJson,
        CLAUDE_REAL_STREAM,
    );
    let outcome = ingest_spawn_dir_once(&db, spawn, &log_dir, false).expect("ingest");
    assert_eq!(
        outcome.new_invalid_rows, 0,
        "real Claude capture parses fully"
    );

    let out = service
        .agent_cost_impl(cost_params_per_turn(Some(spawn)))
        .expect("agent_cost per-turn");
    let sp = &out.per_spawn[0];
    let turns = sp.turns.as_ref().expect("per-turn present");
    // The real capture has 9 distinct assistant message ids = 9 turns.
    assert_eq!(turns.len(), 9, "one turn per distinct assistant message");
    for (i, turn) in turns.iter().enumerate() {
        assert_eq!(
            turn.turn_index,
            u64::try_from(i + 1).unwrap(),
            "claude turns are 1-based contiguous"
        );
        assert!(
            matches!(turn.output_basis, TurnOutputBasis::PartialSnapshot),
            "claude per-turn output is a partial snapshot"
        );
    }
    let summary = sp.turns_summary.as_ref().expect("summary");
    assert_eq!(summary.method, "claude_per_message");
    assert_eq!(summary.turn_count, 9);
    assert!(
        !summary.reconciles,
        "claude per-turn output is partial — must not claim reconciliation"
    );
    // The authoritative session output (spawn-level) exceeds the partial
    // per-message output sum — the gap is surfaced, never hidden.
    assert!(
        summary.turns_usage_sum.output_tokens < sp.usage.output_tokens,
        "partial per-message output undercounts the authoritative session output"
    );
}

// ---------------------------------------------------------------------------
// #951 — per-template / per-task rollups (real CF_AGENT_EVENTS + CF_KV rows)
// ---------------------------------------------------------------------------
//
// The join sources are built through real write paths:
//  * spawn->template: a real `SpawnRequested` row journaled to CF_AGENT_EVENTS
//    via `record_agent_event` (the same call act_spawn_agent makes), flushed so
//    the rollup scan sees it.
//  * spawn->task: a real claim via `claim_internal` (the #957 dispatcher path)
//    binding spawn_id + template_version onto the task's attempt in CF_KV.
// Each test then asserts the grouped rollup reconciles EXACTLY with the fleet
// total — the FinOps "no unallocated spend hidden" invariant.

use crate::server::agent_events::record_agent_event;
use synapse_core::{AgentEventKind, AgentEventRecord};

/// Journals a real `SpawnRequested` event carrying #909 template provenance and
/// flushes it so the spawn->template scan (the RocksDB read path) sees it.
fn journal_spawn_with_template(db: &Db, spawn: &str, template_id: &str, version: u32, ts_ns: u64) {
    let mut record = AgentEventRecord::new(ts_ns, AgentEventKind::SpawnRequested);
    record.spawn_id = Some(spawn.to_owned());
    record.payload = serde_json::json!({
        "template_id": template_id,
        "template_version": version,
    });
    record_agent_event(db, &record).expect("journal SpawnRequested");
    db.flush().expect("flush agent events");
}

/// Writes a minimal complete priced Claude spawn (init + single result row) and
/// returns its hand-computed cost in micro-USD under `fable_price()` rates
/// (input 3, output 15 USD/Mtok; no cache).
fn write_simple_claude_spawn(db: &Db, spawn: &str, input: u64, output: u64, ts: u64) -> u64 {
    write_row(
        db,
        spawn,
        1,
        ts,
        TranscriptSource::ClaudeStreamJson,
        "system/init",
        Some("claude-fable-5"),
        TranscriptRole::System,
        None,
    );
    write_row(
        db,
        spawn,
        2,
        ts + 10,
        TranscriptSource::ClaudeStreamJson,
        "result/success",
        Some("claude-fable-5"),
        TranscriptRole::Result,
        Some(claude_usage(input, output, 0, 0, None)),
    );
    // micro-USD per token = rate_per_mtok / 1e6, so input*3 + output*15.
    input * 3 + output * 15
}

#[test]
fn must_pass_per_template_rollup_reconciles_and_buckets_unattributed() {
    let temp = TempDir::new().expect("tempdir");
    let service = service_with_db(temp.path());
    let db = db_of(&service);
    service
        .agent_cost_price_put_impl(fable_price())
        .expect("price");

    let a = "agent-spawn-aaaa";
    let b = "agent-spawn-bbbb";
    let c = "agent-spawn-cccc";
    let cost_a = write_simple_claude_spawn(&db, a, 1_000, 100, 100);
    let cost_b = write_simple_claude_spawn(&db, b, 2_000, 200, 200);
    let cost_c = write_simple_claude_spawn(&db, c, 500, 50, 300);
    journal_spawn_with_template(&db, a, "rev", 1, 1_000);
    journal_spawn_with_template(&db, b, "rev", 2, 2_000);
    // c: no template event -> must land in the (unattributed) bucket.

    let mut params = cost_params(None, None, None);
    params.group_by = vec![AgentCostGroupBy::Template];
    let out = service.agent_cost_impl(params).expect("rollup");

    assert_eq!(out.fleet.computed_micro_usd, cost_a + cost_b + cost_c);
    assert_eq!(
        out.scanned_event_rows, 2,
        "exactly the 2 journaled SpawnRequested rows are scanned"
    );

    let groups = out.per_template.expect("per_template present");
    assert_eq!(groups.len(), 2, "rev + unattributed: {groups:#?}");
    let rev = &groups[0];
    assert_eq!(rev.key, "rev");
    assert!(rev.attributed);
    assert_eq!(rev.spawns, 2);
    assert_eq!(rev.spawns_complete, 2);
    assert_eq!(rev.template_versions, vec![1, 2]);
    assert_eq!(rev.computed_micro_usd, cost_a + cost_b);
    let mut got_ids = rev.spawn_ids.clone();
    got_ids.sort();
    assert_eq!(got_ids, vec![a.to_owned(), b.to_owned()]);

    let resid = &groups[1];
    assert_eq!(resid.key, "(unattributed)");
    assert!(!resid.attributed);
    assert_eq!(resid.spawns, 1);
    assert_eq!(resid.computed_micro_usd, cost_c);
    assert_eq!(resid.spawn_ids, vec![c.to_owned()]);

    let summed: u64 = groups.iter().map(|g| g.computed_micro_usd).sum();
    assert_eq!(
        summed, out.fleet.computed_micro_usd,
        "per_template must reconcile with fleet"
    );
    println!(
        "FSV[per_template] rev={} unattributed={} fleet={} (reconciles)",
        rev.computed_micro_usd, resid.computed_micro_usd, out.fleet.computed_micro_usd
    );
}

#[test]
fn must_pass_per_task_rollup_groups_by_task_and_carries_template() {
    let temp = TempDir::new().expect("tempdir");
    let service = service_with_db(temp.path());
    let db = db_of(&service);
    service
        .agent_cost_price_put_impl(fable_price())
        .expect("price");

    let a = "agent-spawn-task-a";
    let b = "agent-spawn-task-b";
    let c = "agent-spawn-orphan";
    let cost_a = write_simple_claude_spawn(&db, a, 1_000, 100, 100);
    let cost_b = write_simple_claude_spawn(&db, b, 3_000, 300, 200);
    let cost_c = write_simple_claude_spawn(&db, c, 700, 70, 300);

    for (task_id, template_id) in [("task-alpha", "rev"), ("task-beta", "deep")] {
        service
            .task_create_for_test(task_id, template_id)
            .expect("create task");
    }
    service
        .task_claim_with_spawn_for_test("task-alpha", "sess-a", a, 1)
        .expect("bind a");
    service
        .task_claim_with_spawn_for_test("task-beta", "sess-b", b, 2)
        .expect("bind b");
    // c: no task binding -> unattributed.

    let mut params = cost_params(None, None, None);
    params.group_by = vec![AgentCostGroupBy::Task];
    let out = service.agent_cost_impl(params).expect("rollup");
    assert_eq!(out.fleet.computed_micro_usd, cost_a + cost_b + cost_c);

    let groups = out.per_task.expect("per_task present");
    assert_eq!(groups.len(), 3, "two tasks + unattributed: {groups:#?}");
    let alpha = groups
        .iter()
        .find(|g| g.key == "task-alpha")
        .expect("alpha");
    assert_eq!(alpha.template_id.as_deref(), Some("rev"));
    assert_eq!(alpha.computed_micro_usd, cost_a);
    assert_eq!(alpha.spawn_ids, vec![a.to_owned()]);
    let beta = groups.iter().find(|g| g.key == "task-beta").expect("beta");
    assert_eq!(beta.template_id.as_deref(), Some("deep"));
    assert_eq!(beta.computed_micro_usd, cost_b);
    let resid = groups
        .iter()
        .find(|g| g.key == "(unattributed)")
        .expect("residual");
    assert!(!resid.attributed);
    assert_eq!(resid.computed_micro_usd, cost_c);
    assert_eq!(
        groups.last().expect("nonempty").key,
        "(unattributed)",
        "residual is last"
    );

    let summed: u64 = groups.iter().map(|g| g.computed_micro_usd).sum();
    assert_eq!(
        summed, out.fleet.computed_micro_usd,
        "per_task must reconcile with fleet"
    );
    println!(
        "FSV[per_task] alpha={} beta={} unattributed={} fleet={} (reconciles)",
        alpha.computed_micro_usd,
        beta.computed_micro_usd,
        resid.computed_micro_usd,
        out.fleet.computed_micro_usd
    );
}

#[test]
fn must_pass_group_by_both_returns_both_rollups_and_omitted_returns_neither() {
    let temp = TempDir::new().expect("tempdir");
    let service = service_with_db(temp.path());
    let db = db_of(&service);
    service
        .agent_cost_price_put_impl(fable_price())
        .expect("price");

    let a = "agent-spawn-both-a";
    write_simple_claude_spawn(&db, a, 1_000, 100, 100);
    journal_spawn_with_template(&db, a, "rev", 1, 1_000);
    service
        .task_create_for_test("task-x", "rev")
        .expect("create");
    service
        .task_claim_with_spawn_for_test("task-x", "sess-x", a, 1)
        .expect("bind");

    // Omitted: no grouping, and the event journal is not scanned at all.
    let bare = service
        .agent_cost_impl(cost_params(None, None, None))
        .expect("bare");
    assert!(bare.per_template.is_none());
    assert!(bare.per_task.is_none());
    assert_eq!(
        bare.scanned_event_rows, 0,
        "no event scan when template not requested"
    );

    let mut params = cost_params(None, None, None);
    params.group_by = vec![AgentCostGroupBy::Template, AgentCostGroupBy::Task];
    let out = service.agent_cost_impl(params).expect("both");
    let tmpl = out.per_template.expect("per_template");
    let task = out.per_task.expect("per_task");
    assert_eq!(
        tmpl.iter()
            .find(|g| g.key == "rev")
            .expect("rev")
            .computed_micro_usd,
        out.fleet.computed_micro_usd
    );
    assert_eq!(
        task.iter()
            .find(|g| g.key == "task-x")
            .expect("task-x")
            .computed_micro_usd,
        out.fleet.computed_micro_usd
    );
}

#[test]
fn must_fail_per_template_surfaces_unpriced_model_and_counts_incomplete() {
    let temp = TempDir::new().expect("tempdir");
    let service = service_with_db(temp.path());
    let db = db_of(&service);
    service
        .agent_cost_price_put_impl(fable_price())
        .expect("price");

    let priced = "agent-spawn-priced";
    let unpriced = "agent-spawn-unpriced";
    let running = "agent-spawn-running";
    let cost_priced = write_simple_claude_spawn(&db, priced, 1_000, 100, 100);
    // Unpriced model, complete: tokens counted, no cost.
    write_row(
        &db,
        unpriced,
        1,
        200,
        TranscriptSource::ClaudeStreamJson,
        "system/init",
        Some("claude-mystery"),
        TranscriptRole::System,
        None,
    );
    write_row(
        &db,
        unpriced,
        2,
        210,
        TranscriptSource::ClaudeStreamJson,
        "result/success",
        Some("claude-mystery"),
        TranscriptRole::Result,
        Some(claude_usage(900, 9, 0, 0, None)),
    );
    // Running: only a non-terminal row -> incomplete.
    write_row(
        &db,
        running,
        1,
        300,
        TranscriptSource::ClaudeStreamJson,
        "system/init",
        Some("claude-fable-5"),
        TranscriptRole::System,
        None,
    );

    journal_spawn_with_template(&db, priced, "rev", 1, 1_000);
    journal_spawn_with_template(&db, unpriced, "rev", 1, 2_000);
    journal_spawn_with_template(&db, running, "rev", 1, 3_000);

    let mut params = cost_params(None, None, None);
    params.group_by = vec![AgentCostGroupBy::Template];
    let out = service.agent_cost_impl(params).expect("rollup");

    let groups = out.per_template.expect("per_template");
    let rev = groups.iter().find(|g| g.key == "rev").expect("rev");
    assert_eq!(rev.spawns, 3, "all three counted");
    assert_eq!(
        rev.spawns_complete, 2,
        "priced + unpriced complete; running not"
    );
    assert_eq!(
        rev.computed_micro_usd, cost_priced,
        "cost excludes unpriced + running"
    );
    assert_eq!(
        rev.unpriced_models,
        vec!["claude-mystery".to_owned()],
        "only the unpriced model is surfaced"
    );
    assert_eq!(
        rev.total_tokens,
        900 + 9 + 1_000 + 100,
        "unpriced tokens still counted; the running spawn has no billable usage"
    );
    let summed: u64 = groups.iter().map(|g| g.computed_micro_usd).sum();
    assert_eq!(summed, out.fleet.computed_micro_usd);
    println!(
        "FSV[per_template unpriced] rev.computed={} unpriced_models={:?}",
        rev.computed_micro_usd, rev.unpriced_models
    );
}
