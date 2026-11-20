//! Daemon-owned periodic storage maintenance (#60).
//!
//! `Db::spawn_gc_task` and `Db::spawn_pressure_task` existed but nothing ever
//! called them, so in production the retention GC never ran and disk pressure
//! was never detected. The daemon (HTTP serve and stdio) now starts both right
//! after the shared RocksDB handle is open and keeps the returned handles alive
//! for its whole lifetime. Both tasks run their scans on tokio's blocking pool
//! (`synapse_storage::blocking`), never on an async worker.
//!
//! This module also owns [`run_storage_gc_once_blocking`], the off-worker entry
//! point for the `storage gc_once` operation (its scan + compaction used to run
//! synchronously on the tool-call worker while holding the reflex mutex).
//!
//! Configuration (read once at startup; invalid values are a startup error, not
//! a silent default):
//!
//! - `SYNAPSE_STORAGE_GC_INTERVAL_SECS` - seconds between GC passes
//!   (default 300; `0` disables the GC task).
//! - `SYNAPSE_STORAGE_PRESSURE_INTERVAL_SECS` - seconds between disk-pressure
//!   probes (default 30; `0` disables the probe).
//!
//! When an operator pins a synthetic free-byte sample
//! (`SYNAPSE_STORAGE_PRESSURE_FREE_BYTES_SAMPLE`) the real probe is not started,
//! otherwise it would overwrite the pinned level within one interval.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use rmcp::ErrorData;
use synapse_core::error_codes;
use synapse_reflex::ReflexRuntime;
use synapse_storage::{Db, GcTask, PressureTask};

use super::SharedM3State;
use super::storage::{StorageGcOnceParams, StorageGcOnceResponse, run_storage_gc_once};
use crate::m1::mcp_error;

/// Environment variable: seconds between periodic storage GC passes.
pub const GC_INTERVAL_ENV: &str = "SYNAPSE_STORAGE_GC_INTERVAL_SECS";
/// Environment variable: seconds between disk-pressure probes.
pub const PRESSURE_INTERVAL_ENV: &str = "SYNAPSE_STORAGE_PRESSURE_INTERVAL_SECS";
/// Default GC interval: 5 minutes.
pub const DEFAULT_GC_INTERVAL_SECS: u64 = 300;
/// Default disk-pressure probe interval: 30 seconds.
pub const DEFAULT_PRESSURE_INTERVAL_SECS: u64 = 30;

/// Resolved intervals; `None` means the task is disabled.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StorageMaintenanceConfig {
    pub gc_interval: Option<Duration>,
    pub pressure_interval: Option<Duration>,
}

impl StorageMaintenanceConfig {
    /// Reads the process environment. `pressure_pinned` disables the real
    /// disk-pressure probe (operator pinned a synthetic sample).
    ///
    /// # Errors
    ///
    /// Infallible today: an invalid override warns and keeps the default.
    pub fn from_env(pressure_pinned: bool) -> anyhow::Result<Self> {
        Self::from_lookup(|name| std::env::var(name).ok(), pressure_pinned)
    }

    /// Same as [`Self::from_env`] with an injectable lookup (tests).
    ///
    /// # Errors
    ///
    /// Infallible today: an invalid override warns and keeps the default.
    pub fn from_lookup(
        lookup: impl Fn(&str) -> Option<String>,
        pressure_pinned: bool,
    ) -> anyhow::Result<Self> {
        let gc_secs = parse_secs(
            GC_INTERVAL_ENV,
            lookup(GC_INTERVAL_ENV),
            DEFAULT_GC_INTERVAL_SECS,
        )?;
        let pressure_secs = parse_secs(
            PRESSURE_INTERVAL_ENV,
            lookup(PRESSURE_INTERVAL_ENV),
            DEFAULT_PRESSURE_INTERVAL_SECS,
        )?;
        Ok(Self {
            gc_interval: (gc_secs > 0).then(|| Duration::from_secs(gc_secs)),
            pressure_interval: (pressure_secs > 0 && !pressure_pinned)
                .then(|| Duration::from_secs(pressure_secs)),
        })
    }
}

fn parse_secs(name: &str, raw: Option<String>, default: u64) -> anyhow::Result<u64> {
    let Some(raw) = raw else {
        return Ok(default);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(default);
    }
    // An invalid override must not stop the daemon (same policy as the #56
    // workspace reaper interval): warn once and keep the default.
    match trimmed.parse::<u64>() {
        Ok(value) => Ok(value),
        Err(error) => {
            tracing::warn!(
                code = "STORAGE_MAINTENANCE_INTERVAL_INVALID",
                env = name,
                value = %raw,
                default_secs = default,
                %error,
                "invalid storage maintenance interval; using default"
            );
            Ok(default)
        }
    }
}

/// Keeps the periodic tasks alive; dropping it stops them. The fields are held
/// only for their `Drop`.
#[derive(Debug)]
pub struct StorageMaintenance {
    _gc: Option<GcTask>,
    _pressure: Option<PressureTask>,
}

/// Starts the periodic GC and disk-pressure tasks on the active Tokio runtime.
///
/// # Errors
///
/// Returns an error when no Tokio runtime is active.
pub fn spawn_storage_maintenance(
    db: &Db,
    config: &StorageMaintenanceConfig,
) -> anyhow::Result<StorageMaintenance> {
    let gc = config
        .gc_interval
        .map(|interval| db.spawn_gc_task_with_interval(interval))
        .transpose()?;
    let pressure = config
        .pressure_interval
        .map(|interval| db.spawn_pressure_task_with_interval(interval))
        .transpose()?;
    tracing::info!(
        code = "STORAGE_MAINTENANCE_SCHEDULED",
        gc_interval_secs = config.gc_interval.map(|interval| interval.as_secs()),
        pressure_interval_secs = config.pressure_interval.map(|interval| interval.as_secs()),
        "periodic storage GC and disk-pressure tasks scheduled"
    );
    Ok(StorageMaintenance {
        _gc: gc,
        _pressure: pressure,
    })
}

/// Daemon startup entry point shared by the HTTP and stdio transports: opens
/// (or reuses) the shared storage handle, reads the environment configuration
/// and starts the tasks.
///
/// # Errors
///
/// Returns an error when the state lock is poisoned, storage cannot open, an
/// interval override is invalid, or no Tokio runtime is active.
pub fn spawn_for_daemon(
    m3_state: &SharedM3State,
    pressure_pinned: bool,
) -> anyhow::Result<StorageMaintenance> {
    let config = StorageMaintenanceConfig::from_env(pressure_pinned)?;
    let db = {
        let mut state = m3_state
            .lock()
            .map_err(|_poisoned| anyhow::anyhow!("m3 service state lock poisoned"))?;
        state.ensure_storage()?
    };
    spawn_storage_maintenance(&db, &config)
}

/// Runs `work` on tokio's blocking pool so a long RocksDB scan or compaction
/// never occupies an async worker.
///
/// # Errors
///
/// Returns a tool error when the blocking job panicked or was cancelled.
pub async fn run_on_blocking_pool<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, ErrorData> + Send + 'static,
) -> Result<T, ErrorData> {
    match tokio::task::spawn_blocking(work).await {
        Ok(result) => result,
        Err(error) => Err(mcp_error(
            error_codes::TOOL_INTERNAL_ERROR,
            format!("blocking storage job did not complete: {error}"),
        )),
    }
}

/// `storage gc_once` without blocking an async worker: the scan, eviction and
/// compaction (and the reflex-runtime mutex they hold) run on the blocking pool.
///
/// # Errors
///
/// Propagates the GC error, or a tool error when the blocking job failed.
pub async fn run_storage_gc_once_blocking(
    runtime: Arc<Mutex<ReflexRuntime>>,
    params: StorageGcOnceParams,
) -> Result<StorageGcOnceResponse, ErrorData> {
    run_on_blocking_pool(move || run_storage_gc_once(&runtime, &params)).await
}

#[cfg(test)]
#[path = "tests/storage_maintenance_60.rs"]
mod storage_maintenance_60_tests;

#[cfg(test)]
#[path = "tests/storage_maintenance_test_support.rs"]
mod storage_maintenance_test_support;
