//! Durable hand-off and lessons ledger: the pure rules (Phase 7 part 3).
//!
//! One `CF_KV` row per scope + topic, outside every workspace run namespace,
//! so it survives a daemon restart. The daemon mirrors the rows one-way into a
//! human-readable text file next to its database; the owner-notes section at
//! the end of that file is copied through unchanged and never rewritten.

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const LEDGER_KEY_PREFIX: &str = "workspace-ledger/v1";
pub const LEDGER_SCHEMA_VERSION: u32 = 1;
pub const MAX_LEDGER_SCOPE_CHARS: usize = 64;
pub const MAX_LEDGER_TOPIC_CHARS: usize = 128;
pub const MAX_LEDGER_SUMMARY_CHARS: usize = 1024;
pub const MAX_LEDGER_NEXT_STEPS: usize = 16;
pub const MAX_LEDGER_STEP_CHARS: usize = 256;
pub const MAX_LEDGER_SOURCE_REFS: usize = 8;
pub const MAX_LEDGER_SOURCE_REF_CHARS: usize = 512;
pub const LEDGER_RETENTION_MS: u64 = 90 * 24 * 60 * 60 * 1000;
pub const OWNER_NOTES_MARKER: &str = "## Owner notes (Synapse never rewrites below this line)";

const LEDGER_FILE_HEADER: &str = "# Synapse ledger\n\nWritten by the Synapse daemon from its database. Edits above the owner-notes line are replaced on the next ledger write.\n";
const FIELD_INDENT: &str = "    ";

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerKind {
    Handoff,
    Useful,
    DeadEnd,
    Correction,
}

impl LedgerKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Handoff => "handoff",
            Self::Useful => "useful",
            Self::DeadEnd => "dead_end",
            Self::Correction => "correction",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerTopicRef {
    pub scope: String,
    pub topic: String,
}

/// One `ledger_put` request.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerInput {
    /// Area the entry belongs to, e.g. "ops" or "synapse". Letters, digits,
    /// space, '-', '_' and '.'; at most 64 characters.
    pub scope: String,
    /// Topic inside the scope; same characters, at most 128 characters.
    pub topic: String,
    pub kind: LedgerKind,
    /// At most 1024 characters. Token-like secrets are stored redacted.
    pub summary: String,
    /// At most 16 single-line steps of at most 256 characters.
    #[serde(default)]
    pub next_steps: Vec<String>,
    /// At most 8 single-line references (issue number, file path, row key).
    #[serde(default)]
    pub source_refs: Vec<String>,
    /// Required for kind=correction: the other entry this one corrects. The
    /// corrected entry is never changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub corrects: Option<LedgerTopicRef>,
    /// Required when the scope/topic row already exists. Omit to create only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerRecord {
    pub schema_version: u32,
    pub row_key: String,
    pub scope: String,
    pub topic: String,
    pub kind: LedgerKind,
    pub summary: String,
    pub next_steps: Vec<String>,
    pub source_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub corrects: Option<LedgerTopicRef>,
    pub writer_session_id: String,
    pub version: u64,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LedgerRefusal {
    Invalid(String),
    VersionConflict {
        expected: Option<u64>,
        current: Option<u64>,
    },
}

/// What to do with the mirror file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LedgerMirrorPlan {
    Write(String),
    /// The existing file has no owner-notes marker, so the owner rewrote it;
    /// the daemon leaves it untouched.
    KeepOwnerFile,
}

/// `CF_KV` row key: scope and topic only, hex-encoded, never a run id.
#[must_use]
pub fn ledger_row_key(scope: &str, topic: &str) -> String {
    format!("{}topic_hex/{}", ledger_scope_prefix(scope), hex(topic))
}

/// Key prefix of every row in one scope.
#[must_use]
pub fn ledger_scope_prefix(scope: &str) -> String {
    format!("{LEDGER_KEY_PREFIX}/scope_hex/{}/", hex(scope))
}

/// Validates a request and builds the row to store. `current` is the row
/// stored under the same scope/topic, if any; an expired row counts as absent.
///
/// # Errors
/// `Invalid` for a field outside the rules, `VersionConflict` when
/// `expected_version` does not match the live row.
pub fn build_ledger_record(
    input: LedgerInput,
    writer_session_id: &str,
    current: Option<&LedgerRecord>,
    now_unix_ms: u64,
) -> Result<LedgerRecord, LedgerRefusal> {
    let input = validate_ledger_input(input)?;
    let current = current.filter(|row| !is_ledger_expired(row, now_unix_ms));
    let current_version = current.map(|row| row.version);
    let version = match (input.expected_version, current_version) {
        (None, None) => 1,
        (Some(expected), Some(found)) if expected == found => found.saturating_add(1),
        (expected, found) => {
            return Err(LedgerRefusal::VersionConflict {
                expected,
                current: found,
            });
        }
    };
    Ok(LedgerRecord {
        schema_version: LEDGER_SCHEMA_VERSION,
        row_key: ledger_row_key(&input.scope, &input.topic),
        scope: input.scope,
        topic: input.topic,
        kind: input.kind,
        summary: input.summary,
        next_steps: input.next_steps,
        source_refs: input.source_refs,
        corrects: input.corrects,
        writer_session_id: writer_session_id.to_owned(),
        version,
        created_at_unix_ms: current.map_or(now_unix_ms, |row| row.created_at_unix_ms),
        updated_at_unix_ms: now_unix_ms,
        expires_at_unix_ms: now_unix_ms.saturating_add(LEDGER_RETENTION_MS),
    })
}

/// A row is expired from its expiry instant on.
#[must_use]
pub const fn is_ledger_expired(record: &LedgerRecord, now_unix_ms: u64) -> bool {
    record.expires_at_unix_ms <= now_unix_ms
}

/// Renders the mirror file.
///
/// The daemon part comes from `records`, then the owner-notes section is
/// copied byte for byte from `existing`. Every free-text line is indented, so
/// no field can start an entry heading or the marker.
#[must_use]
pub fn render_ledger_file(records: &[LedgerRecord], existing: Option<&str>) -> LedgerMirrorPlan {
    let owner_section = match existing.filter(|text| !text.trim().is_empty()) {
        None => format!("{OWNER_NOTES_MARKER}\n"),
        Some(text) => match owner_notes_section(text) {
            Some(section) => section.to_owned(),
            None => return LedgerMirrorPlan::KeepOwnerFile,
        },
    };
    let mut sorted: Vec<&LedgerRecord> = records.iter().collect();
    sorted.sort_by(|left, right| (&left.scope, &left.topic).cmp(&(&right.scope, &right.topic)));
    let mut text = String::from(LEDGER_FILE_HEADER);
    for record in sorted {
        text.push('\n');
        text.push_str(&render_record(record));
    }
    text.push('\n');
    text.push_str(&owner_section);
    LedgerMirrorPlan::Write(text)
}

/// The mirror file sits next to the database directory and is named after
/// it, so a candidate database never writes the live daemon's file.
#[must_use]
pub fn ledger_mirror_path(db_path: &Path) -> PathBuf {
    let name = db_path.file_name().map_or_else(
        || "synapse".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    db_path.with_file_name(format!("{name}.ledger.md"))
}

fn validate_ledger_input(input: LedgerInput) -> Result<LedgerInput, LedgerRefusal> {
    let scope = name_field("scope", &input.scope, MAX_LEDGER_SCOPE_CHARS)?;
    let topic = name_field("topic", &input.topic, MAX_LEDGER_TOPIC_CHARS)?;
    let summary = input.summary.trim().to_owned();
    if summary.is_empty() || summary.chars().count() > MAX_LEDGER_SUMMARY_CHARS {
        return Err(invalid(format!(
            "ledger summary must be 1..={MAX_LEDGER_SUMMARY_CHARS} characters"
        )));
    }
    if summary
        .chars()
        .any(|ch| ch.is_control() && ch != '\n' && ch != '\t')
    {
        return Err(invalid(
            "ledger summary must not contain control characters other than newline and tab",
        ));
    }
    let next_steps = line_list(
        "next_steps",
        input.next_steps,
        MAX_LEDGER_NEXT_STEPS,
        MAX_LEDGER_STEP_CHARS,
    )?;
    let source_refs = line_list(
        "source_refs",
        input.source_refs,
        MAX_LEDGER_SOURCE_REFS,
        MAX_LEDGER_SOURCE_REF_CHARS,
    )?;
    let corrects = match (input.kind, input.corrects) {
        (LedgerKind::Correction, None) => {
            return Err(invalid("ledger kind=correction requires corrects"));
        }
        (LedgerKind::Correction, Some(target)) => {
            let target = LedgerTopicRef {
                scope: name_field("corrects.scope", &target.scope, MAX_LEDGER_SCOPE_CHARS)?,
                topic: name_field("corrects.topic", &target.topic, MAX_LEDGER_TOPIC_CHARS)?,
            };
            if target.scope == scope && target.topic == topic {
                return Err(invalid(
                    "a correction is stored under its own topic; it never replaces the entry it corrects",
                ));
            }
            Some(target)
        }
        (_, Some(_)) => {
            return Err(invalid("corrects is accepted only with kind=correction"));
        }
        (_, None) => None,
    };
    Ok(LedgerInput {
        scope,
        topic,
        kind: input.kind,
        summary,
        next_steps,
        source_refs,
        corrects,
        expected_version: input.expected_version,
    })
}

fn name_field(field: &str, value: &str, max_chars: usize) -> Result<String, LedgerRefusal> {
    let trimmed = value.trim();
    let allowed = |ch: char| ch.is_alphanumeric() || matches!(ch, ' ' | '-' | '_' | '.');
    if trimmed.is_empty()
        || trimmed.chars().count() > max_chars
        || !trimmed.chars().all(allowed)
        || trimmed.starts_with('.')
    {
        return Err(invalid(format!(
            "ledger {field} must be 1..={max_chars} letters, digits, spaces, '-', '_' or '.', not starting with '.'"
        )));
    }
    Ok(trimmed.to_owned())
}

fn line_list(
    field: &str,
    values: Vec<String>,
    max_items: usize,
    max_chars: usize,
) -> Result<Vec<String>, LedgerRefusal> {
    if values.len() > max_items {
        return Err(invalid(format!(
            "ledger {field} accepts at most {max_items} items"
        )));
    }
    values
        .into_iter()
        .map(|value| {
            let trimmed = value.trim();
            if trimmed.is_empty()
                || trimmed.chars().count() > max_chars
                || trimmed.chars().any(char::is_control)
            {
                Err(invalid(format!(
                    "each ledger {field} item must be one line of 1..={max_chars} characters"
                )))
            } else {
                Ok(trimmed.to_owned())
            }
        })
        .collect()
}

fn owner_notes_section(text: &str) -> Option<&str> {
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        if line.trim_end_matches(['\r', '\n']) == OWNER_NOTES_MARKER {
            return text.get(offset..);
        }
        offset += line.len();
    }
    None
}

fn render_record(record: &LedgerRecord) -> String {
    let mut out = format!(
        "### {} / {}\n{FIELD_INDENT}kind: {} | version: {} | writer: {} | updated_at_unix_ms: {}\n",
        record.scope,
        record.topic,
        record.kind.as_str(),
        record.version,
        record.writer_session_id,
        record.updated_at_unix_ms
    );
    if let Some(target) = &record.corrects {
        let corrected = [target.scope.as_str(), target.topic.as_str()].join(" / ");
        push_field_line(&mut out, "corrects: ", &corrected);
    }
    for line in record.summary.lines() {
        push_field_line(&mut out, "", line);
    }
    for step in &record.next_steps {
        push_field_line(&mut out, "next: ", step);
    }
    for source in &record.source_refs {
        push_field_line(&mut out, "source: ", source);
    }
    out
}

/// One indented free-text line: `<indent><label><value>\n`.
fn push_field_line(out: &mut String, label: &str, value: &str) {
    out.push_str(FIELD_INDENT);
    out.push_str(label);
    out.push_str(value);
    out.push('\n');
}

fn invalid(message: impl Into<String>) -> LedgerRefusal {
    LedgerRefusal::Invalid(message.into())
}

fn hex(value: &str) -> String {
    use std::fmt::Write as _;
    value
        .bytes()
        .fold(String::with_capacity(value.len() * 2), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}
