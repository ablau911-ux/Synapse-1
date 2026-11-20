//! Periodic in-daemon routine mining batch job (#848).
//!
//! Re-mines `CF_ROUTINES` from `CF_EPISODES` on a fixed interval so the
//! routine store tracks the operator's evolving behavior without anyone
//! calling `routine_mine` by hand. The job shares
//! [`super::routines::mine_and_store_routines`] (and its process-wide
//! mining lock) with the MCP tool, so the two paths can never interleave a
//! replace-all.
//!
//! Before each run the job first cuts the new timeline rows into episodes
//! (`EpisodeCatchUp`, #607), so the miner sees the owner's real history; a
//! failed catch-up is logged and the run still mines.
//!
//! Configuration (read once at daemon startup; invalid values are a startup
//! error, not a silent default):
//!
//! - `SYNAPSE_ROUTINE_MINE_INTERVAL_SECS` — seconds between runs
//!   (default 21600 = 6 h; `0` disables the job).
//! - `SYNAPSE_ROUTINE_MINE_STARTUP_DELAY_SECS` — delay before the first run
//!   (default 300 s, so startup cost never lands on the recorder's warmup).
//! - `SYNAPSE_ROUTINE_MINE_WINDOW_DAYS` — days of recent history each run
//!   mines (default 28; `0` = the whole history). Over a long history every
//!   frequent pattern also runs at other hours and is rejected as dispersed,
//!   and the scan budget grows with it (#607 second part).
//!
//! A failed run is logged loudly (`ROUTINE_MINE_PERIODIC_FAILED`) and the
//! job keeps its schedule — one bad run must not stop future mining — but
//! the failure is never swallowed: the structured log carries the exact
//! error for diagnosis.

use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

use super::M3State;
use super::episode_catch_up::EpisodeCatchUp;
use super::episodes::now_ts_ns;
use super::mining_window::{MiningWindow, WINDOW_DAYS_ENV};
use super::routines::{RoutineMineParams, mine_and_store_routines};

/// Environment variable: seconds between periodic mining runs.
pub const INTERVAL_ENV: &str = "SYNAPSE_ROUTINE_MINE_INTERVAL_SECS";
/// Environment variable: seconds before the first run.
pub const STARTUP_DELAY_ENV: &str = "SYNAPSE_ROUTINE_MINE_STARTUP_DELAY_SECS";
/// Default interval: 6 hours.
pub const DEFAULT_INTERVAL_SECS: u64 = 21_600;
/// Default startup delay: 5 minutes.
pub const DEFAULT_STARTUP_DELAY_SECS: u64 = 300;

fn parse_secs_env(name: &str, default: u64) -> anyhow::Result<u64> {
    match std::env::var(name) {
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(error) => anyhow::bail!("{name} is not valid unicode: {error}"),
        Ok(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                Ok(default)
            } else {
                trimmed.parse::<u64>().map_err(|error| {
                    anyhow::anyhow!(
                        "{name} must be an unsigned integer of seconds; got {value:?}: {error}"
                    )
                })
            }
        }
    }
}

/// Spawns the periodic miner. Returns `Ok(None)` when disabled by
/// configuration (interval `0`); the decision is logged either way.
///
/// # Errors
///
/// Returns an error when an environment override is present but
/// unparseable — a misconfigured daemon must fail at startup, not run with
/// a silently substituted schedule.
pub fn spawn_periodic_miner(
    m3_state: Arc<Mutex<M3State>>,
    cancel: CancellationToken,
) -> anyhow::Result<Option<tokio::task::JoinHandle<()>>> {
    let interval_secs = parse_secs_env(INTERVAL_ENV, DEFAULT_INTERVAL_SECS)?;
    let startup_delay_secs = parse_secs_env(STARTUP_DELAY_ENV, DEFAULT_STARTUP_DELAY_SECS)?;
    let window = match std::env::var(WINDOW_DAYS_ENV) {
        Err(std::env::VarError::NotPresent) => MiningWindow::from_env_value(None),
        Err(error) => Err(format!("{WINDOW_DAYS_ENV} is not valid unicode: {error}")),
        Ok(value) => MiningWindow::from_env_value(Some(&value)),
    }
    .map_err(anyhow::Error::msg)?;
    if interval_secs == 0 {
        tracing::info!(
            code = "ROUTINE_MINE_PERIODIC_DISABLED",
            "periodic routine mining disabled via {INTERVAL_ENV}=0"
        );
        return Ok(None);
    }
    tracing::info!(
        code = "ROUTINE_MINE_PERIODIC_SCHEDULED",
        interval_secs,
        startup_delay_secs,
        window_days = window.days(),
        "periodic routine mining scheduled"
    );
    let handle = tokio::spawn(async move {
        let mut delay = std::time::Duration::from_secs(startup_delay_secs);
        loop {
            tokio::select! {
                () = cancel.cancelled() => {
                    tracing::info!(
                        code = "ROUTINE_MINE_PERIODIC_STOPPED",
                        "periodic routine mining stopped by daemon shutdown"
                    );
                    return;
                }
                () = tokio::time::sleep(delay) => {}
            }
            // Storage-bound work (the episode catch-up and mining) runs on
            // the blocking pool, not on a runtime worker.
            let state = Arc::clone(&m3_state);
            if let Err(error) = tokio::task::spawn_blocking(move || run_once(&state, window)).await
            {
                tracing::error!(
                    code = "ROUTINE_MINE_PERIODIC_FAILED",
                    detail = %error,
                    "periodic routine mining run did not finish; next run keeps the schedule"
                );
            }
            delay = std::time::Duration::from_secs(interval_secs);
        }
    });
    Ok(Some(handle))
}

/// One periodic mining run over the rolling window of recent days.
fn run_once(m3_state: &Arc<Mutex<M3State>>, window: MiningWindow) {
    let db = {
        let mut state = match m3_state.lock() {
            Ok(state) => state,
            Err(_poisoned) => {
                tracing::error!(
                    code = "ROUTINE_MINE_PERIODIC_FAILED",
                    detail = "m3 state lock poisoned",
                    "periodic routine mining could not access storage"
                );
                return;
            }
        };
        match state.ensure_storage() {
            Ok(db) => db,
            Err(error) => {
                tracing::error!(
                    code = "ROUTINE_MINE_PERIODIC_FAILED",
                    detail = %error,
                    "periodic routine mining could not open storage"
                );
                return;
            }
        }
    };
    // #607: cut the timeline into episodes first, so the miner sees the
    // owner's real history without anyone calling episode_segment by hand.
    // A failed catch-up is logged and the run still mines what is stored.
    match EpisodeCatchUp::run(&db) {
        Ok(catch_up) => tracing::info!(
            code = "ROUTINE_EPISODE_CATCH_UP_OK",
            episodes_written = catch_up.episodes_written(),
            "episodes brought up to date before periodic routine mining"
        ),
        Err(error) => tracing::error!(
            code = "ROUTINE_EPISODE_CATCH_UP_FAILED",
            error_code = %error.code.0,
            detail = %error.message,
            "episode catch-up failed; mining the episodes already stored"
        ),
    }
    let params = RoutineMineParams {
        start_ts_ns: window.start_ts_ns(now_ts_ns()),
        ..RoutineMineParams::default()
    };
    match mine_and_store_routines(&db, &params) {
        Ok(response) => {
            tracing::info!(
                code = "ROUTINE_MINE_PERIODIC_OK",
                routines_written = response.routines_written,
                routines_deleted = response.routines_deleted,
                active_days = response.active_days,
                eligible_episodes = response.eligible_episodes,
                candidates = response.candidates_evaluated,
                "periodic routine mining completed"
            );
        }
        Err(error) => {
            tracing::error!(
                code = "ROUTINE_MINE_PERIODIC_FAILED",
                error_code = %error.code.0,
                detail = %error.message,
                "periodic routine mining failed; next run keeps the schedule"
            );
        }
    }
}
