//! #158: diagnostic heartbeat. A dedicated OS thread (not a Tokio task, so a
//! stalled runtime cannot silence it by itself) overwrites one small file,
//! `daemon-heartbeat.json`, next to the lifecycle ledger every N seconds.
//! The file is replaced atomically, never appended, so it cannot grow.
//!
//! On the next start, the unclean-run detection in `daemon_lifecycle` calls
//! [`assess`] and logs `last_heartbeat_age_ms` and `suspected=hang|kill|power`.
//! This explains an unclean exit AFTER the fact; it is not live detection and a
//! hung process cannot report itself. The classification is a best-effort
//! heuristic, not proof:
//!   * `hang`  - the last heartbeat is more than 3 intervals older than the last
//!     ledger activity (tool events), i.e. the process kept recording work after
//!     the heartbeat went stale, so the heartbeat thread or process stalled.
//!   * `power` - the heartbeat was fresh relative to ledger activity and the
//!     machine booted after the last beat (reboot or power loss, no exit record).
//!   * `kill`  - fresh heartbeat, machine not rebooted since: the process was
//!     terminated externally (task kill, crash). Boot time comes from the tick
//!     counter, which may undercount time spent asleep, so `power` can be
//!     over-reported after a suspend.
//!   * `unknown` - no heartbeat for that run (disabled, older build, unreadable).
//!
//! `SYNAPSE_HEARTBEAT_INTERVAL_SECS`: default 30, `0` disables, invalid values
//! fall back to the default and are reported.
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use serde::{Deserialize, Serialize};

use crate::daemon_lifecycle::{now_unix_ms, read_optional_json, write_json_atomic};

pub(crate) const HEARTBEAT_FILE: &str = "daemon-heartbeat.json";
pub(crate) const DEFAULT_INTERVAL_SECS: u64 = 30;
const INTERVAL_ENV: &str = "SYNAPSE_HEARTBEAT_INTERVAL_SECS";
const SCHEMA_VERSION: u32 = 1;
/// Heartbeat older than this many intervals before last activity means hang.
const STALE_INTERVALS: u64 = 3;
/// Granularity at which the loop re-checks the stop flag while sleeping.
const STOP_POLL: Duration = Duration::from_millis(5);

static STOP: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct Heartbeat {
    pub schema_version: u32,
    pub run_id: String,
    pub pid: u32,
    pub interval_ms: u64,
    pub beat_unix_ms: u64,
    pub seq: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Suspected {
    Hang,
    Kill,
    Power,
    Unknown,
}

impl Suspected {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Hang => "hang",
            Self::Kill => "kill",
            Self::Power => "power",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ClassifyInput {
    pub last_beat_unix_ms: Option<u64>,
    pub interval_ms: u64,
    pub last_activity_unix_ms: u64,
    pub boot_unix_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Assessment {
    pub last_heartbeat_age_ms: Option<u64>,
    pub suspected: Suspected,
}

/// Resolve the interval from the raw env value. `None` interval = disabled.
/// Returns the rejected raw value when it was invalid (default is used).
pub(crate) fn resolve_interval(raw: Option<&str>) -> (Option<Duration>, Option<String>) {
    let default = Some(Duration::from_secs(DEFAULT_INTERVAL_SECS));
    let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return (default, None);
    };
    match raw.parse::<u64>() {
        Ok(0) => (None, None),
        Ok(secs) => (Some(Duration::from_secs(secs)), None),
        Err(_) => (default, Some(raw.to_owned())),
    }
}

pub(crate) fn heartbeat_path(dir: &Path) -> PathBuf {
    dir.join(HEARTBEAT_FILE)
}

pub(crate) fn write_heartbeat(dir: &Path, heartbeat: &Heartbeat) -> anyhow::Result<()> {
    write_json_atomic(&heartbeat_path(dir), heartbeat)
}

pub(crate) fn read_heartbeat(dir: &Path) -> anyhow::Result<Option<Heartbeat>> {
    read_optional_json(&heartbeat_path(dir))
}

pub(crate) fn classify(input: &ClassifyInput) -> Suspected {
    let Some(last_beat) = input.last_beat_unix_ms else {
        return Suspected::Unknown;
    };
    if input.interval_ms == 0 {
        return Suspected::Unknown;
    }
    let stale_after = input.interval_ms.saturating_mul(STALE_INTERVALS);
    if input.last_activity_unix_ms.saturating_sub(last_beat) > stale_after {
        return Suspected::Hang;
    }
    match input.boot_unix_ms {
        Some(boot) if boot > last_beat => Suspected::Power,
        _ => Suspected::Kill,
    }
}

/// Read the previous run's heartbeat and classify its unclean end. Never fails:
/// unreadable or foreign-run heartbeat files degrade to `unknown`.
pub(crate) fn assess(
    dir: &Path,
    previous_run_id: &str,
    last_activity_unix_ms: u64,
    now_unix_ms: u64,
    boot_unix_ms: Option<u64>,
) -> Assessment {
    let heartbeat = match read_heartbeat(dir) {
        Ok(found) => found.filter(|beat| beat.run_id == previous_run_id),
        Err(error) => {
            tracing::warn!(code = "MCP_DAEMON_HEARTBEAT_READ_FAILED", %error,
                "cannot read previous heartbeat; suspected cause unknown");
            None
        }
    };
    let Some(beat) = heartbeat else {
        return Assessment {
            last_heartbeat_age_ms: None,
            suspected: Suspected::Unknown,
        };
    };
    let suspected = classify(&ClassifyInput {
        last_beat_unix_ms: Some(beat.beat_unix_ms),
        interval_ms: beat.interval_ms,
        last_activity_unix_ms,
        boot_unix_ms,
    });
    Assessment {
        last_heartbeat_age_ms: Some(now_unix_ms.saturating_sub(beat.beat_unix_ms)),
        suspected,
    }
}

/// Approximate machine boot time, from the tick counter (None off Windows).
pub(crate) fn boot_unix_ms() -> Option<u64> {
    #[cfg(windows)]
    {
        use windows::Win32::System::SystemInformation::GetTickCount64;
        // SAFETY: GetTickCount64 takes no arguments and has no preconditions.
        let uptime_ms = unsafe { GetTickCount64() };
        now_unix_ms().checked_sub(uptime_ms)
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Beat until `stop` is set. Blocking; run on a dedicated OS thread.
pub(crate) fn run_loop(dir: &Path, run_id: &str, interval: Duration, stop: &AtomicBool) {
    let interval_ms = u64::try_from(interval.as_millis()).unwrap_or(u64::MAX);
    let mut seq = 0_u64;
    let mut failing = false;
    while !stop.load(Ordering::Acquire) {
        seq += 1;
        let beat = Heartbeat {
            schema_version: SCHEMA_VERSION,
            run_id: run_id.to_owned(),
            pid: std::process::id(),
            interval_ms,
            beat_unix_ms: now_unix_ms(),
            seq,
        };
        match write_heartbeat(dir, &beat) {
            Ok(()) => failing = false,
            // Log the transition only, so a full disk does not spam every beat.
            Err(error) if !failing => {
                failing = true;
                tracing::warn!(code = "MCP_DAEMON_HEARTBEAT_WRITE_FAILED", %error,
                    "heartbeat write failed; will keep retrying");
            }
            Err(_) => {}
        }
        let mut slept = Duration::ZERO;
        while slept < interval && !stop.load(Ordering::Acquire) {
            let step = STOP_POLL.min(interval - slept);
            std::thread::sleep(step);
            slept += step;
        }
    }
}

/// Start the heartbeat thread per `SYNAPSE_HEARTBEAT_INTERVAL_SECS`.
pub(crate) fn start_from_env(dir: &Path, run_id: &str) {
    let raw = std::env::var(INTERVAL_ENV).ok();
    let (interval, rejected) = resolve_interval(raw.as_deref());
    if let Some(value) = rejected {
        tracing::warn!(code = "MCP_DAEMON_HEARTBEAT_INTERVAL_INVALID", value = %value,
            default_secs = DEFAULT_INTERVAL_SECS, "unknown {INTERVAL_ENV}; using default");
    }
    let Some(interval) = interval else {
        tracing::info!(
            code = "MCP_DAEMON_HEARTBEAT_DISABLED",
            "{INTERVAL_ENV}=0; heartbeat off"
        );
        return;
    };
    let (dir, run_id) = (dir.to_path_buf(), run_id.to_owned());
    let started = std::thread::Builder::new()
        .name("synapse-heartbeat".into())
        .spawn({
            let (dir, run_id) = (dir.clone(), run_id.clone());
            move || run_loop(&dir, &run_id, interval, &STOP)
        });
    match started {
        Ok(_) => {
            tracing::info!(code = "MCP_DAEMON_HEARTBEAT_STARTED", path = %heartbeat_path(&dir).display(),
            interval_secs = interval.as_secs(), run_id = %run_id, "diagnostic heartbeat thread started")
        }
        Err(error) => tracing::error!(code = "MCP_DAEMON_HEARTBEAT_START_FAILED", %error,
            "cannot start heartbeat thread"),
    }
}
