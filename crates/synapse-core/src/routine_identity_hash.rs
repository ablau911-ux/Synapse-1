//! Signature normalization and deterministic routine identity hashing.
use sha2::{Digest, Sha256};

use crate::routines::granularity_token;
use crate::types::{RoutineGranularity, RoutineRecord, RoutineStep};

use super::IdClaims;

fn collapse_lower(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

pub(super) fn normalize_app(app: &str) -> String {
    let mut normalized = collapse_lower(app);
    let stem_len = normalized.strip_suffix(".exe").map_or(0, str::len);
    if stem_len > 0 {
        normalized.truncate(stem_len);
    }
    normalized
}

pub(super) fn normalize_document(document: Option<&str>) -> String {
    collapse_lower(document.unwrap_or_default()).replace('\\', "/")
}

/// Normalized, order-significant lookup signature. Signature equality alone
/// never establishes ownership: distinct raw spellings can share it.
#[must_use]
pub fn routine_signature(granularity: RoutineGranularity, steps: &[RoutineStep]) -> String {
    let mut signature = String::from(granularity_token(granularity));
    for step in steps {
        signature.push('\u{1F}');
        signature.push_str(&normalize_app(&step.app));
        signature.push('\u{1E}');
        signature.push_str(&normalize_document(step.document.as_deref()));
    }
    signature
}

/// Deterministic stable id: `rt1-` + first 16 hex chars of SHA-256 over the
/// normalized step signature and an independent cluster discriminator.
///
/// Excludes the mining timestamp, day-of-week class, and all derived
/// statistics, so re-mining the same routine reproduces the same id — the
/// property lifecycle state (#849), intent feedback (#856), and armed
/// automations anchor on.
#[must_use]
pub fn routine_id(
    granularity: RoutineGranularity,
    steps: &[RoutineStep],
    cluster_ordinal: u32,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(routine_signature(granularity, steps).as_bytes());
    hasher.update([0x1F]);
    hasher.update(cluster_ordinal.to_be_bytes());
    rt1_id(&hasher.finalize())
}

/// `rt1-` + the first 16 lowercase hex chars of `digest`.
fn rt1_id(digest: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut hex = String::with_capacity(16);
    for byte in digest.iter().take(8) {
        hex.push(char::from(HEX[usize::from(byte >> 4)]));
        hex.push(char::from(HEX[usize::from(byte & 0x0F)]));
    }
    format!("rt1-{hex}")
}

/// Id for a freshly mined routine whose anchor-derived id already belongs to
/// a different identity (a sibling spelling took its tie slot, or a vanished
/// routine's rows still live under it). The id hashes the displaced id plus
/// the raw steps, so it is deterministic, keeps the `rt1-` key format, and
/// never names another routine's dependent rows. Later passes keep it
/// through raw-step reconciliation, or regain it through its recorded owner
/// after the routine vanished and returned.
pub(super) fn displaced_routine_id(routine: &RoutineRecord, claims: &IdClaims) -> String {
    let mut attempt: u32 = 0;
    loop {
        let mut hasher = Sha256::new();
        hasher.update(routine.routine_id.as_bytes());
        for step in &routine.steps {
            hasher.update([0x1F]);
            hasher.update(step.app.as_bytes());
            hasher.update([0x1E, u8::from(step.document.is_some())]);
            hasher.update(step.document.as_deref().unwrap_or_default().as_bytes());
        }
        hasher.update([0x1D]);
        hasher.update(attempt.to_be_bytes());
        let id = rt1_id(&hasher.finalize());
        if claims.may_take_derived(&id) {
            return id;
        }
        attempt = attempt.wrapping_add(1);
    }
}
