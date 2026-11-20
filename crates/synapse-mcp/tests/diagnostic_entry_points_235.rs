//! #235: public, schema-validating calls; no helper-only diagnostic assertions.
#![allow(clippy::unwrap_used, clippy::expect_used)]
#[allow(dead_code)]
mod support;

use serde_json::json;
use support::HttpDaemon;

#[tokio::test]
async fn must_fail_agent_schema_cannot_call_shipped_task_queue_unimplemented() -> anyhow::Result<()>
{
    let daemon = HttpDaemon::start(&[]).await?;
    let mut session = daemon.session("g8-235-schema", &[]).await?;
    let listed = session.tools_list().await?;
    let agent = listed["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .find(|tool| tool["name"] == "agent")
        .expect("agent facade");
    let schema = serde_json::to_string(&agent["outputSchema"])?;
    assert!(
        !schema.contains("Always null until #910"),
        "#235: task queue ships; unresolved link must not be described as unshipped queue"
    );
    let after = session.tools_list().await?;
    assert_eq!(after, listed, "schema reads cannot mutate registry");
    drop(daemon.stop().await?);
    Ok(())
}

#[tokio::test]
async fn must_pass_agent_query_names_shipped_task_source_without_fabricated_link()
-> anyhow::Result<()> {
    let daemon = HttpDaemon::start(&[]).await?;
    let mut session = daemon.session("g8-235-task", &[]).await?;
    let id = session.session_id().to_owned();
    let result = session
        .tools_call(
            "agent",
            json!({
                "operation": "query", "query": {"session_id": id}
            }),
        )
        .await?;
    let query = &result["structuredContent"]["query"];
    assert_eq!(query["session_id"], id);
    assert_eq!(query["task"], serde_json::Value::Null);
    assert_eq!(
        query["sources"]["task"],
        "not linked by agent query (reported null, never fabricated); read the agent's task with task operation=list/get (CF_KV agent-task/v1 rows, #910)"
    );
    drop(daemon.stop().await?);
    Ok(())
}

#[tokio::test]
async fn must_fail_missing_video_at_public_entry_names_path_without_creating_file()
-> anyhow::Result<()> {
    let daemon = HttpDaemon::start(&[]).await?;
    let path = daemon.home().join("absent-235.mp4");
    assert!(!path.try_exists()?, "precondition: file absent");
    let mut session = daemon.session("g8-235-video", &[]).await?;
    for operation in ["inspect", "analyze", "motion", "text_timeline"] {
        let error = session
            .tools_call_error(
                "videoanalyze",
                json!({
                    "operation": operation, "path": path.to_string_lossy()
                }),
            )
            .await?;
        assert_eq!(error["data"]["code"], "TOOL_PARAMS_INVALID");
        assert_eq!(
            error["message"],
            format!("videoanalyze file not found: {}", path.display())
        );
        assert!(!path.try_exists()?, "refusal must not create input");
    }
    drop(daemon.stop().await?);
    Ok(())
}
