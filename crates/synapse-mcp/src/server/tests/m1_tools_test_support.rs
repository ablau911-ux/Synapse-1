//! Shared helpers for the `m1_tools` regression tests (#228).

use std::{collections::BTreeSet, num::NonZeroUsize, path::Path};

use rmcp::{
    model::{ClientCapabilities, Implementation, InitializeRequestParams},
    transport::streamable_http_server::session::SessionState,
};
use tokio_util::sync::CancellationToken;

use super::{SessionTarget, SynapseService};
use crate::{m2::M2ServiceConfig, m3::M3ServiceConfig, m4::M4ServiceConfig};

pub(super) fn service_with_temp_db(path: &Path) -> anyhow::Result<SynapseService> {
    SynapseService::try_with_m2_shutdown_reason_and_m3_config(
        CancellationToken::new(),
        "test",
        CancellationToken::new(),
        &M2ServiceConfig::default(),
        M3ServiceConfig::from_cli_parts(
            Some(path.join("db")),
            Some(path.to_path_buf()),
            false,
            "127.0.0.1:0".to_owned(),
            NonZeroUsize::new(4).ok_or_else(|| anyhow::anyhow!("nonzero"))?,
            false,
            true,
            None,
            false,
            None,
        ),
        M4ServiceConfig::default(),
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn seed_spawned_child(
    service: &SynapseService,
    child_session_id: &str,
    parent_session_id: &str,
    spawn_id: &str,
    launcher_process_id: u32,
    agent_process_id: Option<u32>,
    log_root: &Path,
    now_unix_ms: u64,
) -> anyhow::Result<()> {
    service
        .session_registry_ref()
        .lock()
        .map_err(|_err| anyhow::anyhow!("session registry lock poisoned"))?
        .record_spawned_agent(
            child_session_id,
            crate::server::session_registry::SpawnedAgentRead {
                spawn_id: spawn_id.to_owned(),
                cli: "codex".to_owned(),
                launcher_process_id,
                agent_process_id,
                started_by_session_id: Some(parent_session_id.to_owned()),
                launched_at_unix_ms: now_unix_ms,
                launch_target: "test".to_owned(),
                log_dir: log_root.join(spawn_id).display().to_string(),
                template_id: None,
                template_version: None,
                control: None,
            },
            now_unix_ms,
        );
    Ok(())
}

pub(super) fn seed_session_client(
    service: &SynapseService,
    session_id: &str,
    client_name: &str,
    now_unix_ms: u64,
) -> anyhow::Result<()> {
    let state = SessionState::new(InitializeRequestParams::new(
        ClientCapabilities::default(),
        Implementation::new(client_name, "0.0.0-test"),
    ));
    service
        .session_registry_ref()
        .lock()
        .map_err(|_err| anyhow::anyhow!("session registry lock poisoned"))?
        .record_initialized(session_id, &state, "http", now_unix_ms);
    Ok(())
}

pub(super) fn insert_test_target_claim(
    service: &SynapseService,
    session_id: &str,
    window_hwnd: i64,
    target_id: &str,
) -> anyhow::Result<()> {
    let now = crate::server::session_registry::unix_time_ms_now();
    let mut live = BTreeSet::new();
    live.insert(session_id.to_owned());
    service
        .lock_target_claims()?
        .claim(
            session_id,
            SessionTarget::Cdp {
                window_hwnd,
                cdp_target_id: target_id.to_owned(),
            },
            60_000,
            now,
            &live,
        )
        .map_err(|conflict| anyhow::anyhow!("insert test target claim conflict: {conflict:?}"))?;
    Ok(())
}
