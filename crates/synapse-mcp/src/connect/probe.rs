//! Daemon `/health` probing and the spawn decision for the connect bridge (#171).
//!
//! The bridge used to read "anything but a fast 2xx" as "daemon down" and spawn
//! one. A revoked scoped credential (401 after a daemon restart) or a `/health`
//! stalled under build load then produced a spawn loop. The probe now reports
//! what actually happened, and only a refused connection with no live
//! single-instance lock holder may lead to a spawn.

use std::time::Duration;

/// Per-request `/health` timeout. Must stay above the daemon's own 3 s `/health`
/// bound (#146 measured 1.8 s under build load), so a slow answer is never
/// mistaken for an absent daemon.
pub(super) const HEALTH_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// What one `/health` request showed about the daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DaemonProbe {
    /// 2xx: daemon is up and accepted the credential.
    Healthy,
    /// 401/403: a daemon answered and refused the credential.
    AuthRejected(u16),
    /// Any other HTTP status (for example 503 while draining): a daemon answered.
    Unhealthy(u16),
    /// The request timed out: something may be listening but is slow.
    TimedOut,
    /// Connection refused: nothing is listening on the port.
    Unreachable,
    /// Any other transport failure: not proof that the daemon is absent.
    Failed,
}

/// What the bridge does next given a [`DaemonProbe`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ProbeAction {
    Proceed,
    AuthRejected(u16),
    WaitForDaemon,
    Spawn,
}

/// Spawn only when the port refuses connections AND no live daemon holds the
/// single-instance lock (it may be starting and not bound yet). `daemon_lock_held`
/// is consulted lazily, only in the refused case.
pub(super) fn plan_for_probe(
    probe: &DaemonProbe,
    daemon_lock_held: impl FnOnce() -> bool,
) -> ProbeAction {
    match probe {
        DaemonProbe::Healthy => ProbeAction::Proceed,
        DaemonProbe::AuthRejected(status) => ProbeAction::AuthRejected(*status),
        DaemonProbe::Unhealthy(_) | DaemonProbe::TimedOut | DaemonProbe::Failed => {
            ProbeAction::WaitForDaemon
        }
        DaemonProbe::Unreachable if daemon_lock_held() => ProbeAction::WaitForDaemon,
        DaemonProbe::Unreachable => ProbeAction::Spawn,
    }
}

/// The daemon answered but refuses the bridge's credential. Terminal: a scoped
/// spawn credential lives in daemon memory only, so it cannot recover after a
/// restart, and spawning another daemon cannot fix it.
#[derive(Debug)]
pub(super) struct AuthRejectedError {
    bind: String,
    status: u16,
}

#[cfg(test)]
impl AuthRejectedError {
    pub(super) fn status(&self) -> u16 {
        self.status
    }
}

impl std::fmt::Display for AuthRejectedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "MCP_CONNECT_AUTH_REJECTED: daemon at {} answered HTTP {} to the bridge credential; the credential was revoked or is wrong (a daemon restart revokes scoped agent credentials), so the bridge will not retry or spawn a daemon; restart the MCP client or agent to obtain a fresh credential",
            self.bind, self.status
        )
    }
}

impl std::error::Error for AuthRejectedError {}

/// Build (and log) the terminal auth-rejection error for `bind`.
pub(super) fn auth_rejected(bind: &str, status: u16) -> anyhow::Error {
    tracing::error!(
        code = "MCP_CONNECT_AUTH_REJECTED",
        bind = %bind,
        status,
        "daemon rejected the bridge credential; not retrying and not spawning a daemon"
    );
    anyhow::Error::new(AuthRejectedError {
        bind: bind.to_owned(),
        status,
    })
}

fn classify_status(status: reqwest::StatusCode) -> DaemonProbe {
    if status.is_success() {
        DaemonProbe::Healthy
    } else if matches!(status.as_u16(), 401 | 403) {
        DaemonProbe::AuthRejected(status.as_u16())
    } else {
        DaemonProbe::Unhealthy(status.as_u16())
    }
}

/// A token worth retrying after a 401/403: the freshly reloaded one, if it
/// differs from `current`. A scoped spawn credential reloads to itself (it lives
/// in the environment), so it is never retried; a rotated `token.txt` is.
pub(super) fn refreshed_token(
    current: &str,
    reload: impl FnOnce() -> anyhow::Result<String>,
) -> Option<String> {
    reload().ok().filter(|fresh| fresh != current)
}

/// [`probe_daemon`], but on 401/403 re-read the token once (a daemon restart may
/// have rotated `token.txt`) and probe again. Returns the probe plus the token
/// that produced it, so callers keep using the one that works.
pub(super) async fn probe_daemon_refreshing_with(
    bind: &str,
    token: &str,
    timeout: Duration,
    reload: impl FnOnce() -> anyhow::Result<String>,
) -> (DaemonProbe, String) {
    let first = probe_daemon(bind, token, timeout).await;
    if matches!(first, DaemonProbe::AuthRejected(_))
        && let Some(fresh) = refreshed_token(token, reload)
    {
        tracing::info!(
            code = "MCP_CONNECT_TOKEN_RELOADED",
            bind = %bind,
            "daemon rejected the bridge token; retrying once with the reloaded token"
        );
        let second = probe_daemon(bind, &fresh, timeout).await;
        return (second, fresh);
    }
    (first, token.to_owned())
}

/// [`probe_daemon_refreshing_with`] reloading through the normal client lookup.
pub(super) async fn probe_daemon_refreshing(
    bind: &str,
    token: &str,
    timeout: Duration,
) -> (DaemonProbe, String) {
    probe_daemon_refreshing_with(
        bind,
        token,
        timeout,
        crate::spawn_credentials::client_bearer_token,
    )
    .await
}

/// Probe `http://{bind}/health` once and report what happened.
pub(super) async fn probe_daemon(bind: &str, token: &str, timeout: Duration) -> DaemonProbe {
    let url = format!("http://{bind}/health");
    let Ok(client) = reqwest::Client::builder().timeout(timeout).build() else {
        return DaemonProbe::Failed;
    };
    match client.get(&url).bearer_auth(token).send().await {
        Ok(resp) => classify_status(resp.status()),
        Err(error) if error.is_timeout() => DaemonProbe::TimedOut,
        Err(error) if error.is_connect() => DaemonProbe::Unreachable,
        Err(_) => DaemonProbe::Failed,
    }
}
