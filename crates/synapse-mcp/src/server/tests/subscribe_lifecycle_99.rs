//! #99: actual schema-validating HTTP facade calls plus separate registry reads.
#[allow(dead_code)]
#[path = "../../../tests/support/mod.rs"]
mod support;
use serde_json::{Value, json};
use support::HttpDaemon;

async fn stats(daemon: &HttpDaemon, id: &str) -> anyhow::Result<reqwest::Response> {
    Ok(reqwest::Client::new()
        .get(daemon.url(&format!("/events/stats?subscription_id={id}")))
        .bearer_auth("synapse-tests-http-bearer")
        .send()
        .await?)
}

fn create() -> Value {
    json!({"operation":"events","events":{"kinds":["g7b-no-match"],"buffer_size":1}})
}

fn cancel(id: &str) -> Value {
    json!({"operation":"events_cancel","events_cancel":{"subscription_id":id}})
}

async fn daemon() -> anyhow::Result<HttpDaemon> {
    HttpDaemon::start(&[
        ("SYNAPSE_MCP_ALLOWED_PERMISSIONS", "READ_EVENTS"),
        ("SYNAPSE_HTTP_SSE_MANUAL", "1"),
    ])
    .await
}

#[tokio::test]
async fn must_pass_public_create_cancel_changes_separate_registry_readback_99() -> anyhow::Result<()>
{
    let daemon = daemon().await?;
    let mut owner = daemon.session("g7b-owner", &[]).await?;
    assert_eq!(
        stats(&daemon, "g7b-absent").await?.status(),
        reqwest::StatusCode::NOT_FOUND
    );
    let response = owner.tools_call("subscribe", create()).await?;
    let id = response["structuredContent"]["subscription_id"]
        .as_str()
        .expect("id")
        .to_owned();
    let before = stats(&daemon, &id).await?;
    assert_eq!(before.status(), reqwest::StatusCode::OK);
    let before: Value = before.json().await?;
    assert_eq!(before["subscription_id"], id);
    assert_eq!(before["ring_len"], 0);
    let response = owner.tools_call("subscribe", cancel(&id)).await?;
    assert_eq!(response["structuredContent"]["cancelled"], true);
    assert_eq!(
        stats(&daemon, &id).await?.status(),
        reqwest::StatusCode::NOT_FOUND
    );
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn must_fail_foreign_unknown_double_cancel_preserve_registry_contract_99()
-> anyhow::Result<()> {
    let daemon = daemon().await?;
    let mut owner = daemon.session("g7b-owner", &[]).await?;
    let mut foreign = daemon.session("g7b-foreign", &[]).await?;
    assert_ne!(owner.session_id(), foreign.session_id());
    let response = owner.tools_call("subscribe", create()).await?;
    let id = response["structuredContent"]["subscription_id"]
        .as_str()
        .expect("id")
        .to_owned();
    let before: Value = stats(&daemon, &id)
        .await?
        .error_for_status()?
        .json()
        .await?;
    let error = foreign.tools_call_error("subscribe", cancel(&id)).await?;
    assert_eq!(error["data"]["code"], "SUBSCRIPTION_NOT_FOUND");
    let error = owner
        .tools_call_error("subscribe", cancel("g7b-absent"))
        .await?;
    assert_eq!(error["data"]["code"], "SUBSCRIPTION_NOT_FOUND");
    let after: Value = stats(&daemon, &id)
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(
        after, before,
        "foreign/unknown refusal changed physical registry state"
    );
    owner.tools_call("subscribe", cancel(&id)).await?;
    assert_eq!(
        stats(&daemon, &id).await?.status(),
        reqwest::StatusCode::NOT_FOUND
    );
    let error = owner.tools_call_error("subscribe", cancel(&id)).await?;
    assert_eq!(error["data"]["code"], "SUBSCRIPTION_NOT_FOUND");
    assert_eq!(
        stats(&daemon, &id).await?.status(),
        reqwest::StatusCode::NOT_FOUND
    );
    daemon.stop().await?;
    Ok(())
}
