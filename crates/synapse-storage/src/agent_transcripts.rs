//! `CF_AGENT_TRANSCRIPTS` key codec (#900).
//!
//! Keys are `spawn_id bytes || 0x00 || line_no (8 bytes BE)`. Spawn ids are
//! strictly `agent-spawn-` plus ASCII alphanumerics/dashes (enforced at
//! ingest), so the `0x00` separator can never appear inside an id and the
//! key space is unambiguous. Rows for one spawn iterate contiguously in
//! source-line order under a prefix scan, and re-ingesting a line always
//! lands on the same key — ingestion is idempotent by construction.
//!
//! Every producer and consumer must encode/decode through this module so a
//! malformed key is a structured error, never a silent skip.

use crate::{StorageError, StorageResult, cf};

/// Separator between the spawn id and the line number.
const KEY_SEPARATOR: u8 = 0x00;

/// Encodes a `CF_AGENT_TRANSCRIPTS` row key.
#[must_use]
pub fn agent_transcript_key(spawn_id: &str, line_no: u64) -> Vec<u8> {
    let mut key = Vec::with_capacity(spawn_id.len() + 1 + 8);
    key.extend_from_slice(spawn_id.as_bytes());
    key.push(KEY_SEPARATOR);
    key.extend_from_slice(&line_no.to_be_bytes());
    key
}

/// Encodes the prefix that scans all rows of one spawn.
#[must_use]
pub fn agent_transcript_spawn_prefix(spawn_id: &str) -> Vec<u8> {
    let mut prefix = Vec::with_capacity(spawn_id.len() + 1);
    prefix.extend_from_slice(spawn_id.as_bytes());
    prefix.push(KEY_SEPARATOR);
    prefix
}

/// Decodes a `CF_AGENT_TRANSCRIPTS` row key into `(spawn_id, line_no)`.
///
/// # Errors
///
/// Returns [`StorageError::ReadFailed`] when the key lacks the separator,
/// the spawn id bytes are not UTF-8, or the line-number suffix is not
/// exactly 8 bytes.
pub fn decode_agent_transcript_key(key: &[u8]) -> StorageResult<(String, u64)> {
    let invalid = |detail: String| StorageError::ReadFailed {
        cf_name: cf::CF_AGENT_TRANSCRIPTS.to_owned(),
        detail,
    };
    let separator_at = key
        .iter()
        .position(|byte| *byte == KEY_SEPARATOR)
        .ok_or_else(|| {
            invalid("AGENT_TRANSCRIPT_KEY_INVALID: missing 0x00 separator".to_owned())
        })?;
    let (id_bytes, rest) = key.split_at(separator_at);
    let line_bytes = &rest[1..];
    if line_bytes.len() != 8 {
        return Err(invalid(format!(
            "AGENT_TRANSCRIPT_KEY_INVALID: expected 8 line-number bytes after separator, got {}",
            line_bytes.len()
        )));
    }
    let spawn_id = std::str::from_utf8(id_bytes)
        .map_err(|_e| {
            invalid("AGENT_TRANSCRIPT_KEY_INVALID: spawn id bytes are not UTF-8".to_owned())
        })?
        .to_owned();
    let line_no = u64::from_be_bytes(line_bytes.try_into().map_err(|_e| {
        invalid("AGENT_TRANSCRIPT_KEY_INVALID: line-number bytes unreadable".to_owned())
    })?);
    Ok((spawn_id, line_no))
}
