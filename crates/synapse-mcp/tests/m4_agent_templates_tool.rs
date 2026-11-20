//! Integration regression coverage for agent spawn templates (#909, simplified).
//!
//! A template is exactly five operator fields: name, description, model,
//! directory, prompt. Every assertion is grounded in the **source of truth**:
//! the daemon's `RocksDB` `CF_KV` column family. We drive the real MCP daemon
//! over stdio, read it back through a *different* tool (get/list), and finally
//! shut the daemon down and open its `RocksDB` directly to scan the physical
//! `agent-template/v2/cur/...` rows and decode their JSON.
//!
//! It also audits the boundary cases: a put edits in place (no versioning), an
//! empty prompt and an unregistered model are rejected loudly, delete makes the
//! row vanish, and spawning from a deleted template or alongside a
//! template-owned field fails with nothing launched.

use std::path::{Path, PathBuf};

use anyhow::{Context, ensure};
use serde_json::{Value, json};
use synapse_core::SCHEMA_VERSION;
use synapse_storage::{Db, cf};
use synapse_test_utils::stdio_mcp_client::StdioMcpClient;

/// Pulls the `structuredContent` object out of a successful tools/call result.
fn structured(result: &Value) -> anyhow::Result<&Value> {
    result
        .get("structuredContent")
        .with_context(|| format!("missing structuredContent in {result}"))
}

fn db_path_under(dir: &Path) -> PathBuf {
    dir.join("db")
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn must_pass_agent_templates_crud_round_trips_against_physical_cf_rows() -> anyhow::Result<()>
{
    let db_dir = tempfile::Builder::new()
        .prefix("synapse-agent-templates-regression")
        .tempdir()?;
    let db_path = db_path_under(db_dir.path());
    let db_path_str = db_path.to_string_lossy().into_owned();

    // Synthetic notepad foreground: act_spawn_agent runs the supported-use scope
    // gate, which reads the real GetForegroundWindow() and fails A11Y_NO_FOREGROUND
    // when the host has no focused window — masking the template-state error this
    // test actually asserts. The fixture makes the foreground a controlled input.
    let mut client = StdioMcpClient::launch_and_init_with_env(
        None,
        &[
            ("SYNAPSE_DB", db_path_str.as_str()),
            ("SYNAPSE_MCP_SYNTHETIC_FIXTURE", "notepad"),
            ("SYNAPSE_DEBUG_TOOLS", "1"),
        ],
    )
    .await?;

    // ---- BEFORE state: the store is empty --------------------------------
    let empty = client.tools_call("agent_template_list", json!({})).await?;
    let empty = structured(&empty)?;
    println!("readback=agent_template_list edge=before state={empty}");
    ensure!(
        empty["count"] == json!(0) && empty["templates"] == json!([]),
        "store must start empty, got {empty}"
    );

    // ---- ACTION 1: create a template -------------------------------------
    let created = client
        .tools_call(
            "agent_template_put",
            json!({
                "template_id": "reviewer",
                "name": "Code reviewer",
                "description": "Reviews the repo for correctness bugs.",
                "model": "claude",
                "directory": "C:\\code\\Synapse",
                "prompt": "Review the repo for correctness bugs."
            }),
        )
        .await?;
    let created = structured(&created)?;
    println!("readback=agent_template_put edge=create state={created}");
    ensure!(
        created["created"] == json!(true),
        "first put must be created"
    );
    let hash_v1 = created["template"]["config_hash"]
        .as_str()
        .context("config_hash missing")?
        .to_owned();
    // #174: config_hash is a keyed digest, `hmac-sha256:` + 64 hex chars.
    let hash_hex = hash_v1
        .strip_prefix("hmac-sha256:")
        .context("config_hash must be an hmac-sha256: keyed digest (#174)")?;
    ensure!(
        hash_hex.len() == 64 && hash_hex.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "config_hash must be hmac-sha256: + 64 hex chars"
    );
    // The put response reports exactly which physical row it wrote (one pointer).
    let written = created["written_rows"]
        .as_array()
        .context("written_rows missing")?;
    ensure!(written.len() == 1, "put writes a single pointer row");
    ensure!(
        written[0]["cf_name"] == json!("CF_KV"),
        "row must live in CF_KV"
    );

    // ---- VERIFY via a different tool (independent RocksDB read) -----------
    let got = client
        .tools_call("agent_template_get", json!({"template_id": "reviewer"}))
        .await?;
    let got = structured(&got)?;
    println!("readback=agent_template_get edge=after_create state={got}");
    ensure!(
        got["template"]["model"] == json!("claude")
            && got["template"]["description"] == json!("Reviews the repo for correctness bugs.")
            && got["template"]["directory"] == json!("C:\\code\\Synapse")
            && got["row_key"] == json!("agent-template/v2/cur/reviewer"),
        "current get must return the saved row, got {got}"
    );

    // ---- ACTION 2: edit (model change) overwrites in place ----------------
    let edited = client
        .tools_call(
            "agent_template_put",
            json!({
                "template_id": "reviewer",
                "name": "Code reviewer",
                "description": "Reviews the repo for correctness bugs.",
                "model": "codex",
                "directory": "C:\\code\\Synapse",
                "prompt": "Review the repo for correctness bugs."
            }),
        )
        .await?;
    let edited = structured(&edited)?;
    println!("readback=agent_template_put edge=edit state={edited}");
    ensure!(
        edited["created"] == json!(false),
        "an edit must not be 'created', got {edited}"
    );
    ensure!(
        edited["template"]["config_hash"].as_str() != Some(hash_v1.as_str()),
        "changing the model must change config_hash"
    );
    ensure!(
        edited["template"]["created_unix_ms"] == created["template"]["created_unix_ms"],
        "created_unix_ms must be preserved across an edit"
    );

    let got_after_edit = client
        .tools_call("agent_template_get", json!({"template_id": "reviewer"}))
        .await?;
    ensure!(
        structured(&got_after_edit)?["template"]["model"] == json!("codex"),
        "current must now reflect the edited model"
    );

    // ---- EDGE: a second template so list ordering is meaningful -----------
    client
        .tools_call(
            "agent_template_put",
            json!({
                "template_id": "idle-codex",
                "name": "Idle Codex",
                "model": "codex",
                "prompt": "Stand by."
            }),
        )
        .await?;
    let listed = client.tools_call("agent_template_list", json!({})).await?;
    let listed = structured(&listed)?;
    println!("readback=agent_template_list edge=two_templates state={listed}");
    ensure!(listed["count"] == json!(2), "two templates expected");
    let ids: Vec<&str> = listed["templates"]
        .as_array()
        .context("templates array")?
        .iter()
        .filter_map(|t| t["template_id"].as_str())
        .collect();
    ensure!(
        ids == vec!["idle-codex", "reviewer"],
        "list must be sorted by id, got {ids:?}"
    );

    // ---- ACTION 3: delete the reviewer -----------------------------------
    let deleted = client
        .tools_call("agent_template_delete", json!({"template_id": "reviewer"}))
        .await?;
    let deleted = structured(&deleted)?;
    println!("readback=agent_template_delete edge=delete state={deleted}");
    ensure!(
        deleted["deleted_row_key"] == json!("agent-template/v2/cur/reviewer"),
        "delete reports the row key, got {deleted}"
    );

    // ---- VERIFY: current get now fails ------------------------------------
    let gone = client
        .tools_call_error("agent_template_get", json!({"template_id": "reviewer"}))
        .await?;
    let gone = gone.to_string();
    println!("readback=agent_template_get edge=after_delete err={gone}");
    ensure!(
        gone.contains("AGENT_TEMPLATE_NOT_FOUND"),
        "deleted template must be gone, got {gone}"
    );

    // ---- PHYSICAL SOURCE-OF-TRUTH VERIFICATION ---------------------------
    // Shut the daemon down (releasing the RocksDB lock), then open the very
    // same on-disk database directly and scan the physical CF_KV rows.
    let status = client.shutdown().await?;
    ensure!(status.success(), "daemon must exit cleanly");

    let db = Db::open(&db_path, SCHEMA_VERSION).context("open daemon RocksDB directly")?;
    let rows = db
        .scan_cf_prefix(cf::CF_KV, b"agent-template/v2/")
        .context("scan CF_KV for template rows")?;
    let keys: Vec<String> = rows
        .iter()
        .map(|(k, _)| String::from_utf8_lossy(k).into_owned())
        .collect();
    println!("readback=cf_kv edge=physical_rows keys={keys:?}");

    ensure!(
        !keys.contains(&"agent-template/v2/cur/reviewer".to_owned()),
        "deleted reviewer must be physically absent, keys={keys:?}"
    );
    ensure!(
        keys.contains(&"agent-template/v2/cur/idle-codex".to_owned()),
        "idle-codex row must be present, keys={keys:?}"
    );
    // The simplified store is unversioned: there are no `/ver/` snapshot rows.
    ensure!(
        keys.iter().all(|k| k.contains("/cur/")),
        "v2 store keeps only current rows (no version snapshots), keys={keys:?}"
    );

    // Decode the idle row and prove its field-level contents on disk.
    let (_, idle_bytes) = rows
        .iter()
        .find(|(k, _)| k == b"agent-template/v2/cur/idle-codex")
        .context("idle-codex row missing on disk")?;
    let idle_row: Value = serde_json::from_slice(idle_bytes).context("decode idle row")?;
    println!("readback=cf_kv edge=decoded_idle row={idle_row}");
    ensure!(
        idle_row["template_id"] == json!("idle-codex")
            && idle_row["model"] == json!("codex")
            && idle_row["prompt"] == json!("Stand by.")
            && idle_row["schema_version"] == json!(2),
        "on-disk idle row must match what the tool reported, got {idle_row}"
    );

    Ok(())
}

#[tokio::test]
async fn must_fail_invalid_templates_and_spawns_from_deleted_or_conflicting_templates_are_refused()
-> anyhow::Result<()> {
    let db_dir = tempfile::Builder::new()
        .prefix("synapse-agent-templates-refusals")
        .tempdir()?;
    let db_path_str = db_path_under(db_dir.path()).to_string_lossy().into_owned();
    // Synthetic notepad foreground: act_spawn_agent's supported-use gate must not
    // mask the template-state error with A11Y_NO_FOREGROUND.
    let mut client = StdioMcpClient::launch_and_init_with_env(
        None,
        &[
            ("SYNAPSE_DB", db_path_str.as_str()),
            ("SYNAPSE_MCP_SYNTHETIC_FIXTURE", "notepad"),
            ("SYNAPSE_DEBUG_TOOLS", "1"),
        ],
    )
    .await?;

    // Empty prompt rejected loudly.
    let bad_prompt = client
        .tools_call_error(
            "agent_template_put",
            json!({"template_id": "bad", "name": "Bad", "model": "claude", "prompt": "   "}),
        )
        .await?;
    ensure!(
        bad_prompt["data"]["code"] == json!("TOOL_PARAMS_INVALID")
            && bad_prompt["message"]
                .as_str()
                .is_some_and(|message| message.contains("prompt")),
        "empty prompt must be rejected loudly, got {bad_prompt}"
    );

    // A model that is neither claude/codex nor registered.
    let bad_model = client
        .tools_call_error(
            "agent_template_put",
            json!({"template_id": "bad2", "name": "Bad2", "model": "no-such-model", "prompt": "go"}),
        )
        .await?;
    ensure!(
        bad_model["data"]["code"] == json!("MODEL_REGISTRY_NOT_FOUND"),
        "an unregistered model must be rejected, got {bad_model}"
    );
    // Neither refused template was stored.
    for refused in ["bad", "bad2"] {
        let missing = client
            .tools_call_error("agent_template_get", json!({"template_id": refused}))
            .await?;
        ensure!(
            missing["data"]["code"] == json!("AGENT_TEMPLATE_NOT_FOUND"),
            "refused template {refused} must not be stored, got {missing}"
        );
    }

    // Spawn from a deleted template errors and launches nothing.
    for (id, model) in [("reviewer", "claude"), ("idle-codex", "codex")] {
        client
            .tools_call(
                "agent_template_put",
                json!({"template_id": id, "name": id, "model": model, "prompt": "Stand by."}),
            )
            .await?;
    }
    client
        .tools_call("agent_template_delete", json!({"template_id": "reviewer"}))
        .await?;
    let spawn_deleted = client
        .tools_call_error("act_spawn_agent", json!({"template_id": "reviewer"}))
        .await?;
    ensure!(
        spawn_deleted["data"]["code"] == json!("AGENT_TEMPLATE_NOT_FOUND"),
        "spawning from a deleted template must fail, got {spawn_deleted}"
    );

    // template_id alongside a template-owned field is rejected.
    let spawn_conflict = client
        .tools_call_error(
            "act_spawn_agent",
            json!({"template_id": "idle-codex", "cli": "claude"}),
        )
        .await?;
    ensure!(
        spawn_conflict["data"]["code"] == json!("TOOL_PARAMS_INVALID")
            && spawn_conflict["message"].as_str().is_some_and(|message| {
                message.contains("[\"cli\"]") && message.contains("from template \"idle-codex\"")
            }),
        "passing a template-owned field alongside template_id must be rejected, got {spawn_conflict}"
    );

    let status = client.shutdown().await?;
    ensure!(status.success(), "daemon exit {status:?}");
    // Launches nothing: no spawn was ever journaled.
    let db = synapse_storage::Db::open(
        std::path::Path::new(&db_path_str),
        synapse_core::SCHEMA_VERSION,
    )?;
    let journal = db.scan_cf(synapse_storage::cf::CF_AGENT_EVENTS)?;
    ensure!(
        journal.is_empty(),
        "refused spawns must not journal any agent event, found {} rows",
        journal.len()
    );
    Ok(())
}
