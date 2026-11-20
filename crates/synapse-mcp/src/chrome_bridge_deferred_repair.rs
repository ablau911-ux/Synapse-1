//! #132: setup can finish healthy while deferring a Chrome bridge repair to the
//! daemon (no eligible Chrome window, or the extension code reload is left to
//! `browser_debugger.reload_bridge`). The installer records the reason in a
//! one-line marker file under `%APPDATA%\synapse`; daemon bridge health reads
//! it back so the pending repair is named instead of invisible. A successful
//! `reload_bridge` clears it; the installer also clears it on a non-deferred run.

use std::path::{Path, PathBuf};

const MARKER_FILE: &str = "chrome-bridge-deferred-repair.txt";
const MAX_REASON_LEN: usize = 128;

fn marker_path() -> Option<PathBuf> {
    marker_path_in(std::env::var_os("APPDATA"))
}

/// Marker location under an injected `APPDATA` value (test seam).
pub(crate) fn marker_path_in(appdata: Option<std::ffi::OsString>) -> Option<PathBuf> {
    Some(PathBuf::from(appdata?).join("synapse").join(MARKER_FILE))
}

/// First line of the marker, accepted only when it is a short `[A-Za-z0-9_]`
/// reason token (the value is echoed into health text).
fn read_reason(path: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    let reason = raw.lines().next()?.trim();
    let valid = !reason.is_empty()
        && reason.len() <= MAX_REASON_LEN
        && reason
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
    valid.then(|| reason.to_owned())
}

/// `deferred_repair=<reason>` when setup left a bridge repair pending.
pub(crate) fn health_note() -> Option<String> {
    health_note_at(&marker_path()?)
}

/// [`health_note`] for an injected marker path (test seam).
pub(crate) fn health_note_at(marker: &Path) -> Option<String> {
    let reason = read_reason(marker)?;
    Some(format!("deferred_repair={reason}"))
}

/// Appends the pending-repair note (if any) to a bridge health detail.
pub(crate) fn append_to_detail(detail: Option<String>) -> Option<String> {
    append_note(detail, health_note())
}

fn append_note(detail: Option<String>, note: Option<String>) -> Option<String> {
    match (detail, note) {
        (Some(detail), Some(note)) => Some(format!("{detail} {note}")),
        (None, Some(note)) => Some(note),
        (detail, None) => detail,
    }
}

/// Best effort: a stale marker is harmless noise, a failed delete is not worth
/// failing the reload that already succeeded.
pub(crate) fn clear() {
    if let Some(path) = marker_path() {
        clear_at(&path);
    }
}

/// [`clear`] for an injected marker path (test seam).
pub(crate) fn clear_at(marker: &Path) {
    let _removed = std::fs::remove_file(marker);
}

#[cfg(test)]
#[path = "tests/chrome_bridge_deferred_repair_132.rs"]
mod chrome_bridge_deferred_repair_132_tests;
