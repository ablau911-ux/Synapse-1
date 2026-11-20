//! At-rest sealing of `CF_OCR_CACHE` rows (#52).
//!
//! A cache row embeds the complete recognized screen text, which can contain
//! tokens, recovery codes and one-time passwords. A sealed row is a small JSON
//! envelope
//!
//! ```json
//! {"ts_ns": 1760000000000000000, "sealed_v": 1, "blob": "<hex of DPAPI(row JSON)>"}
//! ```
//!
//! The row JSON is protected with DPAPI (CurrentUser, purpose entropy), so an
//! offline copy of the database cannot read it. `ts_ns` is the only plaintext
//! field: the storage TTL compaction filter reads it, so sealed rows still
//! expire with the CF retention. DPAPI does not protect against another
//! process running as the same Windows user.
//!
//! Rows written before this change are plain JSON without `ts_ns` (they were
//! never TTL-evicted and fall only to the size caps); [`open`] still reads
//! them and nothing writes plaintext back: every writer must go through
//! [`seal`], which fails closed when DPAPI is unavailable.

use std::borrow::Cow;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

const ENTROPY: &[u8] = b"synapse/ocr-cache-row/v1";
const SEALED_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct SealedEnvelope {
    ts_ns: u64,
    sealed_v: u32,
    blob: String,
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX)
        })
}

/// Seals serialized row JSON for storage. Fails (rather than storing
/// plaintext) when DPAPI cannot protect the bytes.
pub(crate) fn seal(plain_json: &[u8]) -> Result<Vec<u8>> {
    seal_at(plain_json, now_ns())
}

/// Re-seals an edited row (e.g. after redaction) keeping the original
/// `ts_ns`, so rewriting a row never extends its retention. A legacy
/// plaintext original has no `ts_ns`; the row is then stamped now.
pub(crate) fn reseal(original_stored: &[u8], edited_plain_json: &[u8]) -> Result<Vec<u8>> {
    let ts_ns = serde_json::from_slice::<serde_json::Value>(original_stored)
        .ok()
        .and_then(|value| value.get("ts_ns").and_then(serde_json::Value::as_u64))
        .unwrap_or_else(now_ns);
    seal_at(edited_plain_json, ts_ns)
}

fn seal_at(plain_json: &[u8], ts_ns: u64) -> Result<Vec<u8>> {
    let blob = crate::secret_crypto::protect_with_entropy(plain_json, ENTROPY)?;
    let envelope = SealedEnvelope {
        ts_ns,
        sealed_v: SEALED_VERSION,
        blob: hex_encode(&blob),
    };
    Ok(serde_json::to_vec(&envelope)?)
}

/// Returns the row JSON for a stored value: unwraps a sealed envelope, passes
/// a legacy plaintext row through unchanged. A sealed row that cannot be
/// unwrapped (foreign account, tampered) is an error, never garbage.
pub(crate) fn open(stored: &[u8]) -> Result<Cow<'_, [u8]>> {
    if stored.first() != Some(&b'{') {
        bail!("OCR cache row is not JSON");
    }
    let value: serde_json::Value =
        serde_json::from_slice(stored).context("OCR cache row is not valid JSON")?;
    if value.get("sealed_v").is_none() {
        return Ok(Cow::Borrowed(stored));
    }
    let envelope: SealedEnvelope =
        serde_json::from_value(value).context("malformed sealed OCR cache envelope")?;
    if envelope.sealed_v != SEALED_VERSION {
        bail!("unsupported sealed OCR cache version {}", envelope.sealed_v);
    }
    let blob = hex_decode(&envelope.blob).context("sealed OCR cache blob is not hex")?;
    crate::secret_crypto::unprotect_with_entropy(&blob, ENTROPY).map(Cow::Owned)
}

/// Whether a stored value is a sealed envelope (as opposed to a legacy
/// plaintext row or garbage).
fn is_sealed(stored: &[u8]) -> bool {
    stored.first() == Some(&b'{')
        && serde_json::from_slice::<serde_json::Value>(stored)
            .is_ok_and(|value| value.get("sealed_v").is_some())
}

/// Rows read per page when purging legacy rows.
const PURGE_PAGE_ROWS: usize = 500;

/// One-time cleanup at startup: deletes every `CF_OCR_CACHE` row that is not a
/// sealed envelope. Legacy plaintext rows carry the full recognized screen
/// text, have no `ts_ns` (so TTL compaction never evicted them) and the cache
/// is regenerable, so they are removed rather than migrated. Returns the
/// number of rows deleted.
pub(crate) fn purge_legacy_plaintext_rows(db: &synapse_storage::Db) -> Result<usize> {
    use synapse_storage::cf;
    let mut start: Vec<u8> = Vec::new();
    let mut deleted = 0_usize;
    loop {
        let (rows, more) = db.scan_cf_from(cf::CF_OCR_CACHE, &start, PURGE_PAGE_ROWS)?;
        let Some((last, _)) = rows.last() else {
            break;
        };
        start = last.clone();
        start.push(0);
        let legacy: Vec<Vec<u8>> = rows
            .into_iter()
            .filter(|(_, value)| !is_sealed(value))
            .map(|(key, _)| key)
            .collect();
        if !legacy.is_empty() {
            deleted = deleted.saturating_add(legacy.len());
            db.delete_batch(cf::CF_OCR_CACHE, legacy)?;
        }
        if !more {
            break;
        }
    }
    if deleted > 0 {
        db.compact_cf(cf::CF_OCR_CACHE)?;
    }
    Ok(deleted)
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn hex_decode(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(text.get(index..index + 2)?, 16).ok())
        .collect()
}

#[cfg(test)]
#[path = "tests/ocr_cache_seal.rs"]
mod ocr_cache_seal_tests;

#[cfg(test)]
#[path = "tests/ocr_cache_purge_compact_52.rs"]
mod ocr_cache_purge_compact_52_tests;
