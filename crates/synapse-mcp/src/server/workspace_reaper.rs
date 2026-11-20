//! Daemon-wide TTL reaper for workspace blackboard rows (#56).
//!
//! Request-path cleanup only examines the run namespace being written, so rows
//! in any other run namespace (including the default namespace of an ended
//! daemon lifecycle) were never removed. The reaper pages through every
//! workspace row in CF_KV, deleting rows whose TTL has elapsed, once at daemon
//! startup and then on a fixed period.

use std::sync::Arc;
use std::time::Duration;

use synapse_storage::{Db, cf};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::{
    ErrorData, SynapseService, WORKSPACE_LIST_RAW_BUDGET, WORKSPACE_LIST_VALUE_BUDGET_BYTES,
    WORKSPACE_PREFIX, WorkspaceRawRow, decode_workspace_row,
    delete_observed_expired_workspace_rows_locked, mcp_error, unix_time_ms_now,
    workspace_write_lock,
};

/// Default sweep period; override with [`REAP_PERIOD_ENV`] (milliseconds).
const DEFAULT_REAP_PERIOD_MS: u64 = 10 * 60 * 1000;
const REAP_PERIOD_ENV: &str = "SYNAPSE_WORKSPACE_REAP_INTERVAL_MS";

const fn default_period() -> Duration {
    Duration::from_millis(DEFAULT_REAP_PERIOD_MS)
}

/// Outcome of one full pass over every workspace run namespace.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct WorkspaceReapReport {
    pub scanned_rows: usize,
    pub expired_rows_deleted: usize,
    pub corrupt_rows_skipped: usize,
    pub pages: usize,
}

/// Sweeps expired workspace rows across all run namespaces.
pub(crate) struct WorkspaceReaper;

impl WorkspaceReaper {
    /// One bounded-page pass over every workspace row.
    pub(crate) fn sweep(db: &Db, now_unix_ms: u64) -> Result<WorkspaceReapReport, ErrorData> {
        Self::sweep_paged(db, now_unix_ms, WORKSPACE_LIST_RAW_BUDGET)
    }

    fn sweep_paged(
        db: &Db,
        now_unix_ms: u64,
        rows_per_page: usize,
    ) -> Result<WorkspaceReapReport, ErrorData> {
        let prefix = format!("{WORKSPACE_PREFIX}/");
        let mut start_key = prefix.as_bytes().to_vec();
        let mut report = WorkspaceReapReport::default();
        loop {
            let (rows, more) = db
                .scan_cf_prefix_window(
                    cf::CF_KV,
                    prefix.as_bytes(),
                    &start_key,
                    rows_per_page,
                    WORKSPACE_LIST_VALUE_BUDGET_BYTES,
                )
                .map_err(|error| mcp_error(error.code(), error.to_string()))?;
            report.pages = report.pages.saturating_add(1);
            report.scanned_rows = report.scanned_rows.saturating_add(rows.len());
            let next_start = rows.last().map(|(key, _)| {
                let mut next = key.clone();
                next.push(0);
                next
            });
            let expired = Self::expired_rows(rows, now_unix_ms, &mut report);
            if !expired.is_empty() {
                let guard = workspace_write_lock()?;
                let deleted = delete_observed_expired_workspace_rows_locked(db, expired, &guard)?;
                report.expired_rows_deleted = report.expired_rows_deleted.saturating_add(deleted);
            }
            match (more, next_start) {
                (true, Some(next)) => start_key = next,
                _ => return Ok(report),
            }
        }
    }

    /// Expired rows of one page; corrupt rows are counted and left in place
    /// for the request path to report.
    fn expired_rows(
        rows: Vec<(Vec<u8>, Vec<u8>)>,
        now_unix_ms: u64,
        report: &mut WorkspaceReapReport,
    ) -> Vec<WorkspaceRawRow> {
        let mut expired = Vec::new();
        for (key, encoded) in rows {
            match decode_workspace_row(key, encoded) {
                Ok(row) if row.entry.expires_at_unix_ms <= now_unix_ms => {
                    expired.push(WorkspaceRawRow {
                        key: row.key,
                        encoded: row.encoded,
                    });
                }
                Ok(_) => {}
                Err(_) => {
                    report.corrupt_rows_skipped = report.corrupt_rows_skipped.saturating_add(1);
                }
            }
        }
        expired
    }

    /// The sweep period from [`REAP_PERIOD_ENV`]. An unset, non-numeric, zero
    /// or non-unicode value logs a warning and falls back to the default
    /// instead of aborting daemon startup.
    pub(crate) fn period_from_env() -> Duration {
        Self::period_from_raw(std::env::var(REAP_PERIOD_ENV))
    }

    /// The sweep period decision for a raw [`REAP_PERIOD_ENV`] lookup result
    /// (test seam: the env read is injected as its `std::env::var` result).
    pub(crate) fn period_from_raw(raw: Result<String, std::env::VarError>) -> Duration {
        let raw = match raw {
            Ok(raw) => raw,
            Err(std::env::VarError::NotPresent) => return default_period(),
            Err(error) => {
                tracing::warn!(
                    code = "WORKSPACE_REAP_PERIOD_INVALID",
                    env = REAP_PERIOD_ENV,
                    error = %error,
                    default_ms = DEFAULT_REAP_PERIOD_MS,
                    "workspace reap interval is not valid unicode; using the default"
                );
                return default_period();
            }
        };
        match Self::parse_period(&raw) {
            Some(period) => period,
            None => {
                tracing::warn!(
                    code = "WORKSPACE_REAP_PERIOD_INVALID",
                    env = REAP_PERIOD_ENV,
                    value = %raw,
                    default_ms = DEFAULT_REAP_PERIOD_MS,
                    "workspace reap interval must be a positive integer (milliseconds); using the default"
                );
                default_period()
            }
        }
    }

    /// Parses a positive millisecond count; `None` for anything else.
    pub(crate) fn parse_period(raw: &str) -> Option<Duration> {
        raw.trim()
            .parse::<u64>()
            .ok()
            .filter(|value| *value > 0)
            .map(Duration::from_millis)
    }

    /// Runs one sweep immediately and then every `period` until shutdown.
    pub(crate) fn spawn(
        db: Arc<Db>,
        period: Duration,
        shutdown: CancellationToken,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            tracing::info!(
                code = "WORKSPACE_REAPER_STARTED",
                period_ms = u64::try_from(period.as_millis()).unwrap_or(u64::MAX),
                "workspace reaper started"
            );
            let mut interval = tokio::time::interval(period);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    () = shutdown.cancelled() => break,
                    _ = interval.tick() => Self::sweep_and_log(&db).await,
                }
            }
        })
    }

    async fn sweep_and_log(db: &Arc<Db>) {
        let db = Arc::clone(db);
        let outcome =
            tokio::task::spawn_blocking(move || Self::sweep(&db, unix_time_ms_now())).await;
        match outcome {
            Ok(Ok(report)) => tracing::info!(
                code = "WORKSPACE_REAPER_SWEPT",
                scanned_rows = report.scanned_rows,
                expired_rows_deleted = report.expired_rows_deleted,
                corrupt_rows_skipped = report.corrupt_rows_skipped,
                pages = report.pages,
                "readback=workspace_blackboard edge=reaper_sweep"
            ),
            Ok(Err(error)) => tracing::warn!(
                code = "WORKSPACE_REAPER_FAILED",
                error = %error.message,
                "workspace reaper sweep failed"
            ),
            Err(error) => tracing::warn!(
                code = "WORKSPACE_REAPER_FAILED",
                error = %error,
                "workspace reaper task failed"
            ),
        }
    }
}

impl SynapseService {
    /// One-time startup purge of legacy plaintext `CF_OCR_CACHE` rows (#52).
    /// Failure is logged, never fatal: the cache is regenerable.
    pub(crate) fn purge_legacy_ocr_cache_rows(&self) {
        match self
            .workspace_db()
            .map_err(|error| anyhow::anyhow!("{}", error.message))
            .and_then(|db| crate::ocr_cache_seal::purge_legacy_plaintext_rows(&db))
        {
            Ok(0) => {}
            Ok(deleted) => tracing::warn!(
                code = "OCR_CACHE_LEGACY_ROWS_PURGED",
                deleted,
                "deleted legacy plaintext OCR cache rows (cache is regenerable)"
            ),
            Err(error) => tracing::warn!(
                code = "OCR_CACHE_LEGACY_PURGE_FAILED",
                error = %error,
                "could not purge legacy plaintext OCR cache rows"
            ),
        }
    }

    /// Starts the workspace reaper on this service's storage. Shared by the
    /// HTTP and stdio daemon startup paths so every daemon mode reaps.
    pub(crate) fn spawn_workspace_reaper(
        &self,
        shutdown: CancellationToken,
    ) -> Result<JoinHandle<()>, String> {
        let period = WorkspaceReaper::period_from_env();
        let db = self
            .workspace_db()
            .map_err(|error| error.message.to_string())?;
        Ok(WorkspaceReaper::spawn(db, period, shutdown))
    }
}

#[cfg(test)]
#[path = "tests/workspace_reaper.rs"]
mod workspace_reaper_tests;

#[cfg(test)]
#[path = "tests/workspace_reaper_period_56.rs"]
mod workspace_reaper_period_tests;
