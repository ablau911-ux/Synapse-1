//! Searchable history of what was done (Phase 7 part 4).
//!
//! `call_tool` (handler.rs) is the one place every MCP tool call finishes;
//! it hands each finished call of a tool with mutating operations to
//! `record_action_history`, which keeps one small `CF_KV` row under
//! `action-history/v1/` when the operation mutates state. `timeline`
//! operation `history_search` reads the rows back with a plain prefix scan.
//!
//! The rules live in `synapse_core::action_history`; this file adds the
//! target window read, the storage reads/writes and the retention pruning.
//! A history failure never changes the tool call's own result.

use std::sync::LazyLock;
use std::sync::atomic::{AtomicU32, Ordering};

use rmcp::model::Tool;
use rmcp::schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use synapse_core::action_history::{
    ACTION_HISTORY_PREFIX, ActionHistoryConfig, ActionHistoryRow, FinishedCall, HistoryQuery,
    outcome_label, resolve_operation,
};
use synapse_storage::{Db, StorageError, cf};

use super::session_registry::unix_time_ms_now;
use super::tool_profiles::{operation_mutates_state, tool_has_mutating_operation};
use super::url_redaction::redact_url_fields_for_public_readback;
use super::workspace_blackboard::redact_ledger_text;
use super::{ErrorData, SessionTarget, SynapseService};
use crate::m1::mcp_error;

static ACTION_HISTORY_CONFIG: LazyLock<ActionHistoryConfig> =
    LazyLock::new(ActionHistoryConfig::from_env);
/// Orders rows written in the same millisecond.
static ACTION_HISTORY_SEQ: AtomicU32 = AtomicU32::new(0);
const ACTION_HISTORY_CF: &str = "CF_KV";

/// What `call_tool` keeps, before dispatch, of a call to a tool that has
/// at least one mutating operation.
pub(super) struct HistoryCall {
    tool: String,
    argument_operation: Option<String>,
    args: Option<Value>,
}

impl HistoryCall {
    /// `None` for a tool without mutating operations, so read-only tools
    /// cost nothing. A call that omits `operation` gets the default its
    /// published schema declares (`act` invoke, `shell` run), so it is
    /// recorded even when it fails before the reply names the operation.
    /// URL query strings and fragments are removed from the kept arguments.
    pub(super) fn start(
        tool: &str,
        arguments: Option<&Map<String, Value>>,
        published: Option<&Tool>,
    ) -> Option<Self> {
        if !tool_has_mutating_operation(tool) {
            return None;
        }
        let schema_default = published
            .and_then(|tool| tool.input_schema.get("properties"))
            .and_then(|properties| properties.get("operation"))
            .and_then(|operation| operation.get("default"));
        let mut args = arguments.map(|arguments| Value::Object(arguments.clone()));
        if let Some(args) = args.as_mut() {
            redact_url_fields_for_public_readback(args);
        }
        Some(Self {
            tool: tool.to_owned(),
            argument_operation: arguments
                .and_then(|arguments| arguments.get("operation"))
                .or(schema_default)
                .and_then(Value::as_str)
                .map(str::to_owned),
            args,
        })
    }
}

/// How the call ended, as `call_tool` saw it.
pub(super) struct CallEnd<'a> {
    /// `operation` named by the reply or the error data.
    pub reply_operation: Option<&'a str>,
    pub failed: bool,
    pub error_code: Option<&'a str>,
    /// The window the call acted on, when the reply named one.
    pub effective_target: Option<&'a Value>,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TimelineHistorySearchParams {
    /// Words; a row matches when every word appears (case-insensitive) in its
    /// tool, operation, target or summary. Empty matches every row.
    #[serde(default)]
    pub query: String,
    /// Oldest finish time to include (unix ms, inclusive).
    #[serde(default)]
    pub since_ms: Option<u64>,
    /// Newest finish time to include (unix ms, inclusive).
    #[serde(default)]
    pub until_ms: Option<u64>,
    /// Rows to return, newest first: default 50, clamped to 1..=500.
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TimelineHistorySearchResponse {
    /// Matching rows, newest first.
    pub rows: Vec<ActionHistoryRow>,
    /// Rows read inside the time window.
    pub scanned: usize,
    pub matched: usize,
    /// True when more rows matched than `limit`.
    pub truncated: bool,
    pub limit: usize,
    pub retention_days: u64,
    pub cf_name: &'static str,
    pub key_prefix: &'static str,
}

impl SynapseService {
    /// Stores the history row of a finished call when its operation mutates
    /// state. Any failure is logged; the call's own result never changes.
    pub(super) fn record_action_history(
        &self,
        call: HistoryCall,
        session_id: Option<&str>,
        end: &CallEnd<'_>,
    ) {
        let Some(operation) =
            resolve_operation(call.argument_operation.as_deref(), end.reply_operation)
        else {
            return;
        };
        if !operation_mutates_state(&call.tool, &operation) {
            return;
        }
        let (app, title) = self.history_target_window(session_id, end.effective_target);
        let outcome = outcome_label(end.failed, end.error_code);
        let row = ActionHistoryRow::build(
            &FinishedCall {
                ts_ms: unix_time_ms_now(),
                session_id: session_id.unwrap_or(""),
                tool: &call.tool,
                operation: &operation,
                target_app: &app,
                target_title: &title,
                outcome: &outcome,
                args: call.args.as_ref(),
            },
            &redact_ledger_text,
        );
        let written = self
            .m3_storage()
            .map_err(|error| error.message.to_string())
            .and_then(|db| write_history_row(&db, &row, &ACTION_HISTORY_CONFIG));
        if let Err(detail) = written {
            tracing::warn!(
                code = "ACTION_HISTORY_WRITE_SKIPPED",
                tool = %call.tool,
                operation = %operation,
                detail = %detail,
                "action history row not stored; the tool call result stands"
            );
        }
    }

    /// `timeline` operation `history_search`: plain scan of the live rows.
    pub(super) fn timeline_history_search(
        &self,
        params: &TimelineHistorySearchParams,
    ) -> Result<TimelineHistorySearchResponse, ErrorData> {
        let cfg = &*ACTION_HISTORY_CONFIG;
        let cutoff = cfg.cutoff_ms(unix_time_ms_now());
        let since = params.since_ms.map_or(cutoff, |since| since.max(cutoff));
        let query = HistoryQuery::new(&params.query, Some(since), params.until_ms, params.limit);
        let db = self.m3_storage()?;
        let rows = db
            .scan_cf_prefix_from(
                cf::CF_KV,
                ACTION_HISTORY_PREFIX.as_bytes(),
                query.start_key().as_bytes(),
            )
            .map_err(|error| {
                mcp_error(
                    error.code(),
                    format!("scan {ACTION_HISTORY_CF} {ACTION_HISTORY_PREFIX}: {error}"),
                )
            })?;
        let mut scanned = 0;
        let mut matches = Vec::new();
        for row in rows
            .iter()
            .filter_map(|(_key, value)| ActionHistoryRow::decode(value))
        {
            if query.is_past(&row) {
                break;
            }
            scanned += 1;
            if query.matches(&row) {
                matches.push(row);
            }
        }
        let matched = matches.len();
        let (rows, truncated) = query.newest_first(matches);
        Ok(TimelineHistorySearchResponse {
            rows,
            scanned,
            matched,
            truncated,
            limit: query.limit(),
            retention_days: cfg.retention_days(),
            cf_name: ACTION_HISTORY_CF,
            key_prefix: ACTION_HISTORY_PREFIX,
        })
    }

    /// Process name and title of the window the call acted on (the reply's
    /// target, else the session target); empty when there is none or it
    /// cannot be read.
    fn history_target_window(
        &self,
        session_id: Option<&str>,
        effective_target: Option<&Value>,
    ) -> (String, String) {
        let hwnd = effective_target
            .and_then(|target| target.get("hwnd").or_else(|| target.get("window_hwnd")))
            .and_then(Value::as_i64)
            .or_else(|| match self.session_target(session_id).ok().flatten()? {
                SessionTarget::Window { hwnd } => Some(hwnd),
                SessionTarget::Cdp { window_hwnd, .. } => Some(window_hwnd),
            });
        let Some(hwnd) = hwnd else {
            return (String::new(), String::new());
        };
        let root = synapse_a11y::top_level_root_hwnd(hwnd).unwrap_or(hwnd);
        synapse_a11y::foreground_context(root).map_or_else(
            |_error| (String::new(), String::new()),
            |context| (context.process_name, context.window_title),
        )
    }
}

/// Writes one row after removing expired rows. When the namespace quota
/// refuses the write, the oldest `quota_prune_rows` rows are removed and
/// the write is tried once more.
fn write_history_row(
    db: &Db,
    row: &ActionHistoryRow,
    cfg: &ActionHistoryConfig,
) -> Result<(), String> {
    let key = row.key(ACTION_HISTORY_SEQ.fetch_add(1, Ordering::Relaxed));
    let encoded = row.encode().map_err(|error| error.to_string())?;
    let cutoff = ActionHistoryRow::time_key(cfg.cutoff_ms(row.ts_ms()));
    delete_oldest(db, cfg.quota_prune_rows(), Some(cutoff.as_bytes()))?;
    let put = || db.put_batch(cf::CF_KV, [(key.as_bytes().to_vec(), encoded.clone())]);
    match put() {
        Ok(()) => Ok(()),
        Err(StorageError::NamespaceQuotaReached { .. }) => {
            delete_oldest(db, cfg.quota_prune_rows(), None)?;
            put().map_err(|error| error.to_string())
        }
        Err(error) => Err(error.to_string()),
    }
}

/// Deletes up to `max_rows` of the oldest history rows; with `before`, only
/// rows whose key sorts before it (rows older than the retention cut-off).
fn delete_oldest(db: &Db, max_rows: usize, before: Option<&[u8]>) -> Result<(), String> {
    let prefix = ACTION_HISTORY_PREFIX.as_bytes();
    // Common case first: one iterator step shows whether anything expired.
    let (first, _more) = db
        .scan_cf_from(cf::CF_KV, prefix, 1)
        .map_err(|error| error.to_string())?;
    let expired = |key: &[u8]| key.starts_with(prefix) && before.is_none_or(|before| key < before);
    if !first.first().is_some_and(|(key, _value)| expired(key)) {
        return Ok(());
    }
    let (rows, _more) = db
        .scan_cf_prefix_window(cf::CF_KV, prefix, prefix, max_rows, usize::MAX)
        .map_err(|error| error.to_string())?;
    let keys: Vec<Vec<u8>> = rows
        .into_iter()
        .map(|(key, _value)| key)
        .take_while(|key| expired(key))
        .collect();
    db.delete_batch(cf::CF_KV, keys)
        .map_err(|error| error.to_string())
}
