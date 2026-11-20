//! Keyed span digests for hygiene flag rows (#161).
//!
//! A hygiene flag used to persist the detected span text in plaintext (plus a
//! plain SHA-256 of it) in `CF_KV`, i.e. a second copy of whatever the scanner
//! matched, outliving redaction of the source row. Flags now store only the
//! span kind/location (field + byte offsets) and an HMAC-SHA256 digest under
//! the per-install content-digest key. Redaction re-locates the span in the
//! source row by offset first, then by sliding a window of the recorded length
//! and comparing digests, so the secret never has to be stored.
//!
//! Legacy rows (pre-#161) are still readable: their `span_text_sha256` is an
//! unprefixed plain SHA-256 and verifies the same way. A one-time migration
//! rewrites them to the keyed shape and compacts the flag range so the old
//! plaintext does not linger in SST files.

use std::sync::atomic::{AtomicBool, Ordering};

use rmcp::ErrorData;
use synapse_core::error_codes;
use synapse_reflex::ReflexRuntime;
use synapse_storage::{cf, decode_json, encode_json};

use super::{
    FLAG_PREFIX, HygieneFlagRecord, STORAGE_SCAN_CHUNK_ROWS, hex_encode, key_after, mcp_error,
    sha256_hex,
};

/// Prefix of every keyed digest persisted in a flag row. Unprefixed digests are
/// legacy plain SHA-256 values.
pub(super) const KEYED_DIGEST_PREFIX: &str = "hmac-sha256:";

/// Exclusive upper bound of the flag key range (`FLAG_PREFIX` ends in `/`,
/// `0` is the next ASCII byte).
const FLAG_RANGE_END: &[u8] = b"hygiene/flag/v10";

/// Keyed digest of a span, in the form persisted in flag rows.
pub(super) fn keyed_span_digest(text: &str) -> String {
    format!(
        "{KEYED_DIGEST_PREFIX}{}",
        crate::content_digest::keyed_text_digest_hex(text)
    )
}

/// True when `candidate` hashes to the persisted `digest` (keyed or legacy).
pub(super) fn span_digest_matches(digest: &str, candidate: &str) -> bool {
    if digest.starts_with(KEYED_DIGEST_PREFIX) {
        keyed_span_digest(candidate) == digest
    } else {
        sha256_hex(candidate.as_bytes()) == digest
    }
}

/// Byte length of the flagged span. A legacy row that still carries its text
/// is authoritative; otherwise the recorded offsets give the length.
fn span_len(record: &HygieneFlagRecord) -> usize {
    if record.span_text.is_empty() {
        let start = usize::try_from(record.span_start).unwrap_or(usize::MAX);
        let end = usize::try_from(record.span_end).unwrap_or(0);
        end.saturating_sub(start)
    } else {
        record.span_text.len()
    }
}

/// Finds the flagged span in `current`: the recorded offsets first, then every
/// char-boundary window of the recorded length (offset drift after an earlier
/// redaction shortened the field). Returns the byte range.
pub(super) fn locate_span(current: &str, record: &HygieneFlagRecord) -> Option<(usize, usize)> {
    let len = span_len(record);
    if len == 0 || len > current.len() {
        return None;
    }
    let digest = record.span_text_sha256.as_str();
    let matches_at = |start: usize, end: usize| {
        current.is_char_boundary(start)
            && current.is_char_boundary(end)
            && span_digest_matches(digest, &current[start..end])
    };
    let start = usize::try_from(record.span_start).unwrap_or(usize::MAX);
    if let Some(end) = start.checked_add(len)
        && end <= current.len()
        && matches_at(start, end)
    {
        return Some((start, end));
    }
    (0..=current.len() - len)
        .filter(|start| current.is_char_boundary(*start))
        .find(|start| matches_at(*start, *start + len))
        .map(|start| (start, start + len))
}

/// Rewrites one decoded legacy flag row into the keyed, text-free shape.
/// Returns `None` when the row is already clean.
fn migrate_record(mut record: HygieneFlagRecord) -> Option<HygieneFlagRecord> {
    let mut changed = false;
    if !record.span_text.is_empty() {
        record.span_text_sha256 = keyed_span_digest(&record.span_text);
        record.span_text.clear();
        changed = true;
    }
    // The whole-source-text digest cannot be re-keyed without the source row;
    // it is informational only, so a legacy plain value is dropped.
    if !record.source_text_sha256.is_empty()
        && !record.source_text_sha256.starts_with(KEYED_DIGEST_PREFIX)
    {
        record.source_text_sha256.clear();
        changed = true;
    }
    changed.then_some(record)
}

/// One-time migration of legacy flag rows (see module docs). Returns the
/// number of rows rewritten. Rows that fail to decode are skipped (the read
/// paths already log them).
pub(crate) fn purge_legacy_flag_plaintext(runtime: &ReflexRuntime) -> Result<u64, ErrorData> {
    let prefix = FLAG_PREFIX.as_bytes();
    let mut start = prefix.to_vec();
    let mut rewritten = 0_u64;
    loop {
        let rows = runtime
            .storage_cf_prefix_rows_from(cf::CF_KV, prefix, &start, STORAGE_SCAN_CHUNK_ROWS)
            .map_err(|error| mcp_error(error.code(), error.to_string()))?;
        let Some((last_key, _)) = rows.last() else {
            break;
        };
        let next_start = key_after(last_key);
        let page_len = rows.len();
        let mut updates = Vec::new();
        for (key, value) in rows {
            let Ok(record) = decode_json::<HygieneFlagRecord>(&value) else {
                continue;
            };
            let Some(migrated) = migrate_record(record) else {
                continue;
            };
            let encoded = encode_json(&migrated).map_err(|error| {
                mcp_error(
                    error.code(),
                    format!(
                        "HYGIENE_FLAG_MIGRATE_ENCODE_FAILED at {}: {error}",
                        hex_encode(&key)
                    ),
                )
            })?;
            updates.push((key, encoded));
        }
        if !updates.is_empty() {
            rewritten = rewritten.saturating_add(updates.len() as u64);
            // Rewrites only shrink rows, so they bypass the ingestion gate.
            runtime
                .storage_put_rows_pressure_bypass(cf::CF_KV, updates)
                .map_err(|error| {
                    mcp_error(
                        error.code(),
                        format!("HYGIENE_FLAG_MIGRATE_WRITE_FAILED: {error}"),
                    )
                })?;
        }
        if page_len < STORAGE_SCAN_CHUNK_ROWS {
            break;
        }
        start = next_start;
    }
    if rewritten > 0 {
        runtime
            .storage_flush()
            .and_then(|()| runtime.storage_compact_cf_range(cf::CF_KV, prefix, FLAG_RANGE_END))
            .map_err(|error| {
                mcp_error(
                    error.code(),
                    format!("HYGIENE_FLAG_MIGRATE_COMPACT_FAILED: {error}"),
                )
            })?;
    }
    Ok(rewritten)
}

static LEGACY_FLAGS_MIGRATED: AtomicBool = AtomicBool::new(false);

/// Runs [`purge_legacy_flag_plaintext`] once per process, before the first
/// hygiene flag read or write. Failure is logged and retried on the next call;
/// it never blocks the caller (old rows stay readable).
pub(super) fn ensure_legacy_flags_migrated(runtime: &ReflexRuntime) {
    if LEGACY_FLAGS_MIGRATED.swap(true, Ordering::AcqRel) {
        return;
    }
    match purge_legacy_flag_plaintext(runtime) {
        Ok(0) => {}
        Ok(rewritten) => tracing::warn!(
            code = "HYGIENE_FLAG_LEGACY_PLAINTEXT_PURGED",
            rewritten,
            "rewrote legacy hygiene flag rows without span plaintext"
        ),
        Err(error) => {
            LEGACY_FLAGS_MIGRATED.store(false, Ordering::Release);
            tracing::warn!(
                code = error_codes::STORAGE_WRITE_FAILED,
                error = %error.message,
                "could not migrate legacy hygiene flag rows; will retry"
            );
        }
    }
}
