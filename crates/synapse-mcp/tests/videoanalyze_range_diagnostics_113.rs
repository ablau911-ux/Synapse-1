//! #113: an out-of-range `videoanalyze` request must be refused with
//! `TOOL_PARAMS_INVALID` AND name the rejected values and the PROBED duration
//! (no silent clipping). The previous tests fed fabricated metadata to the
//! validators; these drive the real public handler, so the duration in the
//! message must be the one ffprobe really reported (read back first through
//! `videoanalyze operation=inspect`, the source of truth).
//!
//! Contract: `server/videoanalyze.rs` `validate_sampling_range` /
//! `validate_motion_range` messages ("received start_seconds=..,
//! end_seconds=.., duration_seconds=.."). Fixture: a synthetic 5-frame GIF
//! (5 x 100 ms) written by the test. Requires FFmpeg/ffprobe on PATH, as the
//! tool itself does.

#[allow(dead_code)]
mod support;

use anyhow::{Context, ensure};
use image::{Delay, Frame, Rgba, RgbaImage, codecs::gif::GifEncoder};
use serde_json::{Value, json};
use support::{HttpDaemon, HttpMcpSession};

/// Writes a 5-frame, 100 ms/frame synthetic GIF.
fn write_fixture_gif(path: &std::path::Path) -> anyhow::Result<()> {
    let file = std::fs::File::create(path)?;
    let mut encoder = GifEncoder::new(file);
    for shade in [0_u8, 60, 120, 180, 240] {
        let image = RgbaImage::from_pixel(64, 48, Rgba([shade, 255 - shade, 90, 255]));
        let delay = Delay::from_numer_denom_ms(100, 1);
        encoder.encode_frame(Frame::from_parts(image, 0, 0, delay))?;
    }
    Ok(())
}

/// Probed duration (seconds) via the public `inspect` operation.
async fn probed_duration(session: &mut HttpMcpSession, path: &str) -> anyhow::Result<f64> {
    let inspect = session
        .tools_call(
            "videoanalyze",
            json!({"operation": "inspect", "path": path}),
        )
        .await?;
    let duration = inspect["structuredContent"]["metadata"]["format"]["duration"]
        .as_str()
        .context("inspect returned no format.duration")?
        .parse::<f64>()?;
    ensure!(
        duration > 0.0 && duration < 1.0,
        "precondition: 5x100 ms GIF, got {duration}"
    );
    Ok(duration)
}

/// Refusal of `args`: asserts `TOOL_PARAMS_INVALID` and returns the message.
async fn refusal(session: &mut HttpMcpSession, args: Value) -> anyhow::Result<String> {
    let error = session.tools_call_error("videoanalyze", args).await?;
    ensure!(
        error["data"]["code"] == json!(synapse_core::error_codes::TOOL_PARAMS_INVALID),
        "out-of-range request must be TOOL_PARAMS_INVALID: {error}"
    );
    Ok(error["message"].as_str().unwrap_or_default().to_owned())
}

async fn fixture() -> anyhow::Result<(HttpDaemon, HttpMcpSession, String, f64)> {
    let daemon = HttpDaemon::start(&[]).await?;
    let gif = daemon.home().join("fixture-113.gif");
    write_fixture_gif(&gif)?;
    let path = gif.to_string_lossy().into_owned();
    let mut session = daemon.session("g6a-113", &[]).await?;
    let duration = probed_duration(&mut session, &path).await?;
    Ok((daemon, session, path, duration))
}

#[tokio::test]
async fn must_fail_analyze_end_past_duration_names_end_and_probed_duration() -> anyhow::Result<()> {
    let (daemon, mut session, path, duration) = fixture().await?;
    let args =
        json!({"operation": "analyze", "path": path, "end_seconds": 8.0, "include_audio": false});
    let message = refusal(&mut session, args).await?;
    for needle in [
        "start_seconds=0".to_owned(),
        "end_seconds=8".to_owned(),
        format!("duration_seconds={duration}"),
    ] {
        ensure!(
            message.contains(&needle),
            "#113: missing `{needle}` in: {message}"
        );
    }
    drop(daemon.stop().await?);
    Ok(())
}

#[tokio::test]
async fn must_fail_motion_end_past_duration_names_range_and_probed_duration() -> anyhow::Result<()>
{
    let (daemon, mut session, path, duration) = fixture().await?;
    let args =
        json!({"operation": "motion", "path": path, "start_seconds": 0.1, "end_seconds": 10.0});
    let message = refusal(&mut session, args).await?;
    for needle in [
        "start_seconds=0.1".to_owned(),
        "end_seconds=10".to_owned(),
        format!("duration_seconds={duration}"),
        format!("range_seconds={}", 10.0_f64 - 0.1),
    ] {
        ensure!(
            message.contains(&needle),
            "#113: missing `{needle}` in: {message}"
        );
    }
    drop(daemon.stop().await?);
    Ok(())
}

#[tokio::test]
async fn must_fail_empty_range_names_both_endpoints() -> anyhow::Result<()> {
    let (daemon, mut session, path, duration) = fixture().await?;
    let edge = duration / 2.0;
    let args = json!({
        "operation": "analyze", "path": path, "start_seconds": edge, "end_seconds": edge,
        "include_audio": false
    });
    let message = refusal(&mut session, args).await?;
    for needle in [
        format!("start_seconds={edge}"),
        format!("end_seconds={edge}"),
    ] {
        ensure!(
            message.contains(&needle),
            "#113: missing `{needle}` in: {message}"
        );
    }
    drop(daemon.stop().await?);
    Ok(())
}

#[tokio::test]
async fn must_pass_in_range_analyze_returns_frames_inside_the_probed_duration() -> anyhow::Result<()>
{
    let (daemon, mut session, path, duration) = fixture().await?;
    let args = json!({
        "operation": "analyze", "path": path, "start_seconds": 0.0, "end_seconds": duration,
        "frame_count": 2, "include_audio": false
    });
    let response = session.tools_call("videoanalyze", args).await?;
    let frames = response["structuredContent"]["frames"]
        .as_array()
        .context("analyze returned no frames array")?;
    ensure!(
        frames.len() == 2,
        "in-range analyze must return 2 frames: {response}"
    );
    drop(daemon.stop().await?);
    Ok(())
}
