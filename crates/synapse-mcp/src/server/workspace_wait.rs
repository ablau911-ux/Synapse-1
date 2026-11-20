//! Roadmap P6 (`workspace wait`, Gitea #2329): waiting for a blackboard key.
//! `WorkspaceWait` validates its budget when built, owns the polling loop and
//! keeps the pure step rule (`next_sleep_ms`) testable without storage or clock.

#[cfg(test)]
#[path = "workspace_wait_table.rs"]
mod workspace_wait_table;

use rmcp::model::ErrorCode;
use serde_json::{Value, json};

use super::ErrorData;
use super::workspace_blackboard::{
    WORKSPACE_KEY_ABSENT, WORKSPACE_SOURCE_OF_TRUTH, WORKSPACE_WAIT_TIMEOUT, WorkspaceGetParams,
    WorkspaceGetResponse, WorkspaceWaitResponse,
};

pub(super) const WORKSPACE_WAIT_DEFAULT_TIMEOUT_MS: u64 = 5_000;
const WORKSPACE_WAIT_MAX_TIMEOUT_MS: u64 = 60_000;
pub(super) const WORKSPACE_WAIT_DEFAULT_POLL_INTERVAL_MS: u64 = 50;
const WORKSPACE_WAIT_MAX_POLL_INTERVAL_MS: u64 = 500;

/// One wait for a blackboard key, with a valid budget.
#[derive(Debug)]
pub(super) struct WorkspaceWait {
    run_id: Option<String>,
    key: String,
    timeout_ms: u64,
    poll_interval_ms: u64,
}

impl WorkspaceWait {
    /// Accepts `timeout_ms` in 1..=60 000 and `poll_interval_ms` in 1..=500. The
    /// refusal text names the refused field and its allowed range.
    pub(super) fn new(
        run_id: Option<String>,
        key: String,
        timeout_ms: u64,
        poll_interval_ms: u64,
    ) -> Result<Self, String> {
        if !(1..=WORKSPACE_WAIT_MAX_TIMEOUT_MS).contains(&timeout_ms) {
            return Err(format!(
                "timeout_ms must be between 1 and {WORKSPACE_WAIT_MAX_TIMEOUT_MS}"
            ));
        }
        if !(1..=WORKSPACE_WAIT_MAX_POLL_INTERVAL_MS).contains(&poll_interval_ms) {
            return Err(format!(
                "poll_interval_ms must be between 1 and {WORKSPACE_WAIT_MAX_POLL_INTERVAL_MS}"
            ));
        }
        Ok(Self {
            run_id,
            key,
            timeout_ms,
            poll_interval_ms,
        })
    }

    /// How long the next sleep is: `None` when the time limit has passed,
    /// else the poll interval cut to the time left, never below 1 ms.
    pub(super) fn next_sleep_ms(&self, waited_ms: u64) -> Option<u64> {
        if waited_ms >= self.timeout_ms {
            None
        } else {
            Some(
                self.poll_interval_ms
                    .min(self.timeout_ms - waited_ms)
                    .max(1),
            )
        }
    }

    /// Reads the key with `read_key` until it is there (the entry with
    /// `waited_ms` / `poll_count`), a read fails for another reason than an
    /// absent key (that error), or the time limit passes
    /// (`WORKSPACE_WAIT_TIMEOUT`).
    pub(super) async fn run(
        self,
        mut read_key: impl FnMut(WorkspaceGetParams) -> Result<WorkspaceGetResponse, ErrorData>,
    ) -> Result<WorkspaceWaitResponse, ErrorData> {
        let started = std::time::Instant::now();
        let mut poll_count: u64 = 0;
        loop {
            poll_count += 1;
            match read_key(WorkspaceGetParams {
                run_id: self.run_id.clone(),
                key: self.key.clone(),
            }) {
                Ok(found) => {
                    return Ok(WorkspaceWaitResponse {
                        ok: true,
                        waited_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                        poll_count,
                        found,
                    });
                }
                Err(error)
                    if error
                        .data
                        .as_ref()
                        .and_then(|data| data.get("code"))
                        .and_then(Value::as_str)
                        == Some(WORKSPACE_KEY_ABSENT) => {}
                Err(error) => return Err(error),
            }
            let waited_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            match self.next_sleep_ms(waited_ms) {
                Some(ms) => {
                    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                }
                None => return Err(self.timeout_error(waited_ms, poll_count)),
            }
        }
    }

    fn timeout_error(&self, waited_ms: u64, poll_count: u64) -> ErrorData {
        ErrorData::new(
            ErrorCode(-32099),
            format!(
                "workspace blackboard key {:?} did not appear within {} ms",
                self.key, self.timeout_ms
            ),
            Some(json!({
                "code": WORKSPACE_WAIT_TIMEOUT,
                "key": self.key,
                "timeout_ms": self.timeout_ms,
                "waited_ms": waited_ms,
                "poll_count": poll_count,
                "source_of_truth": WORKSPACE_SOURCE_OF_TRUTH,
                "remediation": "the writer has not put the key yet; wait again or check the writer",
            })),
        )
    }
}
