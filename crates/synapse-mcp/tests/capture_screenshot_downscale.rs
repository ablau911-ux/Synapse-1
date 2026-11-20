//! Reality FSV for #1336 — screenshot pixel-budget downscaling.
//!
//! Drives the real `synapse-mcp` binary over the stdio MCP protocol through the
//! public `screenshot` facade (operation=capture), captures a real screen
//! region with a downscale budget, and then INDEPENDENTLY decodes the written
//! PNG file (the source of truth on disk) to prove the pixel dimensions
//! actually changed. Return values alone are not trusted: every assertion is
//! cross-checked against the decoded image bytes.
//!
//! Precondition: an unlocked interactive desktop of at least 1000x1000 (real
//! screen capture). A locked screen fails these tests; that is reported, not
//! faked.
//!
//! Synthetic, deterministic inputs (X+X=Y): a `1000x500` region under a 400 px
//! long-edge budget MUST land at exactly `400x200` (scale 0.4); a `1000x1000`
//! region under a `250000` px budget MUST land at `500x500` (scale 0.5); with
//! both budgets set the more restrictive wins; a budget larger than the
//! capture is a no-op; a zero budget fails loudly.
#![allow(clippy::too_many_lines)]

use anyhow::{Context, bail, ensure};
use serde_json::{Value, json};
use synapse_test_utils::stdio_mcp_client::StdioMcpClient;

fn u64_field(value: &Value, key: &str) -> anyhow::Result<u64> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .with_context(|| format!("response missing u64 field {key}: {value}"))
}

fn f64_field(value: &Value, key: &str) -> anyhow::Result<f64> {
    value
        .get(key)
        .and_then(Value::as_f64)
        .with_context(|| format!("response missing f64 field {key}: {value}"))
}

/// Decode the written file from disk and return its true (width, height). This is
/// the Source of Truth — independent of anything the tool reported.
fn decoded_dimensions(path: &str) -> anyhow::Result<(u32, u32)> {
    let bytes = std::fs::read(path).with_context(|| format!("read written screenshot {path}"))?;
    ensure!(!bytes.is_empty(), "written screenshot {path} is empty");
    let image = image::load_from_memory(&bytes)
        .with_context(|| format!("decode written screenshot {path}"))?;
    Ok(image::GenericImageView::dimensions(&image))
}

/// `screenshot operation=capture` through the public facade; returns the
/// `capture` readback object.
async fn capture(client: &mut StdioMcpClient, args: Value) -> anyhow::Result<Value> {
    let mut call = json!({"operation": "capture", "overwrite": true});
    if let (Some(call), Some(args)) = (call.as_object_mut(), args.as_object()) {
        call.extend(args.clone());
    }
    let response = client.tools_call("screenshot", call).await?;
    response
        .pointer("/structuredContent/capture")
        .cloned()
        .with_context(|| format!("screenshot response missing capture readback: {response}"))
}

/// Asserts reported and decoded dimensions plus scale for one capture.
fn assert_capture(
    capture: &Value,
    expected: (u32, u32),
    native: (u64, u64),
    scale: f64,
) -> anyhow::Result<()> {
    assert_eq!(
        u64_field(capture, "native_width")?,
        native.0,
        "native_width"
    );
    assert_eq!(
        u64_field(capture, "native_height")?,
        native.1,
        "native_height"
    );
    assert_eq!(u64_field(capture, "width")?, u64::from(expected.0), "width");
    assert_eq!(
        u64_field(capture, "height")?,
        u64::from(expected.1),
        "height"
    );
    let reported_scale = f64_field(capture, "scale")?;
    ensure!(
        (reported_scale - scale).abs() < 1e-9,
        "scale should be {scale}, was {reported_scale}"
    );
    // SOURCE OF TRUTH: decode the file from disk.
    let path = capture
        .get("path")
        .and_then(Value::as_str)
        .context("response path missing")?;
    assert_eq!(
        decoded_dimensions(path)?,
        expected,
        "decoded file dimensions must match the budget"
    );
    Ok(())
}

#[tokio::test]
async fn must_pass_capture_screenshot_downscale_writes_budgeted_pixels() -> anyhow::Result<()> {
    let mut client = StdioMcpClient::launch_and_init().await?;
    let dir = tempfile::tempdir()?;

    // Long-edge budget: 1000x500 native, budget 400 -> 400x200, scale 0.4.
    let happy = capture(
        &mut client,
        json!({
            "path": dir.path().join("happy.png").to_string_lossy(),
            "region": { "x": 0, "y": 0, "w": 1000, "h": 500 },
            "max_long_edge": 400
        }),
    )
    .await?;
    assert_capture(&happy, (400, 200), (1000, 500), 0.4)?;

    // Pixel-area budget: 1000x1000 = 1_000_000 px, budget 250_000 -> 500x500.
    let area = capture(
        &mut client,
        json!({
            "path": dir.path().join("area.png").to_string_lossy(),
            "region": { "x": 0, "y": 0, "w": 1000, "h": 1000 },
            "max_pixels": 250_000
        }),
    )
    .await?;
    assert_capture(&area, (500, 500), (1000, 1000), 0.5)?;

    // Both budgets: long edge 800 gives 0.8, area 125_000 of 500_000 gives
    // 0.5; the more restrictive (0.5) wins -> 500x250.
    let both = capture(
        &mut client,
        json!({
            "path": dir.path().join("both.png").to_string_lossy(),
            "region": { "x": 0, "y": 0, "w": 1000, "h": 500 },
            "max_long_edge": 800,
            "max_pixels": 125_000
        }),
    )
    .await?;
    assert_capture(&both, (500, 250), (1000, 500), 0.5)?;

    // No-op: budget larger than the capture leaves pixels untouched, scale 1.0.
    let noop = capture(
        &mut client,
        json!({
            "path": dir.path().join("noop.png").to_string_lossy(),
            "region": { "x": 0, "y": 0, "w": 800, "h": 600 },
            "max_pixels": 10_000_000u64
        }),
    )
    .await?;
    assert_capture(&noop, (800, 600), (800, 600), 1.0)?;

    let status = client.shutdown().await?;
    if !status.success() {
        bail!("daemon exited with non-success status: {status:?}");
    }
    Ok(())
}

#[tokio::test]
async fn must_fail_zero_pixel_budget_is_refused_without_writing_a_file() -> anyhow::Result<()> {
    let mut client = StdioMcpClient::launch_and_init().await?;
    let dir = tempfile::tempdir()?;

    // Zero budgets must fail loudly, not silently capture full-res. Each case
    // names the offending field; the second budget being valid does not save a
    // zero one.
    let cases = [
        (
            "long_edge_zero",
            json!({"max_long_edge": 0}),
            "max_long_edge must be greater than zero",
        ),
        (
            "pixels_zero",
            json!({"max_pixels": 0}),
            "max_pixels must be greater than zero",
        ),
        (
            "pixels_zero_with_valid_long_edge",
            json!({"max_pixels": 0, "max_long_edge": 400}),
            "max_pixels must be greater than zero",
        ),
    ];
    for (name, budget, fragment) in cases {
        let bad_path = dir.path().join(format!("{name}.png"));
        let mut args = json!({
            "operation": "capture",
            "path": bad_path.to_string_lossy(),
            "region": { "x": 0, "y": 0, "w": 100, "h": 100 },
            "overwrite": true
        });
        if let (Some(args), Some(budget)) = (args.as_object_mut(), budget.as_object()) {
            args.extend(budget.clone());
        }
        let error = client.tools_call_error("screenshot", args).await?;
        eprintln!("{name} error = {error}");
        ensure!(
            error.pointer("/data/code").and_then(Value::as_str) == Some("TOOL_PARAMS_INVALID"),
            "{name}: zero budget should fail with TOOL_PARAMS_INVALID, got {error}"
        );
        ensure!(
            error["message"]
                .as_str()
                .is_some_and(|message| message.contains(fragment)),
            "{name}: error must name the zero budget ({fragment}), got {error}"
        );
        ensure!(
            !bad_path.exists(),
            "{name}: a rejected zero-budget capture must not write a file"
        );
    }

    let status = client.shutdown().await?;
    if !status.success() {
        bail!("daemon exited with non-success status: {status:?}");
    }
    Ok(())
}
