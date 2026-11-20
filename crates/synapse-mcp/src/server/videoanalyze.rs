//! Timestamped video evidence delivered inline for the calling model to analyze.

use super::{ErrorData, Parameters, SynapseService, mcp_error, tool, tool_router};
use base64::Engine as _;
use rmcp::{
    RoleServer,
    model::{AnnotateAble, CallToolResult, Content, RawAudioContent, RawContent},
    service::RequestContext,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
// Plain SHA-256 is reserved for decoded-frame and PNG integrity evidence.
use sha2::{Digest as _, Sha256};
use std::{
    path::{Path, PathBuf},
    process::Output,
    time::Duration,
};
use synapse_core::error_codes;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum VideoAnalyzeParams {
    /// Read video metadata (duration, streams). Takes path only: no range, frame_count, or audio
    /// keys. Call inspect first, read the duration, then call analyze or motion with a range.
    Inspect { path: String },
    /// Measure decoded-frame visual changes in 250 ms bins, up to 60 seconds.
    Motion {
        path: String,
        #[serde(default)]
        start_seconds: Option<f64>,
        #[serde(default)]
        end_seconds: Option<f64>,
    },
    Analyze {
        path: String,
        #[serde(default)]
        start_seconds: Option<f64>,
        #[serde(default)]
        end_seconds: Option<f64>,
        #[serde(default)]
        frame_count: Option<u32>,
        #[serde(default)]
        max_long_edge: Option<u32>,
        #[serde(default)]
        include_audio: Option<bool>,
        #[serde(default)]
        audio_clip_seconds: Option<f64>,
    },
    TextTimeline {
        path: String,
        #[serde(default)]
        start_seconds: Option<f64>,
        #[serde(default)]
        end_seconds: Option<f64>,
        #[serde(default)]
        frame_count: Option<u32>,
        #[serde(default)]
        max_long_edge: Option<u32>,
        #[serde(default)]
        lang_hint: Option<String>,
        #[serde(default)]
        confidence_threshold: Option<f32>,
        /// Add independent text-detector pixel coverage without filtering OCR words.
        /// Requires the pinned PP-OCRv5 mobile detection model installed on this host.
        #[serde(default)]
        include_text_support: bool,
    },
}

fn invalid(message: impl Into<String>) -> ErrorData {
    mcp_error(error_codes::TOOL_PARAMS_INVALID, message.into())
}

/// Reads the source video's file metadata, naming the path on failure (#235).
fn source_metadata(path: impl AsRef<std::path::Path>) -> Result<std::fs::Metadata, ErrorData> {
    let path = path.as_ref();
    if path.as_os_str().to_string_lossy().trim().is_empty() {
        return Err(invalid("videoanalyze path must not be empty"));
    }
    std::fs::metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            invalid(format!("videoanalyze file not found: {}", path.display()))
        } else {
            invalid(format!(
                "videoanalyze could not read {}: {error}",
                path.display()
            ))
        }
    })
}

fn duration_from_metadata(metadata: &Value) -> Result<f64, ErrorData> {
    let finite_duration = |value: &Value| {
        value
            .as_str()
            .and_then(|text| text.parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v > 0.0)
    };
    finite_duration(&metadata["format"]["duration"])
        .or_else(|| finite_duration(&metadata["streams"][0]["duration"]))
        .ok_or_else(|| {
            invalid("video duration is unavailable; cannot choose a reliable sampling range")
        })
}

fn validate_sampling_range(
    metadata: &Value,
    start: Option<f64>,
    end: Option<f64>,
    count: Option<u32>,
    size: Option<u32>,
    max_count: u32,
) -> Result<(f64, f64, u32, u32), ErrorData> {
    let duration = duration_from_metadata(metadata)?;
    let start = start.unwrap_or(0.0);
    let end = end.unwrap_or(duration);
    let count = count.unwrap_or(6);
    let size = size.unwrap_or(1280);
    if !start.is_finite()
        || !end.is_finite()
        || start < 0.0
        || start >= end
        || end > duration
        || !(1..=max_count).contains(&count)
        || !(128..=2048).contains(&size)
    {
        return Err(invalid(format!(
            "require finite 0 <= start_seconds < end_seconds <= duration, frame_count 1..={max_count}, max_long_edge 128..=2048; received start_seconds={start}, end_seconds={end}, duration_seconds={duration}, frame_count={count}, max_long_edge={size}"
        )));
    }
    Ok((start, end, count, size))
}

fn validate_motion_range(
    metadata: &Value,
    start: Option<f64>,
    end: Option<f64>,
) -> Result<(f64, f64), ErrorData> {
    let duration = duration_from_metadata(metadata).map_err(|_| {
        invalid(format!(
            "motion analysis requires a finite video duration; received start_seconds={start:?}, end_seconds={end:?}, duration_seconds=unavailable"
        ))
    })?;
    let start = start.unwrap_or(0.0);
    let end = end.unwrap_or(duration.min(start + 60.0));
    if !start.is_finite()
        || !end.is_finite()
        || start < 0.0
        || end <= start
        || end > duration
        || end - start > 60.0
    {
        return Err(invalid(format!(
            "motion requires finite 0 <= start_seconds < end_seconds <= duration and a range at most 60 seconds; received start_seconds={start}, end_seconds={end}, duration_seconds={duration}, range_seconds={}",
            end - start
        )));
    }
    Ok((start, end))
}

async fn motion_timeline(
    source: &str,
    start: Option<f64>,
    end: Option<f64>,
    context: &RequestContext<RoleServer>,
) -> Result<CallToolResult, ErrorData> {
    let path = resolve_video(source, context).await?;
    let before = source_metadata(&path)?;
    let metadata = probe(&path, context).await?;
    let (start, end) = validate_motion_range(&metadata, start, end)?;
    // Preserve each decoded frame's timestamp; no temporal sampling filter.
    // Spatial downscaling bounds cost, but can hide tiny visual changes.
    let args = vec![
        "-hide_banner".into(), "-loglevel".into(), "error".into(), "-nostdin".into(),
        "-ss".into(), format!("{start:.6}"), "-i".into(), path.clone(),
        "-t".into(), format!("{:.6}", end - start), "-map".into(), "0:v:0".into(),
        "-an".into(), "-vf".into(),
        "scale=160:160:force_original_aspect_ratio=decrease:force_divisible_by=2,format=yuv420p,signalstats,metadata=mode=print:key=lavfi.signalstats.YDIF:file=-".into(),
        "-frames:v".into(), "20000".into(), "-f".into(), "null".into(), "-".into(),
    ];
    let output = run_decoder("ffmpeg", &args, context).await?;
    let text = String::from_utf8(output.stdout).map_err(|error| invalid(error.to_string()))?;
    let mut timestamp = None;
    let mut previous = None;
    let mut count = 0_u32;
    let mut decoded = 0_u32;
    let mut bins = std::collections::BTreeMap::<u32, (u32, f64, f64)>::new();
    for line in text.lines() {
        if let Some((_, value)) = line.split_once("pts_time:") {
            timestamp = value.trim().parse::<f64>().ok();
        } else if let Some(value) = line.strip_prefix("lavfi.signalstats.YDIF=") {
            decoded += 1;
            let time = timestamp
                .take()
                .ok_or_else(|| invalid("motion frame has no timestamp"))?;
            let change = value
                .parse::<f64>()
                .map_err(|_| invalid("invalid motion value"))?;
            if !time.is_finite()
                || time < 0.0
                || !change.is_finite()
                || change < 0.0
                || previous.is_some_and(|last| time < last)
            {
                return Err(invalid("invalid or unordered motion measurements"));
            }
            if time >= end - start {
                continue;
            }
            previous = Some(time);
            count += 1;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let index = (time * 4.0).floor() as u32;
            let bin = bins.entry(index).or_insert((0, 0.0, start + time));
            bin.0 += 1;
            if change > bin.1 {
                bin.1 = change;
                bin.2 = start + time;
            }
        }
    }
    if count == 0 {
        return Err(invalid("video produced no motion measurements"));
    }
    let after = source_metadata(&path)?;
    if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
        return Err(invalid("video changed during motion analysis"));
    }
    let bins: Vec<Value> = bins
        .into_iter()
        .map(|(index, (frames, peak, at))| {
            json!({
                "start_seconds": start + f64::from(index) / 4.0,
                "decoded_frames": frames, "peak_luma_difference": peak, "peak_at_seconds": at,
            })
        })
        .collect();
    let value = json!({
        "source_of_truth": "local video + FFmpeg decoded-frame signalstats.YDIF",
        "path": path, "source_size_bytes": after.len(), "start_seconds": start,
        "end_seconds": end, "decoded_frames": count, "frame_cap_reached": decoded >= 20000,
        "last_measured_seconds": previous.map(|time| start + time), "bin_seconds": 0.25,
        "bins": bins,
        "interpretation_limits": "Spatially reduced luma differences locate activity, not loading completion or UI meaning. Tiny changes can disappear. First frame has no predecessor. Inspect video around peaks; never infer a freeze from a low score alone."
    });
    let mut result = CallToolResult::success(vec![Content::text(value.to_string())]);
    result.structured_content = Some(value);
    Ok(result)
}

async fn text_timeline(
    source: &str,
    start: Option<f64>,
    end: Option<f64>,
    count: Option<u32>,
    size: Option<u32>,
    lang_hint: Option<String>,
    confidence_threshold: Option<f32>,
    include_text_support: bool,
    context: &RequestContext<RoleServer>,
) -> Result<CallToolResult, ErrorData> {
    if include_text_support {
        return Err(invalid(
            "include_text_support requires the video text-support detector, which is not available in this build",
        ));
    }
    let path = resolve_video(source, context).await?;
    let before = source_metadata(&path)?;
    let metadata = probe(&path, context).await?;
    let (start, end, count, size) =
        validate_sampling_range(&metadata, start, end, count, size, 24)?;
    let threshold = confidence_threshold.unwrap_or(0.45);
    if !threshold.is_finite() || !(0.0..=1.0).contains(&threshold) {
        return Err(invalid("confidence_threshold must be 0.0..=1.0"));
    }
    let origin = video_origin(&metadata)?;
    let final_time = if count > 1 {
        final_sample_time(&path, start, end, context).await?
    } else {
        start
    };
    let mut frames = Vec::new();
    for index in 0..count {
        let timestamp = sample_timestamp(index, count, start, final_time);
        let seek_time = endpoint_safe_seek_time(index, count, timestamp, start);
        let filter = format!(
            "settb=expr=1/1000000,scale=w='min({size},iw)':h='min({size},ih)':force_original_aspect_ratio=decrease,format=rgba,showinfo"
        );
        let args = vec![
            "-hide_banner".into(),
            "-loglevel".into(),
            "info".into(),
            "-nostdin".into(),
            "-ss".into(),
            format!("{seek_time:.6}"),
            "-copyts".into(),
            "-i".into(),
            path.clone(),
            "-map".into(),
            "0:v:0".into(),
            "-frames:v".into(),
            "1".into(),
            "-vf".into(),
            filter,
            "-f".into(),
            "rawvideo".into(),
            "pipe:1".into(),
        ];
        let output = run_decoder("ffmpeg", &args, context).await?;
        let stderr = String::from_utf8_lossy(&output.stderr);
        let decoded_time = stderr
            .lines()
            .find_map(decoded_timestamp)
            .ok_or_else(|| invalid("decoded video frame has no finite timestamp"))?;
        let relative_time = decoded_time - origin;
        if relative_time < start - 0.000_001 || relative_time >= end {
            return Err(invalid("decoded frame is outside requested range"));
        }
        if count > 1 && index == count - 1 && (relative_time - final_time).abs() > 0.000_01 {
            return Err(invalid(
                "decoded final frame does not match probed endpoint",
            ));
        }
        let (width, height) = decoded_frame_dimensions(&stderr)?;
        let mut bgra = output.stdout;
        validate_rgba_len(width, height, &bgra)?;
        for pixel in bgra.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        let region = synapse_core::Rect {
            x: 0,
            y: 0,
            w: i32::try_from(width)
                .map_err(|err| invalid(format!("OCR frame width invalid: {err}")))?,
            h: i32::try_from(height)
                .map_err(|err| invalid(format!("OCR frame height invalid: {err}")))?,
        };
        let ocr = read_frame_text(region, width, height, &bgra, lang_hint.as_deref())?;
        let words: Vec<Value> = ocr
            .into_iter()
            .filter(|word| {
                word.confidence_source
                    .meets_threshold(word.confidence, threshold)
                    && !word.text.trim().is_empty()
            })
            .map(|word| {
                let value = json!({
                    "text": word.text,
                    "bbox": word.bbox,
                    "confidence": word.confidence,
                    "confidence_source": word.confidence_source.as_str(),
                });
                Ok(value)
            })
            .collect::<Result<_, ErrorData>>()?;
        let frame = json!({
            "index": index,
            "requested_seconds": timestamp,
            "decoded_pts_seconds": decoded_time,
            "relative_seconds": relative_time,
            "width": width,
            "height": height,
            "frame_bgra_sha256": Sha256::digest(&bgra).iter().map(|byte| format!("{byte:02x}")).collect::<String>(),
            "word_count": words.len(),
            "words": words,
        });
        frames.push(frame);
    }
    let after = source_metadata(&path)?;
    if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
        return Err(invalid(
            "video changed during text analysis; no mixed-version frames returned",
        ));
    }
    let value = json!({
        "source_of_truth": "local video file + FFmpeg decoded frame bytes + configured OCR backend",
        "source": source,
        "path": path,
        "source_size_bytes": after.len(),
        "start_seconds": start,
        "end_seconds": end,
        "frame_count": count,
        "max_long_edge": size,
        "confidence_threshold": threshold,
        "lang_hint": lang_hint,
        "word_order": "ocr_backend",
        "frames": frames,
        "interpretation_limits": "Words retain OCR backend sequence, not guaranteed linguistic reading order. OCR text is semantic evidence for visible words only. Missing or garbled words do not prove absence of UI state; inspect sampled images around unexpected gaps."
    });
    let mut result = CallToolResult::success(vec![Content::text(value.to_string())]);
    result.structured_content = Some(value);
    Ok(result)
}

#[cfg(windows)]
fn read_frame_text(
    region: synapse_core::Rect,
    width: u32,
    height: u32,
    bgra: &[u8],
    _lang_hint: Option<&str>,
) -> Result<Vec<synapse_perception::TextRegion>, ErrorData> {
    match synapse_perception::read_text_from_bgra_bitmap(region, width, height, bgra) {
        Ok(words) => Ok(words),
        Err(synapse_perception::PerceptionError::OcrNoText { .. }) => Ok(Vec::new()),
        Err(error) => Err(invalid(format!("video text OCR failed: {error}"))),
    }
}

#[cfg(not(windows))]
fn read_frame_text(
    _region: synapse_core::Rect,
    _width: u32,
    _height: u32,
    _bgra: &[u8],
    _lang_hint: Option<&str>,
) -> Result<Vec<synapse_perception::TextRegion>, ErrorData> {
    Err(invalid("video text OCR requires the Windows OCR adapter"))
}

fn decoder_path(name: &str) -> PathBuf {
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let installed = PathBuf::from(local)
            .join("Microsoft/WinGet/Links")
            .join(format!("{name}.exe"));
        if installed.is_file() {
            return installed;
        }
    }
    PathBuf::from(name)
}

fn video_origin(metadata: &Value) -> Result<f64, ErrorData> {
    metadata["format"]["start_time"]
        .as_str()
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite())
        .ok_or_else(|| invalid("video start timestamp is unavailable"))
}

fn sample_timestamp(index: u32, count: u32, start: f64, final_time: f64) -> f64 {
    if count == 1 {
        start
    } else {
        start + (final_time - start) * f64::from(index) / f64::from(count - 1)
    }
}

fn endpoint_safe_seek_time(index: u32, count: u32, timestamp: f64, start: f64) -> f64 {
    if count > 1 && index == count - 1 {
        (timestamp - 0.000_001).max(start)
    } else {
        timestamp
    }
}

fn decoded_frame_dimensions(stderr: &str) -> Result<(u32, u32), ErrorData> {
    stderr
        .lines()
        .find_map(|line| {
            let (_, rest) = line.split_once(" s:")?;
            let dimensions = rest.split_whitespace().next()?;
            let (width, height) = dimensions.split_once('x')?;
            Some((width.parse::<u32>().ok()?, height.parse::<u32>().ok()?))
        })
        .filter(|(width, height)| *width > 0 && *height > 0)
        .ok_or_else(|| invalid("decoded video frame dimensions are unavailable"))
}

fn validate_rgba_len(width: u32, height: u32, bytes: &[u8]) -> Result<(), ErrorData> {
    let expected = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| invalid("decoded video frame dimensions overflow"))?;
    let actual = u64::try_from(bytes.len())
        .map_err(|_err| invalid("decoded video frame byte length is invalid"))?;
    if expected == actual {
        Ok(())
    } else {
        Err(invalid(format!(
            "decoded video frame byte length mismatch: expected {expected}, got {actual}"
        )))
    }
}

async fn run_decoder(
    name: &str,
    args: &[String],
    context: &RequestContext<RoleServer>,
) -> Result<Output, ErrorData> {
    #[cfg(test)]
    if let Ok(result) = VIDEO_RESOLVER_TEST_IO.try_with(|io| (io.decoder)(name, args)) {
        return decoder_result(name, result?);
    }
    let timeout_seconds = if name == "python" { 300 } else { 30 };
    let mut command =
        crate::m4::child_environment::internal_helper_tokio_command(decoder_path(name));
    command.args(args).kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let output = tokio::select! {
        () = context.ct.cancelled() => return Err(invalid("video analysis cancelled")),
        result = tokio::time::timeout(Duration::from_secs(timeout_seconds), command.output()) => result
            .map_err(|_| invalid(format!("{name} timed out after {timeout_seconds} seconds")))?
            .map_err(|error| invalid(format!("could not run {name}: {error}; install FFmpeg with ffprobe")))?,
    };
    decoder_result(name, output)
}

fn decoder_result(name: &str, output: Output) -> Result<Output, ErrorData> {
    if !output.status.success() {
        return Err(invalid(format!(
            "{name} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(output)
}

fn video_cache_root() -> Result<PathBuf, ErrorData> {
    #[cfg(test)]
    if let Ok(cache) = VIDEO_RESOLVER_TEST_IO.try_with(|io| io.cache_root.clone()) {
        return Ok(cache);
    }
    Ok(PathBuf::from(
        std::env::var_os("LOCALAPPDATA").ok_or_else(|| invalid("LOCALAPPDATA is unavailable"))?,
    )
    .join("synapse/video-cache"))
}

#[cfg(test)]
type VideoDecoder =
    std::sync::Arc<dyn Fn(&str, &[String]) -> Result<Output, ErrorData> + Send + Sync>;

#[cfg(test)]
struct VideoResolverTestIo {
    cache_root: PathBuf,
    decoder: VideoDecoder,
}

#[cfg(test)]
tokio::task_local! {
    static VIDEO_RESOLVER_TEST_IO: VideoResolverTestIo;
}

#[cfg(test)]
async fn with_video_resolver_test_io<F: std::future::Future>(
    cache_root: PathBuf,
    decoder: VideoDecoder,
    future: F,
) -> F::Output {
    VIDEO_RESOLVER_TEST_IO
        .scope(
            VideoResolverTestIo {
                cache_root,
                decoder,
            },
            future,
        )
        .await
}

async fn resolve_video(
    source: &str,
    context: &RequestContext<RoleServer>,
) -> Result<String, ErrorData> {
    if !source.starts_with("https://") && !source.starts_with("http://") {
        return Ok(source.to_owned());
    }
    let cache = video_cache_root()?;
    std::fs::create_dir_all(&cache).map_err(|error| invalid(error.to_string()))?;
    let key = crate::content_digest::keyed_text_digest_hex(source);
    let template = cache.join(format!("{key}.%(ext)s"));
    let args = vec![
        "-m".into(),
        "yt_dlp".into(),
        "--no-playlist".into(),
        "--no-progress".into(),
        "--js-runtimes".into(),
        "node".into(),
        "--print".into(),
        "after_move:filepath".into(),
        "-f".into(),
        "bestvideo[height<=720]+bestaudio/best[height<=720]/best".into(),
        "--ffmpeg-location".into(),
        decoder_path("ffmpeg").to_string_lossy().into_owned(),
        "-o".into(),
        template.to_string_lossy().into_owned(),
        "--".into(),
        source.to_owned(),
    ];
    let output = run_decoder("python", &args, context).await?;
    let path = String::from_utf8(output.stdout).map_err(|error| invalid(error.to_string()))?;
    let path = path
        .lines()
        .last()
        .ok_or_else(|| invalid("video downloader returned no file"))?
        .trim();
    if !Path::new(path).is_file() {
        return Err(invalid("downloaded video file is missing"));
    }
    Ok(path.to_owned())
}

async fn probe(path: &str, context: &RequestContext<RoleServer>) -> Result<Value, ErrorData> {
    if !Path::new(path).is_absolute() || !Path::new(path).is_file() {
        return Err(invalid(
            "path must name an existing absolute local video file",
        ));
    }
    let args = [
        "-v",
        "error",
        "-show_streams",
        "-show_format",
        "-of",
        "json",
        path,
    ]
    .map(str::to_owned);
    let output = run_decoder("ffprobe", &args, context).await?;
    let metadata: Value =
        serde_json::from_slice(&output.stdout).map_err(|error| invalid(error.to_string()))?;
    if metadata["streams"]
        .as_array()
        .is_none_or(|streams| !streams.iter().any(|stream| stream["codec_type"] == "video"))
    {
        return Err(invalid("file contains no video stream"));
    }
    Ok(metadata)
}

// Both decoding paths set a microsecond time base before showinfo. Read integer
// PTS to avoid the rounded, six-significant-digit pts_time field.
fn decoded_timestamp(line: &str) -> Option<f64> {
    line.split_once(" pts:")?
        .1
        .split_whitespace()
        .next()?
        .parse::<f64>()
        .ok()
        .map(|pts| pts / 1_000_000.0)
        .filter(|time| time.is_finite())
}

/// Decode the tail to include buffered B-frames before the exclusive end.
async fn final_sample_time(
    path: &str,
    start: f64,
    end: f64,
    context: &RequestContext<RoleServer>,
) -> Result<f64, ErrorData> {
    let tail_start = start.max(end - 60.0);
    let args = vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "info".into(),
        "-nostdin".into(),
        "-ss".into(),
        format!("{tail_start:.6}"),
        "-i".into(),
        path.into(),
        "-t".into(),
        format!("{:.6}", end - tail_start),
        "-map".into(),
        "0:v:0".into(),
        "-an".into(),
        "-vf".into(),
        "settb=expr=1/1000000,scale=2:2,showinfo".into(),
        "-frames:v".into(),
        "20001".into(),
        "-f".into(),
        "null".into(),
        "-".into(),
    ];
    let output = run_decoder("ffmpeg", &args, context).await?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    let times: Vec<f64> = stderr.lines().filter_map(decoded_timestamp).collect();
    if times.len() >= 20001 {
        return Err(invalid(
            "final-frame scan reached its frame limit; use a shorter range",
        ));
    }
    times
        .into_iter()
        .map(|time| tail_start + time)
        .filter(|time| *time >= start && *time < end)
        .max_by(f64::total_cmp)
        .ok_or_else(|| {
            invalid("no timestamped video frame in requested range; narrow or move the range")
        })
}

#[tool_router(router = videoanalyze_tool_router, vis = "pub(crate)")]
impl SynapseService {
    #[tool(
        input_schema = super::schema_sanitize::tagged_object_input_schema::<VideoAnalyzeParams>(),
        description = "Analyze visual behavior in a local video or YouTube URL (pass either in path) by delivering timestamped decoded frames directly to the calling assistant. Call order: call inspect first (inspect takes path only; start_seconds/end_seconds/frame_count/include_audio are rejected there), read the duration from its result, then call analyze or motion with start_seconds/end_seconds within that duration (analyze also accepts frame_count/include_audio). inspect returns video metadata; motion returns decoded-frame luma changes; analyze returns 1..12 inline JPEG images; text_timeline returns sampled OCR words/boxes/confidence from decoded video frames. Multiple samples span the requested start through the last timestamped video frame before end_seconds, while a single sample uses the start. Results include requested and decoded timestamps, image hashes, dimensions, and source-file identity. The assistant must inspect these images/text and produce the interpretation; this tool does not fabricate a semantic verdict. Sampled frames can miss brief events: narrow the range and repeat for detail. By default analyze also returns aligned mono WAV audio clips (include_audio=false disables them; audio_clip_seconds defaults to 3, range 0.1..10). Compare what is heard with what is visible, but lip-reading and hidden intent are uncertain. Requires FFmpeg/ffprobe; URLs additionally use python -m yt_dlp and cache the downloaded video under LOCALAPPDATA/synapse/video-cache. No screenshot calls, upload, or source-file modification."
    )]
    pub async fn videoanalyze(
        &self,
        params: Parameters<VideoAnalyzeParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let (path, sampling) = match params.0 {
            VideoAnalyzeParams::Inspect { path } => (path, None),
            VideoAnalyzeParams::Motion {
                path,
                start_seconds,
                end_seconds,
            } => {
                return motion_timeline(&path, start_seconds, end_seconds, &context).await;
            }
            VideoAnalyzeParams::TextTimeline {
                path,
                start_seconds,
                end_seconds,
                frame_count,
                max_long_edge,
                lang_hint,
                confidence_threshold,
                include_text_support,
            } => {
                return text_timeline(
                    &path,
                    start_seconds,
                    end_seconds,
                    frame_count,
                    max_long_edge,
                    lang_hint,
                    confidence_threshold,
                    include_text_support,
                    &context,
                )
                .await;
            }
            VideoAnalyzeParams::Analyze {
                path,
                start_seconds,
                end_seconds,
                frame_count,
                max_long_edge,
                include_audio,
                audio_clip_seconds,
            } => (
                path,
                Some((
                    start_seconds,
                    end_seconds,
                    frame_count,
                    max_long_edge,
                    include_audio,
                    audio_clip_seconds,
                )),
            ),
        };
        let source = path;
        let path = resolve_video(&source, &context).await?;
        let before = source_metadata(&path)?;
        let metadata = probe(&path, &context).await?;
        let mut content = Vec::new();
        let mut frames = Vec::new();
        let has_audio = metadata["streams"]
            .as_array()
            .is_some_and(|streams| streams.iter().any(|stream| stream["codec_type"] == "audio"));
        let mut audio_clips = Vec::new();
        if let Some((start, end, count, size, include_audio, audio_clip_seconds)) = sampling {
            let include_audio = include_audio.unwrap_or(true);
            let clip_seconds = audio_clip_seconds.unwrap_or(3.0);
            if !clip_seconds.is_finite() || !(0.1..=10.0).contains(&clip_seconds) {
                return Err(invalid("audio_clip_seconds must be 0.1..=10"));
            }
            let (start, end, count, size) =
                validate_sampling_range(&metadata, start, end, count, size, 12)?;
            let origin = video_origin(&metadata)?;
            let final_time = if count > 1 {
                final_sample_time(&path, start, end, &context).await?
            } else {
                start
            };
            for index in 0..count {
                let timestamp = sample_timestamp(index, count, start, final_time);
                // The decoder reports microsecond-rounded PTS; seek just before the
                // final PTS so rounding cannot skip the very frame we selected.
                let seek_time = endpoint_safe_seek_time(index, count, timestamp, start);
                let filter = format!(
                    "settb=expr=1/1000000,showinfo,scale=w='min({size},iw)':h='min({size},ih)':force_original_aspect_ratio=decrease"
                );
                let args = vec![
                    "-hide_banner".into(),
                    "-loglevel".into(),
                    "info".into(),
                    "-nostdin".into(),
                    "-ss".into(),
                    format!("{seek_time:.6}"),
                    "-copyts".into(),
                    "-i".into(),
                    path.clone(),
                    "-map".into(),
                    "0:v:0".into(),
                    "-frames:v".into(),
                    "1".into(),
                    "-vf".into(),
                    filter,
                    "-c:v".into(),
                    "mjpeg".into(),
                    "-q:v".into(),
                    "3".into(),
                    "-f".into(),
                    "image2pipe".into(),
                    "pipe:1".into(),
                ];
                let output = run_decoder("ffmpeg", &args, &context).await?;
                let decoded = image::load_from_memory(&output.stdout).map_err(|error| {
                    invalid(format!("no decodable frame at {timestamp}: {error}"))
                })?;
                let stderr = String::from_utf8_lossy(&output.stderr);
                let decoded_time = stderr
                    .lines()
                    .find_map(decoded_timestamp)
                    .ok_or_else(|| invalid("decoded video frame has no finite timestamp"))?;
                let relative_time = decoded_time - origin;
                if relative_time < start - 0.000_001 || relative_time >= end {
                    return Err(invalid("decoded frame is outside requested range"));
                }
                if count > 1 && index == count - 1 && (relative_time - final_time).abs() > 0.000_01
                {
                    return Err(invalid(
                        "decoded final frame does not match probed endpoint",
                    ));
                }
                let frame = json!({ "index": index, "requested_seconds": timestamp, "decoded_pts_seconds": decoded_time,
                    "width": decoded.width(), "height": decoded.height(), "image_sha256": Sha256::digest(&output.stdout).iter().map(|byte| format!("{byte:02x}")).collect::<String>() });
                content.push(Content::text(frame.to_string()));
                content.push(Content::image(
                    base64::engine::general_purpose::STANDARD.encode(&output.stdout),
                    "image/jpeg",
                ));
                frames.push(frame);
                if include_audio && has_audio {
                    let length = clip_seconds.min(end - timestamp);
                    let audio_args = vec![
                        "-hide_banner".into(),
                        "-loglevel".into(),
                        "error".into(),
                        "-nostdin".into(),
                        "-ss".into(),
                        format!("{timestamp:.6}"),
                        "-i".into(),
                        path.clone(),
                        "-t".into(),
                        format!("{length:.6}"),
                        "-map".into(),
                        "0:a:0".into(),
                        "-vn".into(),
                        "-ac".into(),
                        "1".into(),
                        "-ar".into(),
                        "16000".into(),
                        "-c:a".into(),
                        "pcm_s16le".into(),
                        "-f".into(),
                        "wav".into(),
                        "pipe:1".into(),
                    ];
                    let output = run_decoder("ffmpeg", &audio_args, &context).await?;
                    if output.stdout.len() <= 44 {
                        return Err(invalid("audio sample is empty"));
                    }
                    let clip = json!({"index": index, "start_seconds": timestamp, "duration_seconds": length,
                        "sample_rate": 16000, "channels": 1, "bytes": output.stdout.len()});
                    content.push(Content::text(clip.to_string()));
                    content.push(
                        RawContent::Audio(RawAudioContent {
                            data: base64::engine::general_purpose::STANDARD.encode(&output.stdout),
                            mime_type: "audio/wav".into(),
                        })
                        .no_annotation(),
                    );
                    audio_clips.push(clip);
                }
            }
        }
        let after = source_metadata(&path)?;
        if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
            return Err(invalid(
                "video changed during analysis; no mixed-version frames returned",
            ));
        }
        let result_metadata = json!({ "source_of_truth": "local video file + ffprobe metadata + FFmpeg decoded image bytes",
            "source": source, "path": path, "source_size_bytes": after.len(), "metadata": metadata, "frames": frames,
            "analysis_by": "calling_assistant", "audio_available": has_audio, "audio_clips": audio_clips,
            "sampling_can_miss_brief_events": true,
            "interpretation_limits": "Compare audible speech and visible motion cautiously. Sparse frames do not establish exact lip-read words, dubbing, deception, or hidden intent." });
        content.insert(0, Content::text(result_metadata.to_string()));
        let mut result = CallToolResult::success(content);
        result.structured_content = Some(result_metadata);
        Ok(result)
    }
}

#[cfg(test)]
#[path = "videoanalyze_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/videoanalyze_inspect_schema_115.rs"]
mod inspect_schema_115;

#[cfg(test)]
#[path = "tests/videoanalyze_missing_path_235.rs"]
mod missing_path_235;

#[cfg(test)]
#[path = "tests/video_cache_digest_174.rs"]
mod video_cache_digest_174_tests;

#[cfg(test)]
#[path = "tests/motion_stream_duration_113.rs"]
mod motion_stream_duration_113;
