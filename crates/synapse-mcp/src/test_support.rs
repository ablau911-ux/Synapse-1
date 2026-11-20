use std::sync::{Mutex, MutexGuard, PoisonError};

#[path = "request_context_support.rs"]
mod request_context_support;
pub(crate) use request_context_support::request_context_for_session;

#[cfg(windows)]
#[path = "owned_claim_window_support.rs"]
mod owned_claim_window_support;
#[cfg(windows)]
pub(crate) use owned_claim_window_support::{OwnedClaimWindow, owned_claim_window};

#[cfg(windows)]
#[path = "pumped_window_support.rs"]
mod pumped_window_support;
#[cfg(windows)]
pub(crate) use pumped_window_support::{PerMonitorDpi, PumpedWindows, from_hwnd, to_hwnd};

static LEASE_SERIAL: Mutex<()> = Mutex::new(());
static DAEMON_LIFECYCLE_SERIAL: Mutex<()> = Mutex::new(());
static HOST_ENV_SERIAL: Mutex<()> = Mutex::new(());

/// Serializes every mailbox-using test with tests requiring a non-skipped sweep.
/// Sync tests use `blocking_lock()`; async tests use `lock().await` so their
/// guard can cross await points. Hold for the whole test, including worker joins.
/// Acquire before `REAL_PROCESS_LOCK` when both guards are needed.
pub(crate) static MAILBOX_SWEEP_SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Serializes tests using the process-wide agent tracker, including fleet kills.
/// Acquire before `lease_serial` when both guards are needed.
pub(crate) static REAL_PROCESS_LOCK: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

/// Crate-wide serializer for tests that mutate or depend on host-path env vars
/// (`LOCALAPPDATA`, `USERPROFILE`, `HOME`, `CLAUDE_CONFIG_DIR`, transcript and
/// projects roots). Every such test must hold it for its whole body.
pub(crate) fn host_env_serial() -> MutexGuard<'static, ()> {
    HOST_ENV_SERIAL
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

static DESKTOP_TOPOLOGY_SERIAL: Mutex<()> = Mutex::new(());

/// Crate-wide serializer for tests that put visible windows on the shared
/// desktop or assert which window is under a screen point (`WindowFromPoint`).
/// Parallel tests that do this cover each other's points, so each holds this
/// guard for as long as its windows exist; window fixtures hold it for their
/// whole lifetime. Never take it twice in one test (std mutex, not reentrant).
pub(crate) fn desktop_topology_serial() -> MutexGuard<'static, ()> {
    DESKTOP_TOPOLOGY_SERIAL
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

pub(crate) struct LeaseSerialGuard {
    _guard: MutexGuard<'static, ()>,
    reason: String,
}

impl Drop for LeaseSerialGuard {
    fn drop(&mut self) {
        let _prior = synapse_action::lease::force_clear(&self.reason);
    }
}

pub(crate) fn lease_serial(reason: &str) -> LeaseSerialGuard {
    let guard = LEASE_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let _prior = synapse_action::lease::force_clear(reason);
    LeaseSerialGuard {
        _guard: guard,
        reason: reason.to_owned(),
    }
}

pub(crate) struct DaemonLifecycleSerialGuard {
    _guard: MutexGuard<'static, ()>,
}

impl Drop for DaemonLifecycleSerialGuard {
    fn drop(&mut self) {
        crate::daemon_lifecycle::reset_for_test();
    }
}

pub(crate) fn daemon_lifecycle_serial() -> DaemonLifecycleSerialGuard {
    let guard = DAEMON_LIFECYCLE_SERIAL
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    crate::daemon_lifecycle::reset_for_test();
    DaemonLifecycleSerialGuard { _guard: guard }
}

/// Builds a `SynapseService` whose storage lives under `dir/db` (startup
/// reconciliation runs in the constructor, like the daemon).
pub(crate) fn service_in(dir: &std::path::Path) -> anyhow::Result<crate::server::SynapseService> {
    use crate::{m2::M2ServiceConfig, m3::M3ServiceConfig, m4::M4ServiceConfig};
    crate::server::SynapseService::try_with_m2_shutdown_reason_and_m3_config(
        tokio_util::sync::CancellationToken::new(),
        "test",
        tokio_util::sync::CancellationToken::new(),
        &M2ServiceConfig::default(),
        M3ServiceConfig::from_cli_parts(
            Some(dir.join("db")),
            Some(dir.to_path_buf()),
            false,
            "127.0.0.1:0".to_owned(),
            std::num::NonZeroUsize::new(4).ok_or_else(|| anyhow::anyhow!("nonzero"))?,
            false,
            true,
            None,
            false,
            None,
        ),
        M4ServiceConfig::default(),
    )
}

/// Reason codes of every `LeaseReleased` journal event for `session_id`, in
/// storage order.
pub(crate) fn lease_released_reasons(
    db: &synapse_storage::Db,
    session_id: &str,
) -> anyhow::Result<Vec<String>> {
    Ok(db
        .scan_cf(synapse_storage::cf::CF_AGENT_EVENTS)?
        .iter()
        .filter_map(|(_key, value)| {
            serde_json::from_slice::<synapse_core::AgentEventRecord>(value).ok()
        })
        .filter(|record| {
            record.kind == synapse_core::AgentEventKind::LeaseReleased
                && record.session_id.as_deref() == Some(session_id)
        })
        .map(|record| record.reason_code.unwrap_or_default())
        .collect())
}

static DIGEST_KEY_STATE_SERIAL: Mutex<()> = Mutex::new(());

/// Serializes tests that force the process-wide content-digest key state
/// (#269). Hold it for the whole test, including the reset.
pub(crate) fn digest_key_state_serial() -> MutexGuard<'static, ()> {
    DIGEST_KEY_STATE_SERIAL
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// Every textual form in which `key` could leak into health, logs or errors:
/// lower/upper hex, standard and URL-safe base64, `Debug` and decimal lists.
pub(crate) fn key_material_renderings(key: &[u8]) -> Vec<String> {
    use base64::Engine as _;
    let hex: String = key.iter().map(|byte| format!("{byte:02x}")).collect();
    let decimals: Vec<String> = key.iter().map(ToString::to_string).collect();
    vec![
        hex.to_uppercase(),
        hex,
        base64::engine::general_purpose::STANDARD.encode(key),
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(key),
        format!("{key:?}"),
        decimals.join(", "),
        decimals.join(","),
    ]
}
