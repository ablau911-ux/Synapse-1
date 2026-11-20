//! Real ServerHandler::call_tool -> facade -> bridge queue -> typed wire output.
//! Node executes current extension page serializers against synthetic DOM.
//! No substitute sanitizer or canned assertion output.
use std::{path::PathBuf, process::Command};

use rmcp::{ServerHandler, model::CallToolResult};
use serde_json::{Value, json};

use crate::chrome_debugger_bridge::dispatch_stage_189_tests::password_fixture_244::with_reply;
use crate::server::{SessionTarget, g4_unit_support as fx};

const SESSION: &str = "password-handler-244-r2";
const HWND: i64 = 0x244_244;
const TARGET: &str = "chrome-tab:244";
const SECRET: &str = "fixture-password-244";

#[path = "browser_password_ax_244_r2.rs"]
mod browser_password_ax_244_r2;

fn page_reply(route: &str, options: &Value) -> anyhow::Result<Value> {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../extensions/synapse-chrome-debugger/tests/password_fixture_244.mjs");
    let output = Command::new("node")
        .arg(script)
        .arg(route)
        .arg(options.to_string())
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "page fixture failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut reply: Value = serde_json::from_slice(&output.stdout)?;
    reply["target_id"] = json!(TARGET);
    reply["tab_id"] = json!(244);
    reply["target_candidate_count"] = json!(1);
    reply["target_selection_reason"] = json!("explicit_owned_target");
    reply["readback_backend"] = json!("chrome.scripting.executeScript");
    Ok(reply)
}

async fn public_read(route: &str, options: Value) -> anyhow::Result<CallToolResult> {
    let (_root, service) = fx::isolated_service();
    fx::seed_live_session(&service, SESSION);
    service
        .session_targets_ref()
        .lock()
        .map_err(|_| anyhow::anyhow!("target lock"))?
        .insert(
            SESSION.to_owned(),
            SessionTarget::Cdp {
                window_hwnd: HWND,
                cdp_target_id: TARGET.to_owned(),
            },
        );
    let (operation, spec, command) = match route {
        "inspect" => (
            "inspect",
            json!({"element_id":"chrome-tab:244:frame:0:path:0.0.0"}),
            "inspectElement",
        ),
        "content" => ("content", json!({"cdp_target_id":TARGET}), "pageContent"),
        _ => anyhow::bail!("unsupported public fixture route"),
    };
    let args = json!({"operation":operation, (operation):spec});
    let context = crate::test_support::request_context_for_session(SESSION).await;
    let request = serde_json::from_value(json!({"name":"browser_dom", "arguments":args}))?;
    let result = with_reply(
        command,
        page_reply(route, &options)?,
        ServerHandler::call_tool(&service, request, context),
    )
    .await?
    .map_err(|error| anyhow::anyhow!("public handler error: {error:?}"))?;
    anyhow::ensure!(
        result.is_error != Some(true),
        "handler returned tool error: {result:?}"
    );
    Ok(result)
}

fn structured(result: &CallToolResult) -> anyhow::Result<&Value> {
    result
        .structured_content
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("missing structured content"))
}

#[tokio::test]
async fn must_pass_dom_handler_preserves_nonpassword_attributes_and_live_value_244()
-> anyhow::Result<()> {
    let result = public_read("inspect", json!({"type":"text", "secret":"ordinary-244"})).await?;
    let element = &structured(&result)?["inspect"]["element"];
    assert_eq!(element["input_value"], "ordinary-244");
    assert_eq!(element["attributes"]["value"], "ordinary-244");
    assert_eq!(element["attributes"]["id"], "plainfield");
    Ok(())
}

#[tokio::test]
async fn must_fail_dom_inspect_handler_never_returns_password_in_any_wire_channel_244()
-> anyhow::Result<()> {
    let result = public_read("inspect", json!({})).await?;
    let wire = serde_json::to_string(&result)?;
    assert!(
        !wire.contains(SECRET),
        "structuredContent or text content leaked password: {wire}"
    );
    let element = &structured(&result)?["inspect"]["element"];
    assert_eq!(element["is_password"], true);
    assert_eq!(element["input_value"], "[REDACTED]");
    assert_eq!(element["attributes"]["value"], "[REDACTED]");
    assert_eq!(element["attributes"]["name"], "login");
    Ok(())
}

#[tokio::test]
async fn must_fail_dom_content_handler_redacts_password_descendants_before_serialization_244()
-> anyhow::Result<()> {
    let result = public_read("content", json!({})).await?;
    let wire = serde_json::to_string(&result)?;
    assert!(
        !wire.contains(SECRET),
        "HTML/text wire content leaked password: {wire}"
    );
    let html = structured(&result)?["content"]["html"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing html"))?;
    assert!(html.contains("[REDACTED]"));
    assert!(html.contains("keep-ordinary-244"));
    Ok(())
}
