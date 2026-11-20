//! #137 raid: when the stdio suites were repaired, the `subscribe` READ_EVENTS permission
//! assertion was dropped and the owned create/cancel lifecycle was left untested
//! ("cannot run over stdio"). These tests restore that coverage through the real
//! Streamable HTTP daemon, where every MCP session is a trusted subscription owner.
//! Client parity: `support::HttpMcpSession` validates tools/list, arguments and outputs.

#[allow(dead_code)]
mod support;

use serde_json::{Value, json};
use support::HttpDaemon;
use synapse_core::error_codes;

const SOURCE_OF_TRUTH: &str = "SseState subscription registry + MCP-session ownership map";

fn structured(result: &Value) -> &Value {
    &result["structuredContent"]
}

fn events_args() -> Value {
    json!({"operation": "events", "events": {"kinds": []}})
}

fn cancel_args(id: &str) -> Value {
    json!({"operation": "events_cancel", "events_cancel": {"subscription_id": id}})
}

#[tokio::test]
async fn must_pass_owned_session_creates_then_cancels_its_subscription_137() -> anyhow::Result<()> {
    let daemon = HttpDaemon::start(&[("SYNAPSE_MCP_ALLOWED_PERMISSIONS", "READ_EVENTS")]).await?;
    let mut owner = daemon.session("raid-137-owner", &[]).await?;

    let created = owner.tools_call("subscribe", events_args()).await?;
    let created = structured(&created);
    assert_eq!(created["source_of_truth"], SOURCE_OF_TRUTH, "{created}");
    let id = created["subscription_id"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(
        !id.is_empty(),
        "subscribe must return a subscription_id: {created}"
    );

    let cancelled = owner.tools_call("subscribe", cancel_args(&id)).await?;
    assert_eq!(
        structured(&cancelled)["cancelled"],
        json!(true),
        "{cancelled}"
    );
    assert_eq!(structured(&cancelled)["reason"], json!("ok"), "{cancelled}");

    // Readback: the registry no longer holds the id, so a second cancel is not found.
    let again = owner
        .tools_call_error("subscribe", cancel_args(&id))
        .await?;
    assert_eq!(
        again["data"]["code"],
        error_codes::SUBSCRIPTION_NOT_FOUND,
        "{again}"
    );

    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn must_fail_other_session_cannot_cancel_a_subscription_it_does_not_own_137()
-> anyhow::Result<()> {
    let daemon = HttpDaemon::start(&[("SYNAPSE_MCP_ALLOWED_PERMISSIONS", "READ_EVENTS")]).await?;
    let mut owner = daemon.session("raid-137-owner", &[]).await?;
    let mut intruder = daemon.session("raid-137-intruder", &[]).await?;

    let created = owner.tools_call("subscribe", events_args()).await?;
    let id = structured(&created)["subscription_id"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(!id.is_empty(), "{created}");

    let refused = intruder
        .tools_call_error("subscribe", cancel_args(&id))
        .await?;
    assert_eq!(
        refused["data"]["code"],
        error_codes::SUBSCRIPTION_NOT_FOUND,
        "{refused}"
    );

    // Readback: the owner's subscription survived the foreign cancel attempt.
    let cancelled = owner.tools_call("subscribe", cancel_args(&id)).await?;
    assert_eq!(
        structured(&cancelled)["cancelled"],
        json!(true),
        "{cancelled}"
    );

    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn must_fail_subscribe_without_read_events_is_refused_naming_the_permission_137()
-> anyhow::Result<()> {
    let daemon = HttpDaemon::start(&[("SYNAPSE_MCP_ALLOWED_PERMISSIONS", "READ_STORAGE")]).await?;
    let mut session = daemon.session("raid-137-ungranted", &[]).await?;

    let refused = session.tools_call_error("subscribe", events_args()).await?;
    assert_eq!(
        refused["data"]["code"],
        error_codes::SAFETY_PERMISSION_DENIED,
        "{refused}"
    );
    assert_eq!(
        refused["data"]["missing_permission"], "READ_EVENTS",
        "{refused}"
    );

    let cancel = session
        .tools_call_error("subscribe", cancel_args("sub-does-not-exist"))
        .await?;
    assert_eq!(
        cancel["data"]["code"],
        error_codes::SAFETY_PERMISSION_DENIED,
        "{cancel}"
    );
    assert_eq!(
        cancel["data"]["missing_permission"], "READ_EVENTS",
        "{cancel}"
    );

    daemon.stop().await?;
    Ok(())
}
