//! MCP facade for tool #44 `livefeed`: background monitor camera for the agent.

#![cfg(feature = "livefeed")]

use std::sync::Arc;

use base64::Engine as _;
use rmcp::model::{CallToolResult, Content};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use synapse_core::{Rect, error_codes};
use synapse_livefeed::{
    Camera, CameraConfig, CameraStatus, Changes, LivefeedError, Look, ScreenState, Watch, Zoom,
};

use super::{ErrorData, Parameters, SynapseService, mcp_error, tool, tool_router};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum LivefeedParams {
    /// Start background capture on the primary monitor (or `monitor` index).
    Start {
        #[serde(default)]
        monitor: Option<u32>,
    },
    /// Stop background capture and join worker threads.
    Stop,
    /// Return the current screen as one downscaled JPEG image plus cursor and monitor origin.
    Look,
    /// Return cropped JPEG images and virtual-screen rects of areas that changed since `since`.
    Changes {
        since: u64,
        #[serde(default)]
        wait_ms: Option<u64>,
        /// Return only once the screen has been quiet for this long (bounded by `wait_ms`).
        #[serde(default)]
        settle_ms: Option<u64>,
    },
    /// Return a full-resolution JPEG crop of a virtual-screen rectangle.
    Zoom { x: i32, y: i32, w: i32, h: i32 },
    /// Return a span of time as ONE filmstrip grid image (row-major, change-gated samples).
    /// Optional x/y/w/h (all four, virtual-screen px) films only that area, sharper.
    Watch {
        #[serde(default)]
        ms: Option<u64>,
        #[serde(default)]
        max_frames: Option<usize>,
        #[serde(default)]
        x: Option<i32>,
        #[serde(default)]
        y: Option<i32>,
        #[serde(default)]
        w: Option<i32>,
        #[serde(default)]
        h: Option<i32>,
    },
    /// Return the livefeed camera status.
    Status,
}

fn source_of_truth(monitor: u32, seq: u64) -> String {
    format!("livefeed camera monitor {monitor} seq {seq}")
}

fn livefeed_mcp_error(err: &LivefeedError) -> ErrorData {
    mcp_error(err.code(), err.to_string())
}

fn join_mcp_error(err: &tokio::task::JoinError) -> ErrorData {
    mcp_error(
        error_codes::TOOL_INTERNAL_ERROR,
        format!("livefeed blocking task failed: {err}"),
    )
}

fn text_result(value: Value) -> CallToolResult {
    let mut result = CallToolResult::success(vec![Content::text(value.to_string())]);
    result.structured_content = Some(value);
    result
}

fn image_result(metadata: Value, mut images: Vec<Content>) -> CallToolResult {
    let mut content = Vec::with_capacity(images.len().saturating_add(usize::from(true)));
    content.push(Content::text(metadata.to_string()));
    content.append(&mut images);
    let mut result = CallToolResult::success(content);
    result.structured_content = Some(metadata);
    result
}

fn status_payload(operation: &str, status: &CameraStatus) -> Value {
    let source = source_of_truth(status.monitor, status.latest_seq);
    json!({
        "operation": operation,
        "source": source,
        "source_of_truth": source,
        "running": status.running,
        "monitor": status.monitor,
        "origin_x": status.origin_x,
        "origin_y": status.origin_y,
        "width": status.width,
        "height": status.height,
        "seq": status.latest_seq,
        "cursor": status.latest_seq,
        "latest_seq": status.latest_seq,
        "ts_ms": status.latest_ts_ms,
        "latest_ts_ms": status.latest_ts_ms,
        "screen_state": status.screen_state.as_str(),
        "frames_seen": status.frames_seen,
        "snapshots_stored": status.snapshots_stored,
        "history_bytes": status.history_bytes,
    })
}

fn format_look_result(look: Look) -> CallToolResult {
    let source = source_of_truth(look.monitor, look.seq);
    let screen_rect = Rect {
        x: look.origin_x,
        y: look.origin_y,
        w: i32::try_from(look.width).unwrap_or(i32::MAX),
        h: i32::try_from(look.height).unwrap_or(i32::MAX),
    };
    let metadata = json!({
        "operation": "look",
        "source": source,
        "source_of_truth": source,
        "monitor": look.monitor,
        "seq": look.seq,
        "cursor": look.seq,
        "ts_ms": look.ts_ms,
        "ts": look.ts_ms,
        "origin_x": look.origin_x,
        "origin_y": look.origin_y,
        "width": look.width,
        "height": look.height,
        "scale": look.scale,
        "rect": screen_rect,
        "screen_state": look.screen_state.as_str(),
    });
    let mut images = Vec::new();
    if look.screen_state == ScreenState::Live && !look.image_jpeg.is_empty() {
        let b64 = base64::engine::general_purpose::STANDARD.encode(&look.image_jpeg);
        images.push(Content::image(b64, "image/jpeg"));
    }
    image_result(metadata, images)
}

fn format_changes_result(changes: Changes) -> CallToolResult {
    let source = source_of_truth(changes.monitor, changes.seq);
    let mut rects = Vec::with_capacity(changes.regions.len());
    let mut regions = Vec::with_capacity(changes.regions.len());
    let mut images = Vec::new();
    for region in changes.regions {
        rects.push(region.rect);
        regions.push(json!({ "rect": region.rect }));
        if changes.screen_state == ScreenState::Live && !region.crop_jpeg.is_empty() {
            let b64 = base64::engine::general_purpose::STANDARD.encode(&region.crop_jpeg);
            images.push(Content::image(b64, "image/jpeg"));
        }
    }
    let metadata = json!({
        "operation": "changes",
        "source": source,
        "source_of_truth": source,
        "monitor": changes.monitor,
        "since": changes.since,
        "seq": changes.seq,
        "cursor": changes.seq,
        "ts_ms": changes.ts_ms,
        "ts": changes.ts_ms,
        "origin_x": changes.origin_x,
        "origin_y": changes.origin_y,
        "rects": rects,
        "regions": regions,
        "too_many": changes.too_many,
        "since_evicted": changes.since_evicted,
        "cut": changes.cut,
        "settled": changes.settled,
        "screen_state": changes.screen_state.as_str(),
    });
    image_result(metadata, images)
}

fn format_watch_result(watch: Watch) -> CallToolResult {
    let first = watch.frames.first().map_or(0, |f| f.seq);
    let last = watch.frames.last().map_or(0, |f| f.seq);
    let source = format!(
        "livefeed camera monitor {} seq {first}..{last}",
        watch.monitor
    );
    let frames: Vec<Value> = watch
        .frames
        .iter()
        .map(|f| json!({ "index": f.index, "seq": f.seq, "ts_ms": f.ts_ms }))
        .collect();
    let metadata = json!({
        "operation": "watch",
        "source": source,
        "source_of_truth": source,
        "monitor": watch.monitor,
        "seq": last,
        "cursor": last,
        "origin_x": watch.origin_x,
        "origin_y": watch.origin_y,
        "cols": watch.cols,
        "rows": watch.rows,
        "cell_w": watch.cell_w,
        "cell_h": watch.cell_h,
        "frames": frames,
        "region": watch.region,
        "ms_elapsed": watch.ms_elapsed,
        "reading_order": "row-major: cell 0 top-left, time increases left to right, then down",
        "screen_state": watch.screen_state.as_str(),
    });
    let mut images = Vec::new();
    if watch.screen_state == ScreenState::Live && !watch.image_jpeg.is_empty() {
        let b64 = base64::engine::general_purpose::STANDARD.encode(&watch.image_jpeg);
        images.push(Content::image(b64, "image/jpeg"));
    }
    image_result(metadata, images)
}

fn format_zoom_result(zoom: Zoom) -> CallToolResult {
    let source = source_of_truth(zoom.monitor, zoom.seq);
    let metadata = json!({
        "operation": "zoom",
        "source": source,
        "source_of_truth": source,
        "monitor": zoom.monitor,
        "seq": zoom.seq,
        "cursor": zoom.seq,
        "rect": zoom.rect,
        "origin_x": zoom.origin_x,
        "origin_y": zoom.origin_y,
        "screen_state": zoom.screen_state.as_str(),
    });
    let mut images = Vec::new();
    if zoom.screen_state == ScreenState::Live && !zoom.image_jpeg.is_empty() {
        let b64 = base64::engine::general_purpose::STANDARD.encode(&zoom.image_jpeg);
        images.push(Content::image(b64, "image/jpeg"));
    }
    image_result(metadata, images)
}

async fn run_start(camera: Arc<Camera>, monitor: Option<u32>) -> Result<CallToolResult, ErrorData> {
    let cfg = CameraConfig::from_env().map_err(|err| livefeed_mcp_error(&err))?;
    let status = tokio::task::spawn_blocking(move || {
        camera.start(cfg, monitor)?;
        Ok::<_, LivefeedError>(camera.status())
    })
    .await
    .map_err(|err| join_mcp_error(&err))?
    .map_err(|err| livefeed_mcp_error(&err))?;
    Ok(text_result(status_payload("start", &status)))
}

async fn run_stop(camera: Arc<Camera>) -> Result<CallToolResult, ErrorData> {
    let status = tokio::task::spawn_blocking(move || {
        camera.stop()?;
        Ok::<_, LivefeedError>(camera.status())
    })
    .await
    .map_err(|err| join_mcp_error(&err))?
    .map_err(|err| livefeed_mcp_error(&err))?;
    Ok(text_result(status_payload("stop", &status)))
}

async fn run_look(camera: Arc<Camera>) -> Result<CallToolResult, ErrorData> {
    let look = tokio::task::spawn_blocking(move || camera.look())
        .await
        .map_err(|err| join_mcp_error(&err))?
        .map_err(|err| livefeed_mcp_error(&err))?;
    Ok(format_look_result(look))
}

async fn run_changes(
    camera: Arc<Camera>,
    since: u64,
    wait_ms: Option<u64>,
    settle_ms: Option<u64>,
) -> Result<CallToolResult, ErrorData> {
    let changes =
        tokio::task::spawn_blocking(move || camera.changes_with(since, wait_ms, settle_ms))
            .await
            .map_err(|err| join_mcp_error(&err))?
            .map_err(|err| livefeed_mcp_error(&err))?;
    Ok(format_changes_result(changes))
}

/// All four of x/y/w/h or none; a partial region is a typed `LIVEFEED_BAD_REGION`.
fn watch_region(
    x: Option<i32>,
    y: Option<i32>,
    w: Option<i32>,
    h: Option<i32>,
) -> Result<Option<Rect>, ErrorData> {
    match (x, y, w, h) {
        (None, None, None, None) => Ok(None),
        (Some(x), Some(y), Some(w), Some(h)) => Ok(Some(Rect { x, y, w, h })),
        _ => Err(livefeed_mcp_error(&LivefeedError::bad_region(
            "watch region needs all of x, y, w, h (or none for the whole monitor)",
        ))),
    }
}

async fn run_watch(
    camera: Arc<Camera>,
    ms: Option<u64>,
    max_frames: Option<usize>,
    region: Option<Rect>,
) -> Result<CallToolResult, ErrorData> {
    let watch = tokio::task::spawn_blocking(move || camera.watch_region(ms, max_frames, region))
        .await
        .map_err(|err| join_mcp_error(&err))?
        .map_err(|err| livefeed_mcp_error(&err))?;
    Ok(format_watch_result(watch))
}

async fn run_zoom(camera: Arc<Camera>, virtual_rect: Rect) -> Result<CallToolResult, ErrorData> {
    let zoom = tokio::task::spawn_blocking(move || camera.zoom(virtual_rect))
        .await
        .map_err(|err| join_mcp_error(&err))?
        .map_err(|err| livefeed_mcp_error(&err))?;
    Ok(format_zoom_result(zoom))
}

async fn run_status(camera: Arc<Camera>) -> Result<CallToolResult, ErrorData> {
    let status = tokio::task::spawn_blocking(move || camera.status())
        .await
        .map_err(|err| join_mcp_error(&err))?;
    Ok(text_result(status_payload("status", &status)))
}

#[tool_router(router = livefeed_tool_router, vis = "pub(crate)")]
impl SynapseService {
    #[tool(
        input_schema = super::schema_sanitize::tagged_object_input_schema::<LivefeedParams>(),
        description = "Background monitor camera for the agent (the agent's retina: it watches continuously, the agent reads on demand). start begins continuous capture on the primary monitor (or monitor index); stop ends it and joins worker threads; look = what is on screen NOW (one downscaled JPEG + cursor + monitor origin); changes = what changed since cursor, as full-res crops with virtual-screen rects; wait_ms blocks until the first change, settle_ms instead returns once the screen has been quiet that long (use for 'wait until it finished loading'); cut=true means most of the screen changed, so re-key with look; watch = a span of time (ms) as ONE row-major filmstrip grid image of change-gated samples (use to see motion: animations, loading bars, a window opening); pass x/y/w/h to film only that area at up to native resolution (a game window, a progress bar) because whole-monitor cells are too small for small detail; zoom = full-resolution crop of a virtual-screen rect for small text; status reports camera state. Locked, black or lost screens are reported in screen_state and never return stale images."
    )]
    pub async fn livefeed(
        &self,
        params: Parameters<LivefeedParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let camera = Arc::clone(&self.livefeed_camera);
        match params.0 {
            LivefeedParams::Start { monitor } => run_start(camera, monitor).await,
            LivefeedParams::Stop => run_stop(camera).await,
            LivefeedParams::Look => run_look(camera).await,
            LivefeedParams::Changes {
                since,
                wait_ms,
                settle_ms,
            } => run_changes(camera, since, wait_ms, settle_ms).await,
            LivefeedParams::Zoom { x, y, w, h } => run_zoom(camera, Rect { x, y, w, h }).await,
            LivefeedParams::Watch {
                ms,
                max_frames,
                x,
                y,
                w,
                h,
            } => run_watch(camera, ms, max_frames, watch_region(x, y, w, h)?).await,
            LivefeedParams::Status => run_status(camera).await,
        }
    }
}
