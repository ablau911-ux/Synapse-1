//! A targetless screenshot must not create a desktop artifact (#23).
//!
//! Driven through the shared schema-validating HTTP MCP harness (D1 client
//! parity). The explicit-region capture needs a real, unlocked desktop (WGC /
//! screen capture); a locked screen fails it, which is reported, not faked.

#[allow(dead_code)]
mod support;

use anyhow::{Context, ensure};
use serde_json::json;
use support::HttpDaemon;

#[tokio::test]
async fn must_fail_unbound_capture_refuses_without_writing_file() -> anyhow::Result<()> {
    let daemon = HttpDaemon::start(&[]).await?;
    let path = daemon.home().join("unbound.png");
    let mut session = daemon.session("syn23-regression", &[]).await?;
    let error = session
        .tools_call_error(
            "screenshot",
            json!({"operation": "capture", "path": path.to_string_lossy()}),
        )
        .await?;
    ensure!(
        error["data"]["code"] == json!(synapse_core::error_codes::CAPTURE_TARGET_INVALID),
        "targetless capture did not return CAPTURE_TARGET_INVALID: {error}"
    );
    ensure!(
        !path.exists(),
        "targetless capture wrote {}",
        path.display()
    );
    drop(daemon.stop().await?);
    Ok(())
}

#[tokio::test]
async fn must_pass_explicit_region_capture_writes_the_requested_artifact() -> anyhow::Result<()> {
    let daemon = HttpDaemon::start(&[]).await?;
    let scoped_path = daemon.home().join("explicit-region.png");
    let mut session = daemon.session("syn23-regression", &[]).await?;
    let response = session
        .tools_call(
            "screenshot",
            json!({
                "operation": "capture", "path": scoped_path.to_string_lossy(),
                "region": {"x": 0, "y": 0, "w": 32, "h": 24}
            }),
        )
        .await?;
    let capture = &response["structuredContent"]["capture"];
    ensure!(
        capture["width"] == 32 && capture["height"] == 24,
        "explicit region dimensions mismatch: {response}"
    );
    // Source of truth: decode the artifact on disk.
    let bytes = std::fs::read(&scoped_path).with_context(|| {
        format!(
            "explicit region wrote no artifact at {}",
            scoped_path.display()
        )
    })?;
    let image = image::load_from_memory(&bytes).context("decode explicit-region artifact")?;
    ensure!(
        image::GenericImageView::dimensions(&image) == (32, 24),
        "decoded artifact must be 32x24, got {:?}",
        image::GenericImageView::dimensions(&image)
    );
    drop(daemon.stop().await?);
    Ok(())
}
