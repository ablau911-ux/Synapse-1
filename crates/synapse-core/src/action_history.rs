//! Searchable history of what was done (Phase 7 part 4): the pure rules.
//!
//! One small `CF_KV` row per finished mutating tool call, keyed by time so a
//! prefix scan returns rows in time order. This module builds the row, the
//! key, the retention cut-off and the search match; it reads no clock, OS or
//! storage. The daemon layer in `synapse-mcp` passes the time, the target and
//! its redaction function, and does the reads and writes.

use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// `CF_KV` key prefix of every history row (quota entry in
/// `retention::KV_NAMESPACE_QUOTAS`).
pub const ACTION_HISTORY_PREFIX: &str = "action-history/v1/";
/// Version written into every row; other versions are skipped on read.
pub const ACTION_HISTORY_RECORD_VERSION: u32 = 1;
/// Longest stored summary, in characters.
pub const MAX_SUMMARY_CHARS: usize = 200;
/// `history_search` rows returned when no limit is given.
pub const DEFAULT_SEARCH_LIMIT: usize = 50;
/// Most `history_search` rows one call returns.
pub const MAX_SEARCH_LIMIT: usize = 500;
/// Call arguments that may appear in a summary, in this order. Typed
/// content (`text`, `value`, shell `args`) is deliberately absent.
pub const SUMMARY_ARG_FIELDS: [&str; 19] = [
    "verb",
    "url",
    "path",
    "command",
    "key",
    "selector",
    "element_id",
    "role",
    "name",
    "title",
    "exe",
    "pid",
    "tab_id",
    "target_id",
    "scope",
    "topic",
    "agent_id",
    "spawn_id",
    "task_id",
];

/// Longest stored session id, tool, operation, target or outcome.
const MAX_FIELD_CHARS: usize = 200;
const TARGET_SEPARATOR: &str = " - ";
/// `act` carries its call spec in this child object.
const ACTION_SPEC_FIELD: &str = "action";
const MS_PER_DAY: u64 = 86_400_000;
const ENV_PREFIX: &str = "SYNAPSE_ACTION_HISTORY_";

/// Retention and quota tuning. `Default` is the shipped value; `from_env`
/// overrides a field with `SYNAPSE_ACTION_HISTORY_<FIELD>` (invalid values
/// keep the default).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActionHistoryConfig {
    /// Rows older than this many days are removed and never returned.
    retention_days: u64,
    /// Oldest rows dropped when the namespace quota refuses a write.
    quota_prune_rows: usize,
}

impl Default for ActionHistoryConfig {
    fn default() -> Self {
        Self {
            retention_days: 90,
            quota_prune_rows: 512,
        }
    }
}

impl ActionHistoryConfig {
    /// Defaults overridden by `SYNAPSE_ACTION_HISTORY_*` environment variables.
    #[must_use]
    pub fn from_env() -> Self {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// Defaults overridden by `lookup("SYNAPSE_ACTION_HISTORY_<FIELD>")`;
    /// a value that is not a positive number keeps the default.
    #[must_use]
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let defaults = Self::default();
        let read = |field: &str| lookup(&format!("{ENV_PREFIX}{field}"));
        Self {
            retention_days: parse_positive(read("RETENTION_DAYS"))
                .unwrap_or(defaults.retention_days),
            quota_prune_rows: parse_positive(read("QUOTA_PRUNE_ROWS"))
                .unwrap_or(defaults.quota_prune_rows),
        }
    }

    #[must_use]
    pub const fn retention_days(&self) -> u64 {
        self.retention_days
    }

    #[must_use]
    pub const fn quota_prune_rows(&self) -> usize {
        self.quota_prune_rows
    }

    /// Rows written before this instant are expired.
    #[must_use]
    pub const fn cutoff_ms(&self, now_ms: u64) -> u64 {
        now_ms.saturating_sub(self.retention_days.saturating_mul(MS_PER_DAY))
    }
}

fn parse_positive<T: FromStr + PartialOrd + Default>(raw: Option<String>) -> Option<T> {
    raw.and_then(|raw| raw.trim().parse().ok())
        .filter(|value| *value > T::default())
}

/// One stored history row (JSON in `CF_KV`).
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActionHistoryRow {
    record_version: u32,
    /// When the call finished (unix ms).
    ts_ms: u64,
    session_id: String,
    tool: String,
    operation: String,
    /// `<process name> - <window title>` of the session target, or empty.
    target: String,
    /// `ok` or the error code.
    outcome: String,
    /// One line of at most 200 characters, built from the call's own fields.
    summary: String,
}

/// The facts of one finished call, as the daemon read them.
#[derive(Clone, Copy, Debug)]
pub struct FinishedCall<'a> {
    pub ts_ms: u64,
    pub session_id: &'a str,
    pub tool: &'a str,
    pub operation: &'a str,
    pub target_app: &'a str,
    pub target_title: &'a str,
    pub outcome: &'a str,
    /// The call's arguments object.
    pub args: Option<&'a Value>,
}

impl ActionHistoryRow {
    /// Builds the row to store. Every text field goes through `redact` before
    /// it is cut to one line, so a secret is never split into a surviving
    /// fragment.
    #[must_use]
    pub fn build(call: &FinishedCall<'_>, redact: &dyn Fn(&str) -> String) -> Self {
        let target = Self::target_line(call.target_app, call.target_title);
        let summary =
            Self::summary_line(call.tool, call.operation, &target, call.args, call.outcome);
        let field = |text: &str| Self::one_line(&redact(text), MAX_FIELD_CHARS);
        Self {
            record_version: ACTION_HISTORY_RECORD_VERSION,
            ts_ms: call.ts_ms,
            session_id: field(call.session_id),
            tool: field(call.tool),
            operation: field(call.operation),
            target: field(&target),
            outcome: field(call.outcome),
            summary: Self::one_line(&redact(&summary), MAX_SUMMARY_CHARS),
        }
    }

    /// When the call finished (unix ms).
    #[must_use]
    pub const fn ts_ms(&self) -> u64 {
        self.ts_ms
    }

    /// `action-history/v1/<ts_ms:020>/<seq:010>`.
    #[must_use]
    pub fn key(&self, seq: u32) -> String {
        format!("{}{seq:010}", Self::time_key(self.ts_ms))
    }

    /// The lowest key of millisecond `ts_ms`: every row of that millisecond or
    /// later sorts at or after it, every earlier row before it.
    #[must_use]
    pub fn time_key(ts_ms: u64) -> String {
        format!("{ACTION_HISTORY_PREFIX}{ts_ms:020}/")
    }

    /// Encodes the row as JSON.
    ///
    /// # Errors
    /// Serialisation failure (not expected for this type).
    pub fn encode(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    /// A stored row of this version, or `None` for anything else.
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        serde_json::from_slice::<Self>(bytes)
            .ok()
            .filter(|row| row.record_version == ACTION_HISTORY_RECORD_VERSION)
    }

    /// `<app> - <title>`, either part alone, or empty.
    fn target_line(app: &str, title: &str) -> String {
        match (app.trim(), title.trim()) {
            ("", title) => title.to_owned(),
            (app, "") => app.to_owned(),
            (app, title) => format!("{app}{TARGET_SEPARATOR}{title}"),
        }
    }

    /// `<tool> <operation>[ <field>=<value>...][ on <target>] -> <outcome>`.
    ///
    /// Fields from `SUMMARY_ARG_FIELDS` found at the top level of `args`, else in
    /// the call's own spec object (`args.action` or `args.<operation>`). Other
    /// child objects (a storage `value`, shell `env`) are content and are never
    /// read. Not yet redacted or cut.
    fn summary_line(
        tool: &str,
        operation: &str,
        target: &str,
        args: Option<&Value>,
        outcome: &str,
    ) -> String {
        let mut parts: Vec<String> = [tool.trim(), operation.trim()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .map(str::to_owned)
            .collect();
        let args = args.and_then(Value::as_object);
        for name in SUMMARY_ARG_FIELDS {
            if let Some(value) =
                args.and_then(|args| Self::summary_value(args, operation.trim(), name))
            {
                parts.push(format!("{name}={value}"));
            }
        }
        if !target.trim().is_empty() {
            parts.push(format!("on {}", target.trim()));
        }
        parts.push(format!("-> {}", outcome.trim()));
        parts.join(" ")
    }

    /// The scalar `name` at the top level, else in the `action` or
    /// `<operation>` spec object.
    fn summary_value(args: &Map<String, Value>, operation: &str, name: &str) -> Option<String> {
        Self::scalar_text(args.get(name)).or_else(|| {
            [ACTION_SPEC_FIELD, operation]
                .into_iter()
                .filter_map(|spec| args.get(spec).and_then(Value::as_object))
                .find_map(|spec| Self::scalar_text(spec.get(name)))
        })
    }

    fn scalar_text(value: Option<&Value>) -> Option<String> {
        match value? {
            Value::String(text) if !text.trim().is_empty() => Some(text.trim().to_owned()),
            Value::Number(number) => Some(number.to_string()),
            Value::Bool(flag) => Some(flag.to_string()),
            _ => None,
        }
    }

    /// One line: control characters become spaces, ends trimmed, at most
    /// `max_chars` characters (never splits a character).
    fn one_line(text: &str, max_chars: usize) -> String {
        let line: String = text
            .chars()
            .map(|ch| if ch.is_control() { ' ' } else { ch })
            .collect();
        let cut: String = line.trim().chars().take(max_chars).collect();
        cut.trim_end().to_owned()
    }
}

/// One `history_search`: the query words, the time window and the row limit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryQuery {
    /// Lowercased query words; an empty or blank query has none.
    words: Vec<String>,
    since_ms: Option<u64>,
    until_ms: Option<u64>,
    limit: usize,
}

impl HistoryQuery {
    /// `limit`: `None` → 50; otherwise clamped to `1..=500`.
    #[must_use]
    pub fn new(
        query: &str,
        since_ms: Option<u64>,
        until_ms: Option<u64>,
        limit: Option<usize>,
    ) -> Self {
        Self {
            words: query.split_whitespace().map(str::to_lowercase).collect(),
            since_ms,
            until_ms,
            limit: match limit {
                None => DEFAULT_SEARCH_LIMIT,
                Some(0) => 1,
                Some(limit) if limit > MAX_SEARCH_LIMIT => MAX_SEARCH_LIMIT,
                Some(limit) => limit,
            },
        }
    }

    #[must_use]
    pub const fn limit(&self) -> usize {
        self.limit
    }

    /// The key a time-ordered scan starts at (the window's first millisecond).
    #[must_use]
    pub fn start_key(&self) -> String {
        ActionHistoryRow::time_key(self.since_ms.unwrap_or(0))
    }

    /// True when the row finished after the window, so a time-ordered scan
    /// can stop.
    #[must_use]
    pub fn is_past(&self, row: &ActionHistoryRow) -> bool {
        self.until_ms.is_some_and(|until| row.ts_ms > until)
    }

    /// True when the row is inside `[since_ms, until_ms]` and every word
    /// appears, case-insensitive, in its tool, operation, target or summary.
    /// Words are plain text, never patterns.
    #[must_use]
    pub fn matches(&self, row: &ActionHistoryRow) -> bool {
        if self.since_ms.is_some_and(|since| row.ts_ms < since) || self.is_past(row) {
            return false;
        }
        // Fields are joined by a newline, which no query word contains, so a
        // word never matches across two fields.
        let searched = [&row.tool, &row.operation, &row.target, &row.summary]
            .map(|field| field.to_lowercase());
        let searched = searched.join("\n");
        self.words
            .iter()
            .all(|word| searched.contains(word.as_str()))
    }

    /// Rows given in time order, returned newest first and cut to the limit;
    /// the flag is true when rows were cut.
    #[must_use]
    pub fn newest_first(&self, mut rows: Vec<ActionHistoryRow>) -> (Vec<ActionHistoryRow>, bool) {
        rows.reverse();
        let truncated = rows.len() > self.limit;
        rows.truncate(self.limit);
        (rows, truncated)
    }
}

/// `ok` for a call that did not fail, else its error code, else `error`.
#[must_use]
pub fn outcome_label(failed: bool, code: Option<&str>) -> String {
    if !failed {
        return "ok".to_owned();
    }
    code.map(str::trim)
        .filter(|code| !code.is_empty())
        .unwrap_or("error")
        .to_owned()
}

/// The call's `operation` argument, else the operation its reply named.
#[must_use]
pub fn resolve_operation(argument: Option<&str>, reply: Option<&str>) -> Option<String> {
    [argument, reply]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|operation| !operation.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
#[path = "action_history_table.rs"]
// The frozen table's row tuples are its own; it is never edited.
#[allow(clippy::type_complexity)]
mod action_history_table;
