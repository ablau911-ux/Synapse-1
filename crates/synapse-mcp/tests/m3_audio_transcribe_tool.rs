use anyhow::Context;
use serde_json::{Value, json};
use synapse_test_utils::stdio_mcp_client::StdioMcpClient;
use tempfile::TempDir;

#[tokio::test]
async fn must_pass_audio_transcribe_schema_defaults_and_silence() -> anyhow::Result<()> {
    let logs = TempDir::new()?;
    let mut client = launch_audio_client(&logs).await?;

    let tools = client.tools_list().await?;
    let tools = tools
        .get("tools")
        .and_then(Value::as_array)
        .context("tools array missing")?;
    let audio_transcribe_tool = tools
        .iter()
        .find(|tool| tool["name"] == "audio_transcribe")
        .context("audio_transcribe tool missing")?;
    assert_audio_transcribe_schema(audio_transcribe_tool);

    let silence = structured(
        &client
            .tools_call("audio_transcribe", json!({"seconds": 5}))
            .await?,
    )?;
    assert_eq!(silence["text"], "");
    assert_eq!(silence["confidence"], 0.0);
    assert_eq!(silence["latency_ms"], 0);
    assert_eq!(silence["model_id"], "whisper_tiny_int8");

    let short_silence = structured(
        &client
            .tools_call("audio_transcribe", json!({"seconds": 0.1}))
            .await?,
    )?;
    assert_eq!(short_silence["text"], "");
    assert_eq!(short_silence["confidence"], 0.0);

    let status = client.shutdown().await?;
    assert!(status.success());
    Ok(())
}

#[tokio::test]
async fn must_fail_audio_transcribe_unknown_language_and_over_cap_seconds_are_refused()
-> anyhow::Result<()> {
    let logs = TempDir::new()?;
    let mut client = launch_audio_client(&logs).await?;

    // Only "en" is supported; a silent window validates the language too.
    for (args, fragment) in [
        (
            json!({"language": "xx"}),
            r#"audio_transcribe language must be "en"; got "xx""#,
        ),
        (
            json!({"language": "es", "seconds": 0.1}),
            r#"audio_transcribe language must be "en"; got "es""#,
        ),
        (
            json!({"seconds": 31}),
            "audio seconds must be between 0 and 30",
        ),
        (
            json!({"seconds": -1}),
            "audio seconds must be between 0 and 30",
        ),
    ] {
        let refused = client
            .tools_call_error("audio_transcribe", args.clone())
            .await?;
        assert_eq!(
            refused["data"]["code"], "TOOL_PARAMS_INVALID",
            "{args}: {refused}"
        );
        let message = refused["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(fragment),
            "{args}: refusal must name the offending field ({fragment}): {refused}"
        );
    }

    let status = client.shutdown().await?;
    assert!(status.success());
    Ok(())
}

#[tokio::test]
async fn must_pass_audio_transcribe_silence_accepts_explicit_language_and_cap() -> anyhow::Result<()>
{
    let logs = TempDir::new()?;
    let mut client = launch_audio_client(&logs).await?;

    for args in [
        json!({"language": "en", "seconds": 0.1}),
        json!({"seconds": 30}),
    ] {
        let silence = structured(&client.tools_call("audio_transcribe", args.clone()).await?)?;
        assert_eq!(silence["text"], "", "{args}");
        assert_eq!(silence["confidence"], 0.0, "{args}");
        assert_eq!(silence["model_id"], "whisper_tiny_int8", "{args}");
    }

    let status = client.shutdown().await?;
    assert!(status.success());
    Ok(())
}

async fn launch_audio_client(logs: &TempDir) -> anyhow::Result<StdioMcpClient> {
    StdioMcpClient::launch_and_init_with_env(
        Some(logs.path()),
        &[
            ("SYNAPSE_ENABLE_AUDIO", "true"),
            ("SYNAPSE_AUDIO_LOOPBACK", "0"),
        ],
    )
    .await
}

fn structured(response: &Value) -> anyhow::Result<Value> {
    if let Some(value) = response.get("structuredContent") {
        return Ok(value.clone());
    }

    let text = response
        .get("content")
        .and_then(Value::as_array)
        .and_then(|content| content.first())
        .and_then(|content| content.get("text"))
        .and_then(Value::as_str)
        .context("structured content missing")?;
    serde_json::from_str(text).context("parse text content")
}

fn assert_audio_transcribe_schema(tool: &Value) {
    let shape = json!({
        "name": tool.get("name").cloned().unwrap_or(Value::Null),
        "inputSchema": tool.get("inputSchema").cloned().unwrap_or(Value::Null),
        "outputSchema": tool.get("outputSchema").cloned().unwrap_or(Value::Null),
    });
    assert_eq!(shape["inputSchema"]["additionalProperties"], false);
    assert_eq!(
        shape["inputSchema"]["properties"]["seconds"]["default"],
        5.0
    );
    assert_eq!(shape["inputSchema"]["properties"]["seconds"]["maximum"], 30);
    assert_eq!(
        shape["inputSchema"]["properties"]["language"]["default"],
        "en"
    );
    assert_eq!(
        shape["outputSchema"]["required"],
        json!(["text", "confidence", "latency_ms", "model_id"])
    );
    insta::assert_json_snapshot!("m3_audio_transcribe_tool", shape);
}
