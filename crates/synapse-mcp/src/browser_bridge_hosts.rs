//! Per-browser-kind active bridge host table (multi-browser bridge).
//!
//! Replaces the single `BridgeInner.active_host_id`: each browser kind has at
//! most one active direct-HTTP host, so registering an Edge host never evicts
//! the Chrome host. A reconnect of the SAME kind still replaces its old host
//! (and fails that host's queued/pending commands), exactly as before.
//!
//! Child module of `chrome_debugger_bridge` (compiled into the daemon and the
//! native-host binary), so it reaches the bridge's private queue types.

use std::collections::BTreeMap;

use synapse_browsers::BrowserKindId;
use synapse_core::error_codes;

use super::{BridgeInner, ChromeResponse, ChromeResponseError};

/// `kind -> active host id`.
#[derive(Debug, Default)]
pub(super) struct BridgeHostTable {
    active_by_kind: BTreeMap<BrowserKindId, String>,
}

impl BridgeHostTable {
    /// Active host id for `kind`.
    pub(super) fn active(&self, kind: &BrowserKindId) -> Option<&str> {
        self.active_by_kind.get(kind).map(String::as_str)
    }

    /// Makes `host_id` the active host of `kind`; returns the previous active
    /// host of that kind when it was a different host.
    pub(super) fn set_active(&mut self, kind: BrowserKindId, host_id: &str) -> Option<String> {
        self.active_by_kind
            .insert(kind, host_id.to_owned())
            .filter(|previous| previous != host_id)
    }

    /// Clears `host_id` wherever it is active; returns its kind.
    pub(super) fn clear_host(&mut self, host_id: &str) -> Option<BrowserKindId> {
        let kind = self
            .active_by_kind
            .iter()
            .find(|(_, active)| active.as_str() == host_id)
            .map(|(kind, _)| kind.clone())?;
        self.active_by_kind.remove(&kind);
        Some(kind)
    }

    /// `(kind, host id)` pairs in kind order.
    pub(super) fn iter(&self) -> impl Iterator<Item = (&BrowserKindId, &str)> {
        self.active_by_kind
            .iter()
            .map(|(kind, host_id)| (kind, host_id.as_str()))
    }
}

/// Removes `host_id`'s queued commands and fails its pending commands with
/// `A11Y_CDP_EXTENSION_UNAVAILABLE` + `detail`. Returns
/// `(queued_removed, pending_failed)`. Shared by host replacement, WS
/// disconnect and native-port disconnect (one implementation, DRY).
pub(super) fn fail_host_commands(
    inner: &mut BridgeInner,
    host_id: &str,
    detail: &str,
) -> (usize, usize) {
    let queued_before = inner.commands.len();
    inner.commands.retain(|queued| queued.host_id != host_id);
    let queued_removed = queued_before.saturating_sub(inner.commands.len());
    let pending_ids = inner
        .pending
        .iter()
        .filter(|(_, pending)| pending.host_id == host_id)
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    for id in &pending_ids {
        if let Some(pending) = inner.pending.remove(id) {
            let _ = pending.sender.send(ChromeResponse {
                id: id.clone(),
                ok: false,
                result: None,
                error: Some(ChromeResponseError {
                    code: Some(error_codes::A11Y_CDP_EXTENSION_UNAVAILABLE.to_owned()),
                    detail: Some(detail.to_owned()),
                    // Host vanished with the request pending: delivery is unknown,
                    // so no stage is claimed (#189: no typed flag = possibly delivered).
                    dispatch_stage: None,
                }),
            });
        }
    }
    (queued_removed, pending_ids.len())
}

/// A new direct-HTTP host of `kind` replaces only the previous direct-HTTP
/// host of the SAME kind; hosts of other kinds keep their queues and pending
/// commands (`must_pass_two_kinds_hosts_coexist`).
pub(super) fn replace_active_direct_http_host(
    inner: &mut BridgeInner,
    kind: &BrowserKindId,
    new_host_id: &str,
) {
    let Some(old_host_id) = inner.active_hosts.active(kind).map(str::to_owned) else {
        return;
    };
    if old_host_id == new_host_id {
        return;
    }
    let Some(old_host) = inner.hosts.get(&old_host_id) else {
        return;
    };
    if old_host.transport.as_deref() != Some("direct_http") {
        return;
    }
    let detail = format!(
        "Chrome debugger direct HTTP bridge host {old_host_id} was replaced by {new_host_id} before command response"
    );
    let (queued_removed, pending_failed) = fail_host_commands(inner, &old_host_id, &detail);
    tracing::warn!(
        code = "CHROME_DEBUGGER_DIRECT_HTTP_HOST_REPLACED",
        browser_kind = %kind,
        old_host_id = %old_host_id,
        new_host_id = %new_host_id,
        queued_removed,
        pending_failed,
        "Chrome debugger direct HTTP bridge host replaced"
    );
}
