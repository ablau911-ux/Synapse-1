//! Running-code identity for the Chrome bridge worker (#237).
//!
//! Chrome keeps executing a cached MV3 service worker until the extension is
//! reloaded, even across browser restarts, while the worker's own integrity
//! check hashes the file on disk. The installer therefore stamps the installed
//! `service_worker.js` with `BRIDGE_CODE_STAMP` (the SHA-256 fingerprint of
//! deployed JavaScript paths and content hashes, including imported modules);
//! the running worker reports the stamp it was loaded with, and a
//! mismatch with the installed file's stamp means the running code is stale.

/// The `BRIDGE_CODE_STAMP` value declared in worker source, if well formed.
/// One implementation for every browser: `synapse_browsers::parse_code_stamp`.
pub(super) fn parse_code_stamp(source: &str) -> Option<String> {
    synapse_browsers::parse_code_stamp(source)
}

/// Why the running worker is stale relative to the installed one, if it is.
/// Without an installed stamp there is nothing to compare against.
pub(super) fn code_stamp_stale_reason(
    running: Option<&str>,
    installed: Option<&str>,
) -> Option<String> {
    let installed = installed?;
    (running != Some(installed)).then(|| {
        format!(
            "code_stamp={} expected={installed}",
            running.unwrap_or("not_seen_yet")
        )
    })
}

/// Whether a stale reason is the running-code mismatch a reload can heal.
pub(super) fn is_code_stamp_reason(reason: &str) -> bool {
    reason.contains("code_stamp=")
}

/// Serializes stale-code reloads across concurrent commands and remembers the
/// installed stamp a reload was already tried for, so a mismatch a reload
/// cannot heal fails fast instead of reloading on every command.
#[derive(Default)]
pub(super) struct StaleCodeReloadGate {
    attempted_for: tokio::sync::Mutex<Option<String>>,
}

impl StaleCodeReloadGate {
    pub(super) async fn lock(&self) -> tokio::sync::MutexGuard<'_, Option<String>> {
        self.attempted_for.lock().await
    }
}

/// What a command refused for stale running code should do next.
#[derive(Debug, Eq, PartialEq)]
pub(super) enum StaleCodeHeal {
    /// The worker runs the installed code now (healed here or by a command
    /// that held the gate first): retry the command.
    Retry,
    /// A reload already proved this installed stamp unhealable: keep the
    /// original stale refusal.
    Refuse,
}

/// The stale-code heal flow behind one gate: re-check under the gate, reload
/// at most once per installed stamp, remember only an unhealable outcome, and
/// report every reload through `record`.
pub(super) async fn heal_stale_code<T, E, Reload, Fut>(
    gate: &StaleCodeReloadGate,
    still_code_stale: impl FnOnce() -> bool,
    installed: Option<String>,
    reload: Reload,
    reconnected_still_stale: impl FnOnce(&E) -> bool,
    record: impl FnOnce(&Result<T, E>),
) -> Result<StaleCodeHeal, E>
where
    Reload: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
{
    let mut attempted_for = gate.lock().await;
    if !still_code_stale() {
        return Ok(StaleCodeHeal::Retry);
    }
    if !should_reload(attempted_for.as_deref(), installed.as_deref()) {
        return Ok(StaleCodeHeal::Refuse);
    }
    let result = reload().await;
    let still_stale = match &result {
        Ok(_) => false,
        Err(error) => reconnected_still_stale(error),
    };
    *attempted_for = remembered_after_reload(still_stale, installed.as_deref());
    record(&result);
    result.map(|_| StaleCodeHeal::Retry)
}

/// A reload is worth trying once per installed stamp, and only when the
/// install is stamped at all.
pub(super) fn should_reload(attempted_for: Option<&str>, installed: Option<&str>) -> bool {
    installed.is_some() && attempted_for != installed
}

/// What the gate remembers after a reload for `installed`. Only a reload that
/// reconnected yet still runs stale code proves the mismatch unhealable; a
/// healed or failed (timed-out, busy) reload leaves the gate open, so another
/// profile's cached worker or a retry can still be healed.
pub(super) fn remembered_after_reload(
    reconnected_still_stale: bool,
    installed: Option<&str>,
) -> Option<String> {
    reconnected_still_stale
        .then(|| installed.map(ToOwned::to_owned))
        .flatten()
}
