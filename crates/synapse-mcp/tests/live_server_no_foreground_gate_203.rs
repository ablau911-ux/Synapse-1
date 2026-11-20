//! #203 through the REAL daemon (stdio transport, real preflight in
//! `server/context.rs::ensure_supported_use_allows_action_scoped`): with
//! `SYNAPSE_ENFORCE_SUPPORTED_USE=1`, an active live-server profile and NO
//! foreground window (`SYNAPSE_MCP_FORCE_NO_FOREGROUND`), the live-server
//! deny-list (`target_policy.rs::live_server_denied_tool_detail`) must still
//! refuse `reflex_register` with `SAFETY_PROFILE_ACTION_DENIED` /
//! `live_server_tool_not_foreground_input`, and nothing may be registered.
//! Removing the policy call from the no-foreground branch fails these tests.

use std::path::Path;

use anyhow::Context;
use serde_json::{Value, json};
use synapse_test_utils::stdio_mcp_client::StdioMcpClient;
use tempfile::TempDir;

const PROFILE_ID: &str = "g1b.live203";
const REFLEX_KIND: &str = "g1b-203-reflex";

fn write_profile(dir: &Path, live_server_allowed: bool) -> anyhow::Result<()> {
    let toml = format!(
        r#"
id = "{PROFILE_ID}"
label = "G1b live-server 203"
schema_version = 2
use_scope = "productivity"
mode = "hybrid"
mouse_velocity_profile_default = "natural"
keyboard_dynamics_default = "natural"

[[matches]]
exe = "g1b-live-203.exe"

[capture]
target = "foreground_window"
min_update_interval_ms = 16
cursor_visible = true

[detection]
model_id = "g1b_live_203_detector"
classes_of_interest = ["window"]
confidence_threshold = 0.50
max_detections = 8

[metadata]
"supported_use.live_server_allowed" = "{live_server_allowed}"
"#
    );
    std::fs::write(dir.join(format!("{PROFILE_ID}.toml")), toml)?;
    Ok(())
}

/// Stdio daemon with no foreground window and the given profile active.
async fn launch(
    temp: &TempDir,
    enforce: bool,
    live_server_allowed: bool,
) -> anyhow::Result<StdioMcpClient> {
    let profiles = temp.path().join("profiles");
    std::fs::create_dir_all(&profiles)?;
    write_profile(&profiles, live_server_allowed)?;
    let db = temp.path().join("db").to_string_lossy().into_owned();
    let profiles = profiles.to_string_lossy().into_owned();
    let envs = vec![
        ("SYNAPSE_DB", db.as_str()),
        ("SYNAPSE_PROFILE_DIR", profiles.as_str()),
        ("SYNAPSE_MCP_FORCE_NO_FOREGROUND", "1"),
        (
            "SYNAPSE_ENFORCE_SUPPORTED_USE",
            if enforce { "1" } else { "0" },
        ),
    ];
    let mut client = StdioMcpClient::launch_and_init_with_env(None, &envs).await?;
    let activated = client
        .tools_call("profile_activate", json!({"profile_id": PROFILE_ID}))
        .await?;
    assert_eq!(
        activated["structuredContent"]["active_profile_id"], PROFILE_ID,
        "precondition: profile active: {activated}"
    );
    Ok(client)
}

fn reflex_args() -> Value {
    json!({
        "kind": "on_event",
        "when": {"op": "kind", "kind": REFLEX_KIND},
        "then": {"kind": "action", "action": {"kind": "release_all"}}
    })
}

async fn registry_ids(client: &mut StdioMcpClient) -> anyhow::Result<Vec<String>> {
    let list = client.tools_call("reflex_list", json!({})).await?;
    list["structuredContent"]["reflexes"]
        .as_array()
        .context("reflex_list statuses missing")?
        .iter()
        .map(|status| {
            status["id"]
                .as_str()
                .map(str::to_owned)
                .context("reflex id missing")
        })
        .collect()
}

async fn register_and_read_back(client: &mut StdioMcpClient) -> anyhow::Result<()> {
    let registered = client.tools_call("reflex_register", reflex_args()).await?;
    let id = registered["structuredContent"]["reflex_id"]
        .as_str()
        .context("registered reflex id missing")?;
    assert!(
        registry_ids(client)
            .await?
            .iter()
            .any(|registered| registered == id),
        "registered reflex {id} must be listed"
    );
    Ok(())
}

#[tokio::test]
async fn must_fail_live_server_profile_denies_reflex_register_without_foreground()
-> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let mut client = launch(&temp, true, true).await?;
    let before = registry_ids(&mut client).await?;
    assert!(before.is_empty(), "precondition: no reflex");

    let error = client
        .tools_call_error("reflex_register", reflex_args())
        .await?;
    let data = error.get("data").context("denial carries data")?;
    assert_eq!(data["code"], "SAFETY_PROFILE_ACTION_DENIED", "{error}");
    assert_eq!(
        data["reason"], "live_server_tool_not_foreground_input",
        "{error}"
    );
    assert_eq!(data["profile_id"], PROFILE_ID, "{error}");

    assert_eq!(
        registry_ids(&mut client).await?,
        before,
        "denied reflex was registered"
    );
    assert!(client.shutdown().await?.success());
    Ok(())
}

#[tokio::test]
async fn must_pass_enforcement_off_keeps_reflex_register_working_without_foreground()
-> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let mut client = launch(&temp, false, true).await?;
    register_and_read_back(&mut client).await?;
    assert!(client.shutdown().await?.success());
    Ok(())
}

#[tokio::test]
async fn must_pass_non_live_server_profile_is_not_denied_without_foreground() -> anyhow::Result<()>
{
    let temp = TempDir::new()?;
    let mut client = launch(&temp, true, false).await?;
    register_and_read_back(&mut client).await?;
    assert!(client.shutdown().await?.success());
    Ok(())
}
