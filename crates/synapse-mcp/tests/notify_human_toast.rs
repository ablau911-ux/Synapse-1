//! `notify_human` refuses invalid parameters with precise errors.
#![cfg(windows)]

use anyhow::ensure;
use serde_json::json;
use synapse_test_utils::stdio_mcp_client::StdioMcpClient;

#[tokio::test]
async fn must_fail_notify_human_rejects_invalid_params_with_precise_errors() -> anyhow::Result<()> {
    let mut client = StdioMcpClient::launch_and_init().await?;

    // Every edge is refused with TOOL_PARAMS_INVALID and a message naming the
    // exact rule it broke.
    let edges = [
        (
            "empty-title",
            json!({"title": "   ", "body": "b", "kind": "info"}),
            "title must not be empty",
        ),
        (
            "oversized-body",
            json!({"title": "t", "body": "x".repeat(2001), "kind": "info"}),
            "max 2000",
        ),
        (
            "control-char",
            json!({"title": "bel\u{0007}l", "body": "b", "kind": "info"}),
            "U+0007",
        ),
        (
            "bad-kind",
            json!({"title": "t", "body": "b", "kind": "shout"}),
            "unknown variant `shout`",
        ),
        (
            "empty-dedupe",
            json!({"title": "t", "body": "b", "kind": "info", "dedupe_key": " "}),
            "dedupe_key must not be empty",
        ),
    ];
    for (edge, args, fragment) in edges {
        let error = client.tools_call_error("notify_human", args).await?;
        println!("readback=edge {edge} error={error}");
        ensure!(
            error["data"]["code"] == json!("TOOL_PARAMS_INVALID"),
            "{edge}: must be refused as TOOL_PARAMS_INVALID: {error}"
        );
        ensure!(
            error["message"]
                .as_str()
                .is_some_and(|message| message.contains(fragment)),
            "{edge}: must be rejected precisely (`{fragment}`): {error}"
        );
    }

    let status = client.shutdown().await?;
    assert!(status.success());
    Ok(())
}

/// A valid quiet toast (`suppress_popup`, no banner) is raised and read back from
/// Action Center history; a repeat with the same `dedupe_key` while the first is
/// still present is suppressed (`deduped=true, shown=false`) under the same tag.
#[tokio::test]
async fn must_pass_notify_human_raises_verified_toast_and_dedupes_repeat() -> anyhow::Result<()> {
    let mut client = StdioMcpClient::launch_and_init().await?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    let dedupe_key = format!("synapse-test-notify-must-pass-{nonce}");
    let args = json!({
        "title": "Synapse test toast",
        "body": "must_pass_notify_human_raises_verified_toast_and_dedupes_repeat",
        "kind": "info",
        "dedupe_key": dedupe_key,
        "suppress_popup": true,
    });

    let first = client.tools_call("notify_human", args.clone()).await?;
    println!("readback=notify_human first={first}");
    let first = first
        .get("structuredContent")
        .ok_or_else(|| anyhow::anyhow!("notify_human structuredContent missing: {first}"))?
        .clone();
    let tag = first["tag"].as_str().unwrap_or_default().to_owned();
    ensure!(
        first["shown"] == json!(true)
            && first["deduped"] == json!(false)
            && first["verified_in_history"] == json!(true)
            && first["history_count"]
                .as_u64()
                .is_some_and(|count| count >= 1)
            && tag.starts_with("dk-")
            && tag.len() == 35
            && matches!(
                first["notification_setting"].as_str(),
                Some("enabled" | "unavailable_first_use")
            ),
        "valid toast must be shown and verified in Action Center history: {first}"
    );

    let repeat = client.tools_call("notify_human", args).await?;
    println!("readback=notify_human repeat={repeat}");
    let repeat = repeat
        .get("structuredContent")
        .ok_or_else(|| anyhow::anyhow!("notify_human structuredContent missing: {repeat}"))?
        .clone();
    ensure!(
        repeat["shown"] == json!(false)
            && repeat["deduped"] == json!(true)
            && repeat["tag"] == json!(tag),
        "repeat with the same dedupe_key must be suppressed under the same tag: {repeat}"
    );

    let status = client.shutdown().await?;
    assert!(status.success());
    Ok(())
}
