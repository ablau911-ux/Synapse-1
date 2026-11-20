use anyhow::Context;
use serde_json::{Value, json};
use synapse_core::error_codes;
use synapse_test_utils::stdio_mcp_client::StdioMcpClient;
use tempfile::TempDir;

#[tokio::test]
async fn must_fail_m3_permissions_refuse_ungranted_tool_calls_and_replay_path_escape()
-> anyhow::Result<()> {
    let logs = TempDir::new()?;
    let db_dir = TempDir::new()?;
    let db_path = db_dir.path().join("db");
    let mut client = StdioMcpClient::launch_and_init_with_env(
        Some(logs.path()),
        &[
            ("SYNAPSE_DEBUG_TOOLS", "1"),
            ("SYNAPSE_MCP_ALLOWED_PERMISSIONS", "none"),
            ("SYNAPSE_DB", db_path.to_str().context("db path utf8")?),
        ],
    )
    .await?;

    // `subscribe` is session-owned: ownerless stdio callers are refused before
    // the permission check, so its READ_EVENTS gate cannot be exercised here.
    assert_missing_permission(
        &client.tools_call_error("reflex_list", json!({})).await?,
        "READ_REFLEX",
    );
    assert_missing_permission(
        &client.tools_call_error("profile_list", json!({})).await?,
        "READ_PROFILE",
    );
    assert_missing_permission(
        &client
            .tools_call_error("replay_record", json!({"duration_ms": 0}))
            .await?,
        "WRITE_REPLAY",
    );
    assert_missing_permission(
        &client
            .tools_call_error("audio_tail", json!({"seconds": 0}))
            .await?,
        "READ_AUDIO",
    );
    assert_missing_permission(
        &client
            .tools_call_error("storage_inspect", json!({}))
            .await?,
        "READ_STORAGE",
    );
    assert_missing_permission(
        &client
            .tools_call_error(
                "storage_put_probe_rows",
                json!({"cf_name": "events", "key_prefix": "deny", "rows": 1, "value_bytes": 1}),
            )
            .await?,
        "WRITE_STORAGE",
    );

    let status = client.shutdown().await?;
    assert!(status.success());
    // The refused WRITE_STORAGE probe wrote no row anywhere.
    let db = synapse_storage::Db::open(&db_path, synapse_core::SCHEMA_VERSION)?;
    for cf_name in synapse_storage::cf::ALL_COLUMN_FAMILIES {
        let rows = db.scan_cf_prefix(cf_name, b"deny")?;
        assert!(
            rows.is_empty(),
            "refused probe wrote {} rows to {cf_name}",
            rows.len()
        );
    }
    drop(db);

    let logs = TempDir::new()?;
    let local_app_data = TempDir::new()?;
    let outside_dir = TempDir::new()?;
    let outside = outside_dir.path().join("outside.jsonl");
    let mut client = StdioMcpClient::launch_and_init_with_env(
        Some(logs.path()),
        &[
            ("SYNAPSE_MCP_ALLOWED_PERMISSIONS", "WRITE_REPLAY"),
            (
                "LOCALAPPDATA",
                local_app_data
                    .path()
                    .to_str()
                    .context("LOCALAPPDATA path utf8")?,
            ),
        ],
    )
    .await?;

    let error = client
        .tools_call_error("replay_record", json!({"duration_ms": 0, "path": outside}))
        .await?;
    assert_eq!(
        error["data"]["code"],
        error_codes::SAFETY_PERMISSION_DENIED,
        "unexpected replay_record error: {error}"
    );
    assert_eq!(error["data"]["permission"], "WRITE_REPLAY");
    assert_eq!(error["data"]["reason"], "path_outside_allow_root");
    assert!(error["data"]["allow_root"].as_str().is_some_and(|path| {
        path.ends_with(r"synapse\replays") || path.ends_with("synapse/replays")
    }));
    assert!(
        !outside.exists(),
        "refused replay_record must not create {}",
        outside.display()
    );

    let status = client.shutdown().await?;
    assert!(status.success());
    Ok(())
}

/// An explicit grant list (comma, semicolon, and whitespace separated) admits
/// exactly the granted permissions: their tools run, while a tool that needs an
/// ungranted permission is still refused naming that permission.
#[tokio::test]
async fn must_pass_m3_permissions_granted_list_admits_exactly_its_tools() -> anyhow::Result<()> {
    let logs = TempDir::new()?;
    let mut client = StdioMcpClient::launch_and_init_with_env(
        Some(logs.path()),
        &[
            ("SYNAPSE_DEBUG_TOOLS", "1"),
            (
                "SYNAPSE_MCP_ALLOWED_PERMISSIONS",
                "READ_REFLEX, READ_PROFILE;READ_STORAGE",
            ),
        ],
    )
    .await?;

    let reflexes = client.tools_call("reflex_list", json!({})).await?;
    let reflexes = reflexes
        .get("structuredContent")
        .context("reflex_list structuredContent missing")?;
    assert_eq!(
        reflexes["reflexes"],
        json!([]),
        "granted READ_REFLEX must serve reflex_list (empty on a fresh DB): {reflexes}"
    );

    let profiles = client.tools_call("profile_list", json!({})).await?;
    let profiles = profiles
        .get("structuredContent")
        .context("profile_list structuredContent missing")?;
    assert!(
        profiles["profiles"].as_array().is_some(),
        "granted READ_PROFILE must serve profile_list: {profiles}"
    );

    let inspect = client.tools_call("storage_inspect", json!({})).await?;
    assert!(
        inspect["structuredContent"]["cf_row_counts"].is_object(),
        "granted READ_STORAGE must serve storage_inspect with cf_row_counts: {inspect}"
    );

    // WRITE_STORAGE was not in the list: the write stays refused.
    assert_missing_permission(
        &client
            .tools_call_error(
                "storage_put_probe_rows",
                json!({"cf_name": "events", "key_prefix": "deny", "rows": 1, "value_bytes": 1}),
            )
            .await?,
        "WRITE_STORAGE",
    );

    let status = client.shutdown().await?;
    assert!(status.success());
    Ok(())
}

fn assert_missing_permission(error: &Value, permission: &str) {
    assert_eq!(
        error["data"]["code"],
        error_codes::SAFETY_PERMISSION_DENIED,
        "expected a missing {permission} permission refusal, got: {error}"
    );
    assert_eq!(error["data"]["missing_permission"], permission);
}
