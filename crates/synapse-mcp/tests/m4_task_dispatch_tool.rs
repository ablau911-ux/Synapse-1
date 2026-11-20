use std::path::{Path, PathBuf};

use anyhow::{Context, ensure};
use serde_json::{Value, json};
use synapse_core::SCHEMA_VERSION;
use synapse_storage::{Db, cf};
use synapse_test_utils::stdio_mcp_client::StdioMcpClient;

fn structured(result: &Value) -> anyhow::Result<&Value> {
    result
        .get("structuredContent")
        .with_context(|| format!("missing structuredContent in {result}"))
}

fn db_path_under(dir: &Path) -> PathBuf {
    dir.join("db")
}

#[tokio::test]
async fn must_pass_task_dispatch_once_on_empty_board_dispatches_nothing() -> anyhow::Result<()> {
    let db_dir = tempfile::Builder::new()
        .prefix("synapse-task-dispatch-empty")
        .tempdir()?;
    let db_path = db_path_under(db_dir.path());
    let db_path_str = db_path.to_string_lossy().into_owned();
    let mut client =
        StdioMcpClient::launch_and_init_with_env(None, &[("SYNAPSE_DB", db_path_str.as_str())])
            .await?;

    let empty = client
        .tools_call("task_dispatch_once", json!({"concurrency_cap": 4}))
        .await?;
    let empty = structured(&empty)?;
    ensure!(
        empty["decision"] == json!("empty")
            && empty["task"].is_null()
            && empty["spawn"].is_null()
            && empty["in_flight"] == json!(0)
            && empty["concurrency_cap"] == json!(4),
        "empty board must dispatch nothing, got {empty}"
    );

    let status = client.shutdown().await?;
    ensure!(status.success(), "daemon must exit cleanly");
    let db = Db::open(&db_path, SCHEMA_VERSION).context("open daemon RocksDB directly")?;
    ensure!(
        db.scan_cf_prefix(cf::CF_KV, b"agent-task/v1/task/")?
            .is_empty(),
        "an empty-board dispatch must not create task rows"
    );
    Ok(())
}

#[tokio::test]
async fn must_fail_task_dispatch_once_spawn_failure_preserves_rows() -> anyhow::Result<()> {
    let db_dir = tempfile::Builder::new()
        .prefix("synapse-task-dispatch-regression")
        .tempdir()?;
    let db_path = db_path_under(db_dir.path());
    let db_path_str = db_path.to_string_lossy().into_owned();

    let mut client =
        StdioMcpClient::launch_and_init_with_env(None, &[("SYNAPSE_DB", db_path_str.as_str())])
            .await?;

    let created = client
        .tools_call(
            "task_create",
            json!({
                "task_id": "ghost-task",
                "title": "references a missing template",
                "priority": 1,
                "template_id": "ghost-template",
                "template_params": {"repo": "Synapse"}
            }),
        )
        .await?;
    ensure!(
        structured(&created)?["task"]["state"] == json!("todo"),
        "task must be created todo"
    );

    let before = client
        .tools_call("task_get", json!({"task_id": "ghost-task"}))
        .await?;
    let before = structured(&before)?;
    ensure!(
        before["task"]["state"] == json!("todo")
            && before["task"]["attempts"].as_array().map(Vec::len) == Some(0),
        "ghost-task must start todo with no attempts, got {before}"
    );

    let dispatch_err = client
        .tools_call_error("task_dispatch_once", json!({"concurrency_cap": 4}))
        .await?;
    ensure!(
        dispatch_err["data"]["code"] == json!("AGENT_TEMPLATE_NOT_FOUND"),
        "dispatch of a missing-template task must surface the template error, got {dispatch_err}"
    );

    let after = client
        .tools_call("task_get", json!({"task_id": "ghost-task"}))
        .await?;
    let after = structured(&after)?;
    ensure!(
        after["task"]["state"] == json!("todo"),
        "a failed spawn must leave the task todo, got {after}"
    );
    let attempts = after["task"]["attempts"]
        .as_array()
        .context("attempts must be an array")?;
    ensure!(
        attempts.len() == 1,
        "exactly one failed attempt, got {after}"
    );
    let attempt = &attempts[0];
    ensure!(
        attempt["attempt_id"] == json!(1)
            && attempt["outcome"] == json!("failed")
            && attempt["session_id"] == json!("")
            && attempt["spawn_id"].is_null()
            && attempt["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("dispatch spawn failed")
                    && reason.contains("AGENT_TEMPLATE_NOT_FOUND")),
        "the recorded attempt must carry the spawn failure reason, got {attempt}"
    );

    let retry_err = client
        .tools_call_error("task_dispatch_once", json!({"concurrency_cap": 4}))
        .await?;
    ensure!(
        retry_err["data"]["code"] == json!("AGENT_TEMPLATE_NOT_FOUND"),
        "second dispatch must select the same task and fail again, got {retry_err}"
    );
    let after_retry = client
        .tools_call("task_get", json!({"task_id": "ghost-task"}))
        .await?;
    let after_retry = structured(&after_retry)?;
    let attempts_retry = after_retry["task"]["attempts"]
        .as_array()
        .context("attempts must be an array")?;
    ensure!(
        after_retry["task"]["state"] == json!("todo")
            && attempts_retry.len() == 2
            && attempts_retry[1]["attempt_id"] == json!(2)
            && attempts_retry[1]["outcome"] == json!("failed"),
        "re-dispatch must append a second failed attempt and leave the task todo, got {after_retry}"
    );

    let status = client.shutdown().await?;
    ensure!(status.success(), "daemon must exit cleanly");

    let db = Db::open(&db_path, SCHEMA_VERSION).context("open daemon RocksDB directly")?;
    let rows = db
        .scan_cf_prefix(cf::CF_KV, b"agent-task/v1/task/")
        .context("scan CF_KV for task rows")?;
    ensure!(
        rows.len() == 1,
        "exactly one task row on disk, got {}",
        rows.len()
    );
    let (key, value) = &rows[0];
    let key = String::from_utf8_lossy(key).into_owned();
    let task: Value = serde_json::from_slice(value).context("decode task row")?;
    ensure!(
        key == "agent-task/v1/task/ghost-task"
            && task["task_id"] == json!("ghost-task")
            && task["state"] == json!("todo")
            && task["attempts"].as_array().map(Vec::len) == Some(2)
            && task["attempts"][0]["outcome"] == json!("failed")
            && task["attempts"][1]["outcome"] == json!("failed")
            && task["attempts"][1]["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("AGENT_TEMPLATE_NOT_FOUND")),
        "on-disk row must stay todo with two recorded failed attempts, got {task}"
    );

    Ok(())
}

/// `task_dispatch_once` selects the documented winner: the strict top priority
/// tier (1 = highest) and, inside it, the oldest enqueued task. The spawn of a
/// missing template fails, so the recorded attempt is the physical readback of
/// which task was selected; every other task stays untouched.
#[tokio::test]
async fn must_pass_task_dispatch_once_selects_top_priority_oldest_task() -> anyhow::Result<()> {
    let db_dir = tempfile::Builder::new()
        .prefix("synapse-task-dispatch-selection")
        .tempdir()?;
    let db_path = db_path_under(db_dir.path());
    let db_path_str = db_path.to_string_lossy().into_owned();

    let mut client =
        StdioMcpClient::launch_and_init_with_env(None, &[("SYNAPSE_DB", db_path_str.as_str())])
            .await?;

    for (task_id, priority) in [("low-first", 5), ("high-old", 1), ("high-new", 1)] {
        let created = client
            .tools_call(
                "task_create",
                json!({
                    "task_id": task_id,
                    "title": format!("selection probe {task_id}"),
                    "priority": priority,
                    "template_id": "ghost-template",
                }),
            )
            .await?;
        ensure!(
            structured(&created)?["task"]["state"] == json!("todo"),
            "{task_id} must be created todo"
        );
    }

    let dispatch_err = client
        .tools_call_error("task_dispatch_once", json!({"concurrency_cap": 1}))
        .await?;
    ensure!(
        dispatch_err["data"]["code"] == json!("AGENT_TEMPLATE_NOT_FOUND"),
        "dispatch must reach the spawn of the selected task, got {dispatch_err}"
    );

    for (task_id, expected_attempts) in [("high-old", 1), ("high-new", 0), ("low-first", 0)] {
        let got = client
            .tools_call("task_get", json!({"task_id": task_id}))
            .await?;
        let got = structured(&got)?;
        ensure!(
            got["task"]["state"] == json!("todo")
                && got["task"]["attempts"].as_array().map(Vec::len) == Some(expected_attempts),
            "{task_id} must hold {expected_attempts} attempt(s) after one dispatch, got {got}"
        );
    }

    let status = client.shutdown().await?;
    ensure!(status.success(), "daemon must exit cleanly");

    let db = Db::open(&db_path, SCHEMA_VERSION).context("open daemon RocksDB directly")?;
    let rows = db
        .scan_cf_prefix(cf::CF_KV, b"agent-task/v1/task/")
        .context("scan CF_KV for task rows")?;
    ensure!(
        rows.len() == 3,
        "three task rows on disk, got {}",
        rows.len()
    );
    for (key, value) in &rows {
        let key = String::from_utf8_lossy(key).into_owned();
        let task: Value = serde_json::from_slice(value).context("decode task row")?;
        let expected = usize::from(key == "agent-task/v1/task/high-old");
        ensure!(
            task["attempts"].as_array().map(Vec::len) == Some(expected),
            "on-disk {key} must hold {expected} attempt(s), got {task}"
        );
    }
    Ok(())
}
