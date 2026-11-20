//! #183: per-command daemon response budget for the Chrome bridge.
//!
//! The budget must outlive every wait the extension may perform for a command
//! (caller wait plus chained auto-wait) by a fixed margin, so the extension's
//! own clean timeout result always reaches the daemon before the daemon gives
//! up. The daemon also stamps an absolute deadline on the command so the
//! extension never starts (or keeps waiting on) work after the daemon has
//! already reported a timeout.

use std::time::Duration;

use serde_json::Value;

/// Default daemon budget for commands that carry no explicit wait.
pub(super) const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
/// Fixed slack on top of the longest in-extension wait chain (same margin the
/// downloads budget has used since #1342).
pub(super) const COMMAND_BUDGET_MARGIN: Duration = Duration::from_secs(5);
/// Upper bound for any single caller wait field; matches the downloads cap.
const MAX_WAIT_FIELD_MS: u64 = 300_000;
/// Auto-wait window the extension applies when the caller does not name one.
const DEFAULT_AUTO_WAIT_MS: u64 = 2_000;
/// Params fields that carry an in-extension wait the daemon must outlive.
const WAIT_FIELDS: [&str; 4] = [
    "waitTimeoutMs",
    "timeoutMs",
    "wait_timeout_ms",
    "timeout_ms",
];
/// Kinds whose auto-wait runs before the action and defaults on or is opt-in.
const AUTO_WAIT_KINDS: [&str; 2] = ["domAction", "cdpInput"];

fn field_ms(params: &Value, key: &str) -> Option<u64> {
    params
        .get(key)
        .and_then(Value::as_u64)
        .map(|value| value.min(MAX_WAIT_FIELD_MS))
}

/// Daemon response budget for `kind`: the longest in-extension wait chain plus
/// [`COMMAND_BUDGET_MARGIN`], never below [`DEFAULT_COMMAND_TIMEOUT`].
pub(super) fn command_budget(kind: &str, params: &Value) -> Duration {
    let wait_ms = WAIT_FIELDS
        .iter()
        .filter_map(|key| field_ms(params, key))
        .max()
        .unwrap_or(0);
    let auto_wait_ms = field_ms(params, "autoWaitTimeoutMs").unwrap_or_else(|| {
        if AUTO_WAIT_KINDS.contains(&kind) {
            DEFAULT_AUTO_WAIT_MS
        } else {
            0
        }
    });
    let chain = Duration::from_millis(wait_ms.saturating_add(auto_wait_ms));
    chain
        .saturating_add(COMMAND_BUDGET_MARGIN)
        .max(DEFAULT_COMMAND_TIMEOUT)
}
