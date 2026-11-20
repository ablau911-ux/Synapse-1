use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, bail};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const MAX_CONNECTIONS_ENV: &str = "SYNAPSE_HTTP_MAX_CONNECTIONS";
const HEADER_TIMEOUT_ENV: &str = "SYNAPSE_HTTP_HEADER_READ_TIMEOUT_SECS";
const RESERVE_ENV: &str = "SYNAPSE_HTTP_LIVENESS_CONNECTION_RESERVE";
const RESERVE_TIMEOUT_ENV: &str = "SYNAPSE_HTTP_LIVENESS_HEADER_READ_TIMEOUT_SECS";
const TRIAGE_ENV: &str = "SYNAPSE_HTTP_LIVENESS_TRIAGE_CAPACITY";

#[derive(Clone)]
pub(super) struct HttpConnectionAdmission {
    normal: Arc<Semaphore>,
    reserve: Arc<Semaphore>,
    triage: Arc<Semaphore>,
    reserve_triage: Arc<Semaphore>,
    max_connections: usize,
    header_timeout: Duration,
    reserve_size: usize,
    reserve_timeout: Duration,
    rejected: Arc<AtomicU64>,
    header_read_timeouts: Arc<AtomicU64>,
}

pub(super) struct ConnectionPermit {
    _permit: OwnedSemaphorePermit,
    header_timeout: Duration,
    keep_alive: bool,
}

impl ConnectionPermit {
    pub(super) const fn header_timeout(&self) -> Duration {
        self.header_timeout
    }

    /// Reserve connections serve exactly one liveness request, then close.
    pub(super) const fn keep_alive(&self) -> bool {
        self.keep_alive
    }
}

impl HttpConnectionAdmission {
    pub(super) fn from_env() -> anyhow::Result<Self> {
        let max_connections = parse_env(MAX_CONNECTIONS_ENV, 256, 1, 4096)?;
        let header_timeout =
            Duration::from_secs(u64::try_from(parse_env(HEADER_TIMEOUT_ENV, 10, 1, 120)?)?);
        let reserve_size = parse_env(RESERVE_ENV, 2, 1, 16)?;
        let reserve_timeout =
            Duration::from_secs(u64::try_from(parse_env(RESERVE_TIMEOUT_ENV, 1, 1, 120)?)?);
        let triage_capacity = parse_env(TRIAGE_ENV, 1024, 1, 8192)?;
        if reserve_timeout > header_timeout {
            bail!("liveness header deadline exceeds normal header deadline");
        }
        tracing::info!(
            code = "MCP_HTTP_CONNECTION_ADMISSION_CONFIGURED",
            max_connections,
            header_timeout_s = header_timeout.as_secs(),
            reserve_size,
            reserve_timeout_s = reserve_timeout.as_secs(),
            triage_capacity,
            "bounded HTTP pre-parse connection admission configured"
        );
        Ok(Self::with_limits(
            max_connections,
            header_timeout,
            reserve_size,
            reserve_timeout,
            triage_capacity,
        ))
    }

    /// Behavior-neutral constructor shared by `from_env` and the admission tests.
    pub(super) fn with_limits(
        max_connections: usize,
        header_timeout: Duration,
        reserve_size: usize,
        reserve_timeout: Duration,
        triage_capacity: usize,
    ) -> Self {
        Self {
            normal: Arc::new(Semaphore::new(max_connections)),
            reserve: Arc::new(Semaphore::new(reserve_size)),
            triage: Arc::new(Semaphore::new(triage_capacity)),
            reserve_triage: Arc::new(Semaphore::new(reserve_size)),
            max_connections,
            header_timeout,
            reserve_size,
            reserve_timeout,
            rejected: Arc::new(AtomicU64::new(0)),
            header_read_timeouts: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Normal budget only. The reserve is never handed out here (#19).
    pub(super) fn try_admit(&self) -> Option<ConnectionPermit> {
        let permit = Arc::clone(&self.normal).try_acquire_owned().ok()?;
        Some(ConnectionPermit {
            _permit: permit,
            header_timeout: self.header_timeout,
            keep_alive: true,
        })
    }

    /// Reserve slot, only for a connection already proven to carry a complete liveness head.
    pub(super) fn try_admit_liveness(&self) -> Option<ConnectionPermit> {
        let permit = Arc::clone(&self.reserve).try_acquire_owned().ok()?;
        Some(ConnectionPermit {
            _permit: permit,
            header_timeout: self.reserve_timeout,
            keep_alive: false,
        })
    }

    /// Bounded slot to hold an over-budget connection while its first bytes are peeked.
    /// When the general triage pool is full, a free liveness reserve slot still admits one
    /// candidate per reserve slot into bounded triage so fragmented or delayed `/health`
    /// heads are not dropped before inspection (#19).
    pub(super) fn try_triage(&self) -> Option<OwnedSemaphorePermit> {
        if let Ok(permit) = Arc::clone(&self.triage).try_acquire_owned() {
            return Some(permit);
        }
        if self.reserve.available_permits() == 0 {
            return None;
        }
        Arc::clone(&self.reserve_triage).try_acquire_owned().ok()
    }

    pub(super) const fn triage_deadline(&self) -> Duration {
        self.reserve_timeout
    }

    pub(super) fn note_rejected(&self) {
        self.rejected.fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn note_header_read_timeout(&self) {
        self.header_read_timeouts.fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn active_connections(&self) -> usize {
        self.max_connections
            .saturating_sub(self.normal.available_permits())
            .saturating_add(
                self.reserve_size
                    .saturating_sub(self.reserve.available_permits()),
            )
    }

    pub(super) const fn max_connections(&self) -> usize {
        self.max_connections
    }

    pub(super) fn rejected_total(&self) -> u64 {
        self.rejected.load(Ordering::Relaxed)
    }

    pub(super) fn header_read_timeouts_total(&self) -> u64 {
        self.header_read_timeouts.load(Ordering::Relaxed)
    }
}

fn parse_env(name: &str, default: usize, minimum: usize, maximum: usize) -> anyhow::Result<usize> {
    let raw = match std::env::var(name) {
        Ok(raw) => raw,
        Err(std::env::VarError::NotPresent) => return Ok(default),
        Err(error) => return Err(error).with_context(|| format!("read {name}")),
    };
    let value = raw
        .trim()
        .parse::<usize>()
        .with_context(|| format!("parse {name}={raw:?}"))?;
    if !(minimum..=maximum).contains(&value) {
        bail!("{name} must be between {minimum} and {maximum}, got {value}");
    }
    Ok(value)
}
