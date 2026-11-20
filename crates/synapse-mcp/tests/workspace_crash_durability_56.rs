//! Real public acknowledgement followed by hard process kill and DB reopen.
//! Supports process-crash recovery; does not simulate OS/power-loss fsync.
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[allow(dead_code)]
mod support;
use serde_json::{Value, json};
use support::HttpDaemon;
use synapse_storage::{Db, cf};

#[tokio::test]
async fn must_pass_acknowledged_workspace_row_survives_immediate_hard_kill_56() -> anyhow::Result<()>
{
    let daemon = HttpDaemon::start(&[]).await?;
    let db_path = daemon.db_path().to_path_buf();
    let mut session = daemon.session("g7b-crash", &[]).await?;
    let before = session
        .tools_call(
            "workspace",
            json!({"operation":"exists",
        "exists":{"run_id":"g7b-crash","key":"durable"}}),
        )
        .await?;
    assert_eq!(before["structuredContent"]["exists"]["exists"], false);
    let response = session
        .tools_call(
            "workspace",
            json!({"operation":"put","put":{
                "run_id":"g7b-crash","key":"durable","value":{"exact":56},"ttl_ms":600000
            }}),
        )
        .await?;
    let put = &response["structuredContent"]["put"];
    assert_eq!(put["version"], 1);
    let row_key = put["row_key"]
        .as_str()
        .expect("physical row key")
        .to_owned();
    // No extra round trip or graceful shutdown between ACK and kill.
    let _home = daemon.stop().await?;
    let db = Db::open(&db_path, synapse_core::SCHEMA_VERSION)?;
    let bytes = db
        .get_cf_exact(cf::CF_KV, row_key.as_bytes())?
        .expect("acknowledged row");
    let row: Value = synapse_storage::decode_json(&bytes)?;
    assert_eq!(row["run_id"], "g7b-crash");
    assert_eq!(row["key"], "durable");
    assert_eq!(row["value"], json!({"exact":56}));
    assert_eq!(row["version"], 1);
    Ok(())
}

#[tokio::test]
async fn must_fail_invalid_workspace_put_leaves_no_recovered_workspace_row_56() -> anyhow::Result<()>
{
    let daemon = HttpDaemon::start(&[]).await?;
    let db_path = daemon.db_path().to_path_buf();
    let mut session = daemon.session("g7b-crash-invalid", &[]).await?;
    let list = json!({"operation":"list","list":{"run_id":"g7b-crash-invalid"}});
    let before = session.tools_call("workspace", list.clone()).await?;
    let error = session
        .tools_call_error(
            "workspace",
            json!({"operation":"put","put":{
                "run_id":"g7b-crash-invalid","key":"","value":{"exact":56},"ttl_ms":600000
            }}),
        )
        .await?;
    assert_eq!(error["data"]["code"], "TOOL_PARAMS_INVALID");
    let after = session.tools_call("workspace", list).await?;
    assert_eq!(workspace_list_state(after), workspace_list_state(before));
    let _home = daemon.stop().await?;
    let db = Db::open(&db_path, synapse_core::SCHEMA_VERSION)?;
    let rows: Vec<Value> = db
        .scan_cf(cf::CF_KV)?
        .into_iter()
        .filter_map(|(_, bytes)| synapse_storage::decode_json::<Value>(&bytes).ok())
        .filter(|row| row["run_id"] == "g7b-crash-invalid")
        .collect();
    assert_eq!(rows, Vec::<Value>::new());
    Ok(())
}

fn workspace_list_state(mut response: Value) -> Value {
    let list = response["structuredContent"]["list"]
        .as_object_mut()
        .expect("workspace list object");
    assert!(
        list.remove("now_unix_ms").is_some(),
        "list observation timestamp"
    );
    response["structuredContent"]["list"].clone()
}
