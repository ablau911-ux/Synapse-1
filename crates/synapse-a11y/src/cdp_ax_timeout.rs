//! Time bound for `Accessibility.queryAXTree` (#153 part 2). Chromium never
//! answers the call for a tab that loaded visible and whose window was then
//! minimized/occluded, so the role locator must fail fast instead of hanging.

use std::{future::Future, time::Duration};

use crate::{A11yError, A11yResult};

/// Env override (milliseconds) for the queryAXTree time bound.
pub const AX_QUERY_TIMEOUT_ENV: &str = "SYNAPSE_CDP_AX_QUERY_TIMEOUT_MS";
const DEFAULT_AX_QUERY_TIMEOUT_MS: u64 = 5_000;

/// Configured queryAXTree timeout (env, else default).
#[must_use]
pub fn ax_query_timeout() -> Duration {
    let ms = std::env::var(AX_QUERY_TIMEOUT_ENV)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|ms| *ms > 0)
        .unwrap_or(DEFAULT_AX_QUERY_TIMEOUT_MS);
    Duration::from_millis(ms)
}

/// Runs `query` under `timeout`; maps timeout and inner errors to
/// `CdpAxtreeFailed` with an actionable occlusion hint.
///
/// # Errors
/// `A11yError::CdpAxtreeFailed` on timeout or inner error.
pub async fn bound_ax_query<T, E, F>(timeout: Duration, what: &str, query: F) -> A11yResult<T>
where
    F: Future<Output = Result<T, E>>,
    E: std::fmt::Display,
{
    match tokio::time::timeout(timeout, query).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(err)) => Err(A11yError::CdpAxtreeFailed {
            detail: format!("Accessibility.queryAXTree({what}): {err}"),
        }),
        Err(_) => Err(A11yError::CdpAxtreeFailed {
            detail: format!(
                "Accessibility.queryAXTree({what}) timed out after {} ms; the tab's window is likely minimized or occluded (Chromium stops answering AX queries). un-minimize the window, or use window capture / a non-role locator ({AX_QUERY_TIMEOUT_ENV} tunes the bound)",
                timeout.as_millis()
            ),
        }),
    }
}
