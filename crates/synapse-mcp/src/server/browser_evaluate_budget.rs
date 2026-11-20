//! Roadmap P4 (page evaluate time limit, Gitea #2331): how long one
//! `browser evaluate` call may run. `EvaluateLimit` is the value object: it
//! refuses an out-of-range request and is the one place that applies the
//! timer and makes the `BROWSER_EVALUATE_TIMEOUT` error.

#[cfg(test)]
#[path = "browser_evaluate_budget_table.rs"]
mod browser_evaluate_budget_table;

#[cfg(windows)]
use super::ErrorData;
#[cfg(windows)]
use crate::m1::mcp_error;
#[cfg(windows)]
use synapse_core::error_codes;

/// Time limit of an evaluate call that names none (what the bridge always used).
const EVALUATE_DEFAULT_TIMEOUT_MS: u64 = 5_000;
/// Shortest time limit a caller may ask for.
const EVALUATE_MIN_TIMEOUT_MS: u64 = 50;
/// Longest time limit a caller may ask for.
const EVALUATE_MAX_TIMEOUT_MS: u64 = 120_000;

/// The time limit in force for one `browser evaluate` call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EvaluateLimit {
    ms: u64,
}

impl Default for EvaluateLimit {
    /// The limit of a call that names none.
    fn default() -> Self {
        Self {
            ms: EVALUATE_DEFAULT_TIMEOUT_MS,
        }
    }
}

impl EvaluateLimit {
    /// The default when the caller names none, the caller's value when it is
    /// inside the allowed range, else a refusal whose text names `timeout_ms`
    /// and the range.
    pub(crate) fn from_request(timeout_ms: Option<u64>) -> Result<Self, String> {
        match timeout_ms {
            None => Ok(Self::default()),
            Some(ms) if (EVALUATE_MIN_TIMEOUT_MS..=EVALUATE_MAX_TIMEOUT_MS).contains(&ms) => {
                Ok(Self { ms })
            }
            Some(_) => Err(format!(
                "timeout_ms must be between {EVALUATE_MIN_TIMEOUT_MS} and {EVALUATE_MAX_TIMEOUT_MS}"
            )),
        }
    }

    pub(crate) const fn as_millis(self) -> u64 {
        self.ms
    }

    #[cfg(windows)]
    pub(crate) const fn as_duration(self) -> std::time::Duration {
        std::time::Duration::from_millis(self.ms)
    }

    /// Runs one raw CDP evaluate call within the limit; when the limit passes
    /// first the call is dropped and `BROWSER_EVALUATE_TIMEOUT` is returned
    /// (the page script itself may keep running).
    #[cfg(windows)]
    pub(crate) async fn run<T>(
        self,
        call: impl std::future::Future<Output = T>,
    ) -> Result<T, ErrorData> {
        tokio::time::timeout(self.as_duration(), call)
            .await
            .map_err(|_elapsed| {
                mcp_error(
                    error_codes::BROWSER_EVALUATE_TIMEOUT,
                    format!(
                        "browser_evaluate did not finish within {} ms; the page script may still be running",
                        self.ms
                    ),
                )
            })
    }
}
