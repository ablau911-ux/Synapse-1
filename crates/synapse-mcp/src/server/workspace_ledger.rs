//! Durable hand-off and lessons ledger on the `workspace` facade (Phase 7
//! part 3): operations `ledger_put` and `ledger_list`.
//!
//! The rules live in `synapse_core::ledger`; this layer stores the row in
//! `CF_KV` with an exact readback, removes expired ledger rows, and mirrors
//! the live rows one-way into a text file next to the daemon database.

use std::{fs, io::ErrorKind, path::PathBuf};

use rmcp::model::ErrorCode;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use synapse_core::{
    error_codes,
    ledger::{
        LEDGER_KEY_PREFIX, LedgerInput, LedgerMirrorPlan, LedgerRecord, LedgerRefusal,
        build_ledger_record, is_ledger_expired, ledger_mirror_path, ledger_row_key,
        ledger_scope_prefix, render_ledger_file,
    },
};
use synapse_storage::{Db, cf};

use super::{
    ErrorData, SynapseService, WorkspaceRowReadback, delete_workspace_rows, hash_bytes, mcp_error,
    params_error, read_exact_kv_row, readback_exact_workspace_row, unix_time_ms_now,
    validate_session_id, workspace_write_lock,
};
use crate::credential_redaction::{REDACTED, token_like_value};

const DEFAULT_LEDGER_LIST_LIMIT: usize = 100;
const MAX_LEDGER_LIST_LIMIT: usize = 500;
const LEDGER_SOURCE_OF_TRUTH: &str = "CF_KV workspace-ledger exact row";

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceLedgerListParams {
    /// Only entries of this scope. Omit for every scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default = "default_ledger_list_limit")]
    #[schemars(default = "default_ledger_list_limit", range(min = 1, max = 500))]
    pub limit: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LedgerMirrorStatus {
    Written,
    /// The file has no owner-notes marker; the owner rewrote it, so it was
    /// left untouched. The database row is still the truth.
    KeptOwnerFile,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceLedgerMirrorReadback {
    pub path: String,
    pub status: LedgerMirrorStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes_len: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceLedgerPutResponse {
    pub ok: bool,
    pub row_key: String,
    pub version: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_version: Option<u64>,
    pub entry: LedgerRecord,
    pub storage_readback: WorkspaceRowReadback,
    pub expired_rows_deleted: usize,
    pub mirror: WorkspaceLedgerMirrorReadback,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceLedgerListResponse {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    pub now_unix_ms: u64,
    pub live_count: usize,
    pub returned_count: usize,
    pub expired_rows_skipped: usize,
    pub corrupt_rows_skipped: usize,
    pub entries: Vec<LedgerRecord>,
    pub mirror_path: String,
}

struct LedgerScan {
    live: Vec<LedgerRecord>,
    expired_keys: Vec<Vec<u8>>,
    corrupt_rows: usize,
}

impl SynapseService {
    pub(super) fn workspace_ledger_put_impl(
        &self,
        input: LedgerInput,
        writer_session_id: &str,
    ) -> Result<WorkspaceLedgerPutResponse, ErrorData> {
        validate_session_id(writer_session_id)?;
        let input = redact_ledger_input(input)?;
        let now_unix_ms = unix_time_ms_now();
        let write_guard = workspace_write_lock()?;
        let db = self.workspace_db()?;
        let row_key = ledger_row_key(input.scope.trim(), input.topic.trim());
        let current = read_ledger_row(&db, &row_key)?;
        let previous_version = current
            .as_ref()
            .filter(|row| !is_ledger_expired(row, now_unix_ms))
            .map(|row| row.version);
        let record = build_ledger_record(input, writer_session_id, current.as_ref(), now_unix_ms)
            .map_err(|refusal| ledger_refusal_error(refusal, &row_key))?;
        let encoded = synapse_storage::encode_json(&record).map_err(|error| {
            mcp_error(
                error_codes::STORAGE_WRITE_FAILED,
                format!("encode ledger row {row_key}: {error}"),
            )
        })?;
        db.put_batch(cf::CF_KV, [(row_key.as_bytes().to_vec(), encoded)])
            .and_then(|()| db.flush())
            .map_err(|error| {
                mcp_error(error.code(), format!("write ledger row {row_key}: {error}"))
            })?;
        let storage_readback = readback_exact_workspace_row(&db, &row_key)?;
        let scan = scan_ledger_rows(&db, LEDGER_KEY_PREFIX, now_unix_ms)?;
        let expired_rows_deleted = scan.expired_keys.len();
        delete_workspace_rows(&db, scan.expired_keys, "delete expired ledger rows")?;
        let mirror = self.write_ledger_mirror(&scan.live)?;
        drop(write_guard);
        tracing::info!(
            code = "WORKSPACE_LEDGER_PUT_COMMITTED",
            row_key = %row_key,
            writer_session_id = %writer_session_id,
            version = record.version,
            mirror_status = ?mirror.status,
            "workspace ledger row committed"
        );
        Ok(WorkspaceLedgerPutResponse {
            ok: true,
            row_key,
            version: record.version,
            previous_version,
            entry: record,
            storage_readback,
            expired_rows_deleted,
            mirror,
        })
    }

    pub(super) fn workspace_ledger_list_impl(
        &self,
        params: WorkspaceLedgerListParams,
    ) -> Result<WorkspaceLedgerListResponse, ErrorData> {
        if params.limit == 0 || params.limit > MAX_LEDGER_LIST_LIMIT {
            return Err(params_error(format!(
                "ledger_list limit must be 1..={MAX_LEDGER_LIST_LIMIT}"
            )));
        }
        let scope = match params.scope.as_deref().map(str::trim) {
            Some("") => return Err(params_error("ledger_list scope must not be empty")),
            other => other.map(str::to_owned),
        };
        let prefix = scope
            .as_deref()
            .map_or_else(|| format!("{LEDGER_KEY_PREFIX}/"), ledger_scope_prefix);
        let now_unix_ms = unix_time_ms_now();
        let db = self.workspace_db()?;
        let mut scan = scan_ledger_rows(&db, &prefix, now_unix_ms)?;
        scan.live
            .sort_by(|left, right| (&left.scope, &left.topic).cmp(&(&right.scope, &right.topic)));
        let live_count = scan.live.len();
        scan.live.truncate(params.limit);
        Ok(WorkspaceLedgerListResponse {
            ok: true,
            scope,
            now_unix_ms,
            live_count,
            returned_count: scan.live.len(),
            expired_rows_skipped: scan.expired_keys.len(),
            corrupt_rows_skipped: scan.corrupt_rows,
            entries: scan.live,
            mirror_path: self.ledger_mirror_file()?.display().to_string(),
        })
    }

    fn ledger_mirror_file(&self) -> Result<PathBuf, ErrorData> {
        let state = self.m3_state_handle();
        let guard = state.lock().map_err(|_error| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                "M3 service state lock poisoned while resolving the ledger file",
            )
        })?;
        let db_path = guard
            .db_path
            .clone()
            .unwrap_or_else(crate::m3::default_db_path);
        Ok(ledger_mirror_path(&db_path))
    }

    /// One-way mirror. A file failure never undoes the stored row; it is
    /// reported as `failed` with the error.
    fn write_ledger_mirror(
        &self,
        live: &[LedgerRecord],
    ) -> Result<WorkspaceLedgerMirrorReadback, ErrorData> {
        let path = self.ledger_mirror_file()?;
        let readback =
            |status, bytes: Option<&[u8]>, error: Option<String>| WorkspaceLedgerMirrorReadback {
                path: path.display().to_string(),
                status,
                bytes_len: bytes.map(|bytes| bytes.len() as u64),
                digest: bytes.map(hash_bytes),
                error,
            };
        let existing = match fs::read_to_string(&path) {
            Ok(text) => Some(text),
            Err(error) if error.kind() == ErrorKind::NotFound => None,
            Err(error) => {
                return Ok(readback(
                    LedgerMirrorStatus::Failed,
                    None,
                    Some(format!("read {}: {error}", path.display())),
                ));
            }
        };
        let text = match render_ledger_file(live, existing.as_deref()) {
            LedgerMirrorPlan::KeepOwnerFile => {
                return Ok(readback(
                    LedgerMirrorStatus::KeptOwnerFile,
                    existing.as_deref().map(str::as_bytes),
                    None,
                ));
            }
            LedgerMirrorPlan::Write(text) => text,
        };
        let temp = path.with_extension("md.tmp");
        let written = fs::write(&temp, text.as_bytes())
            .and_then(|()| fs::rename(&temp, &path))
            .and_then(|()| fs::read(&path));
        Ok(match written {
            Ok(bytes) if bytes == text.as_bytes() => {
                readback(LedgerMirrorStatus::Written, Some(&bytes), None)
            }
            Ok(bytes) => readback(
                LedgerMirrorStatus::Failed,
                Some(&bytes),
                Some("ledger file readback differs from the rendered text".to_owned()),
            ),
            Err(error) => readback(
                LedgerMirrorStatus::Failed,
                None,
                Some(format!("write {}: {error}", path.display())),
            ),
        })
    }
}

/// Replaces every token-like word (known token prefixes, long mixed
/// letter-digit runs) with the redaction marker; whitespace and plain words
/// are kept as they are.
pub(crate) fn redact_ledger_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut word = String::new();
    for ch in text.chars() {
        if ch.is_whitespace() {
            push_redacted_word(&mut out, &mut word);
            out.push(ch);
        } else {
            word.push(ch);
        }
    }
    push_redacted_word(&mut out, &mut word);
    out
}

fn push_redacted_word(out: &mut String, word: &mut String) {
    if word.is_empty() {
        return;
    }
    if token_like_value(word) {
        out.push_str(REDACTED);
    } else {
        out.push_str(word);
    }
    word.clear();
}

fn redact_ledger_input(input: LedgerInput) -> Result<LedgerInput, ErrorData> {
    let identifiers = [input.scope.as_str(), input.topic.as_str()]
        .into_iter()
        .chain(
            input
                .corrects
                .iter()
                .flat_map(|reference| [reference.scope.as_str(), reference.topic.as_str()]),
        );
    for identifier in identifiers {
        if redact_ledger_text(identifier) != identifier {
            return Err(params_error(
                "ledger identifier looks like a credential; nothing was stored",
            ));
        }
    }
    Ok(LedgerInput {
        summary: redact_ledger_text(&input.summary),
        next_steps: input
            .next_steps
            .iter()
            .map(String::as_str)
            .map(redact_ledger_text)
            .collect(),
        source_refs: input
            .source_refs
            .iter()
            .map(String::as_str)
            .map(redact_ledger_text)
            .collect(),
        ..input
    })
}

fn read_ledger_row(db: &Db, row_key: &str) -> Result<Option<LedgerRecord>, ErrorData> {
    let Some(encoded) = read_exact_kv_row(db, row_key.as_bytes())? else {
        return Ok(None);
    };
    decode_ledger_row(row_key, &encoded)
        .map(Some)
        .map_err(|detail| {
            ErrorData::new(
                ErrorCode(-32099),
                format!("ledger row {row_key} is corrupt: {detail}"),
                Some(json!({
                    "code": error_codes::STORAGE_CORRUPTED,
                    "row_key": row_key,
                    "detail": detail,
                    "source_of_truth": LEDGER_SOURCE_OF_TRUTH,
                })),
            )
        })
}

fn decode_ledger_row(row_key: &str, encoded: &[u8]) -> Result<LedgerRecord, String> {
    let record: LedgerRecord = synapse_storage::decode_json(encoded)
        .map_err(|error| format!("decode ledger row {row_key}: {error}"))?;
    if record.row_key != row_key {
        return Err(format!(
            "ledger row key mismatch: stored {} actual {row_key}",
            record.row_key
        ));
    }
    Ok(record)
}

fn scan_ledger_rows(db: &Db, prefix: &str, now_unix_ms: u64) -> Result<LedgerScan, ErrorData> {
    let rows = db
        .scan_cf_prefix(cf::CF_KV, prefix.as_bytes())
        .map_err(|error| mcp_error(error.code(), error.to_string()))?;
    let mut scan = LedgerScan {
        live: Vec::new(),
        expired_keys: Vec::new(),
        corrupt_rows: 0,
    };
    for (key, encoded) in rows {
        match decode_ledger_row(&String::from_utf8_lossy(&key), &encoded) {
            Ok(record) if is_ledger_expired(&record, now_unix_ms) => scan.expired_keys.push(key),
            Ok(record) => scan.live.push(record),
            Err(_) => scan.corrupt_rows += 1,
        }
    }
    Ok(scan)
}

fn ledger_refusal_error(refusal: LedgerRefusal, row_key: &str) -> ErrorData {
    match refusal {
        LedgerRefusal::Invalid(message) => params_error(message),
        LedgerRefusal::VersionConflict { expected, current } => ErrorData::new(
            ErrorCode(-32099),
            format!(
                "ledger row {row_key} version precondition failed: expected_version={expected:?} current={current:?}; read it with ledger_list and retry with expected_version"
            ),
            Some(json!({
                "code": "WORKSPACE_VERSION_CONFLICT",
                "detail_code": "WORKSPACE_VERSION_CONFLICT",
                "row_key": row_key,
                "expected_version": expected,
                "current_version": current,
                "source_of_truth": LEDGER_SOURCE_OF_TRUTH,
            })),
        ),
    }
}

const fn default_ledger_list_limit() -> usize {
    DEFAULT_LEDGER_LIST_LIMIT
}

#[cfg(test)]
#[path = "workspace_ledger_table_ledger.rs"]
// The frozen table compares owned paths on purpose; it is never edited.
#[allow(clippy::cmp_owned)]
mod workspace_ledger_table_ledger;
