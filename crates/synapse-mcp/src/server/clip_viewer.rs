//! Clip viewer: `screenshot operation=clip_overview | clip_frames | clip_frame
//! | clip_diff` (building plan Part 2, research section 8).
//!
//! Agents cannot watch video, so a recorded clip folder is read back as a
//! text timeline plus a few labelled PNG frames:
//!
//! - `clip_overview`: the change timeline and up to 8 key frames per page.
//! - `clip_frames`: frames in a time range (at most 8, at most 15 fps).
//! - `clip_frame`: one frame at native pixels, optional crop and 1-2x scale.
//! - `clip_diff`: changed regions between two times plus one before/after crop.
//!
//! Frame choice (plan "Frame choice"): `frames.jsonl` `new` flags skip repeated
//! frames without comparing them; new frames are compared with the picture on
//! screen by an 8x8 block test at native resolution. A change settles after
//! 200 ms without change; a change that returns to the previous picture
//! within 500 ms is short-lived. Picked: the last frame before and the first
//! settled frame after each change, the start of each short-lived change, one
//! frame every 10 s as a floor, and the first and last frames. The cursor
//! position is not recorded in clip format v1, so no cursor mask is applied.
//!
//! Every image is a labelled PNG copy under `<clip folder>\views\` (the clip
//! files stay untouched). All decoding and image work runs on a blocking
//! thread, never on the async worker.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use rmcp::model::{CallToolResult, Content};
use serde::{Deserialize, Serialize};
use serde_json::json;
use synapse_core::{Rect, error_codes};

use super::ErrorData;
use super::capture_clip::{
    CLIP_JSON_FILE, CLIP_SCHEMA_VERSION, FRAMES_FILE, FrameLine, VIDEO_FILE,
};
use super::clip_decoder::{ClipDecoder, DecodedFrame};
use crate::m1::clip_view_limits::{MAX_FRAMES_FPS, MAX_SCALE, MIN_CROP_SHORT_SIDE, MIN_SCALE};
use crate::m1::{
    ClipChangedRegion, ClipTimelineEntry, ClipViewImage, ClipViewParams, ClipViewResponse,
    ScreenshotOperation, ScreenshotResponse, mcp_error,
};

/// Images per call: Claude Code caps a tool result near 25,000 tokens.
const MAX_IMAGES_PER_CALL: usize = 8;
/// Default long edge of overview and range images (about 1,200 image tokens
/// each, so 8 images stay well inside the result cap).
const DEFAULT_LONG_EDGE: u32 = 1_280;
/// Long edge of the `clip_diff` before/after image.
const DIFF_LONG_EDGE: u32 = 1_568;
/// Largest `clip_frame` output; crop a smaller region instead.
const MAX_FRAME_OUTPUT_PIXELS: u64 = 3_840 * 2_160;

/// Side of the change-test blocks, in native clip pixels.
const BLOCK: usize = 8;
/// Luma difference that counts a pixel as changed (above encoder noise).
const PIXEL_DELTA: u8 = 32;
/// Changed pixels that mark an 8x8 block as changed. A one-line text edit
/// changes whole glyph strokes (dozens of high-contrast pixels per block).
const MIN_CHANGED_PIXELS: u32 = 3;
/// A change is settled after this long without further change.
const SETTLE_MS: u64 = 200;
/// A change that returns to the previous picture within this is short-lived.
const SHORT_LIVED_MS: u64 = 500;
/// At least one key frame per this interval.
const FLOOR_INTERVAL_MS: u64 = 10_000;
/// Unchanged stretches at least this long are listed in the timeline.
const STATIC_REPORT_MS: u64 = 1_000;
/// Padding around the changed area in the `clip_diff` crop.
const CROP_PADDING: i32 = 16;
const MAX_DIFF_REGIONS: usize = 20;

const VIEWS_DIR: &str = "views";
/// Cached overview analysis, so later pages skip the full decode.
const ANALYSIS_FILE: &str = "overview_analysis.json";
const ANALYSIS_VERSION: u32 = 1;

/// Runs a `clip_*` operation and builds the tool result: the response JSON as
/// structured content, then a text timeline, then `Image k` label + PNG pairs.
pub(super) async fn screenshot_clip_view_result(
    params: ClipViewParams,
) -> Result<CallToolResult, ErrorData> {
    let operation = params.operation;
    let folder = params.folder.clone();
    let output = tokio::task::spawn_blocking(move || run_view(&params))
        .await
        .unwrap_or_else(|error| Err(internal(format!("the clip viewer task failed: {error}"))))
        .map_err(|error| with_view_context(error, operation, &folder))?;
    let response = ScreenshotResponse {
        operation,
        source_of_truth: source_of_truth(&folder),
        capture: None,
        gif: None,
        record: None,
        clip_view: Some(output.response),
    };
    let value = serde_json::to_value(&response)
        .map_err(|error| internal(format!("could not serialise the clip view: {error}")))?;
    let mut content = vec![Content::text(output.text), Content::text(value.to_string())];
    let images = response.clip_view.iter().flat_map(|view| &view.images);
    for (image, png) in images.zip(output.pngs) {
        content.push(Content::text(format!("{} {}", image.label, image.path)));
        content.push(Content::image(
            base64::engine::general_purpose::STANDARD.encode(png),
            "image/png",
        ));
    }
    let mut result = CallToolResult::success(content);
    result.structured_content = Some(value);
    Ok(result)
}

struct ViewOutput {
    response: ClipViewResponse,
    text: String,
    pngs: Vec<Vec<u8>>,
}

fn run_view(params: &ClipViewParams) -> Result<ViewOutput, ErrorData> {
    let clip = ClipFolder::open(Path::new(&params.folder))?;
    let mut view = ClipView::new(clip);
    match params.operation {
        ScreenshotOperation::ClipOverview => view.overview(params)?,
        ScreenshotOperation::ClipFrames => view.frames(params)?,
        ScreenshotOperation::ClipFrame => view.frame(params)?,
        ScreenshotOperation::ClipDiff => view.diff(params)?,
        other => {
            return Err(internal(format!(
                "operation {other:?} is not a clip viewer operation"
            )));
        }
    }
    Ok(view.finish())
}

// ---------------------------------------------------------------------------
// Clip folder
// ---------------------------------------------------------------------------

/// The parts of `clip.json` the viewer needs (written by `capture_clip`).
#[derive(Deserialize)]
struct ManifestView {
    schema_version: u32,
    clip_id: String,
    encode: EncodeView,
    health: HealthView,
}

#[derive(Deserialize)]
struct EncodeView {
    width: u32,
    height: u32,
    fps: u32,
}

#[derive(Deserialize)]
struct HealthView {
    frames_new: u64,
    #[serde(default)]
    minimized_intervals: Vec<MinimizedView>,
    #[serde(default)]
    stopped_reason: String,
}

#[derive(Deserialize)]
struct MinimizedView {
    start_frame: u64,
    end_frame: u64,
    start_ms: u64,
    end_ms: u64,
}

/// A recorded clip folder: manifest, per-frame flags and the video path.
struct ClipFolder {
    folder: PathBuf,
    manifest: ManifestView,
    frames: Vec<FrameLine>,
}

impl ClipFolder {
    fn open(folder: &Path) -> Result<Self, ErrorData> {
        let read = |name: &str| {
            std::fs::read_to_string(folder.join(name)).map_err(|error| {
                params_invalid(format!(
                    "{folder:?} is not a readable clip folder: {name}: {error}"
                ))
            })
        };
        let manifest: ManifestView = serde_json::from_str(&read(CLIP_JSON_FILE)?)
            .map_err(|error| params_invalid(format!("{CLIP_JSON_FILE} is not valid: {error}")))?;
        if manifest.schema_version != CLIP_SCHEMA_VERSION {
            return Err(params_invalid(format!(
                "{CLIP_JSON_FILE} schema_version {} is not supported (expected {CLIP_SCHEMA_VERSION})",
                manifest.schema_version
            )));
        }
        if manifest.encode.fps == 0 || manifest.encode.width == 0 || manifest.encode.height == 0 {
            return Err(params_invalid(format!(
                "{CLIP_JSON_FILE} has an empty encode size or fps"
            )));
        }
        let frames = read(FRAMES_FILE)?
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(serde_json::from_str::<FrameLine>)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| params_invalid(format!("{FRAMES_FILE} is not valid: {error}")))?;
        if frames.is_empty() {
            return Err(params_invalid(format!("{FRAMES_FILE} lists no frames")));
        }
        if !folder.join(VIDEO_FILE).is_file() {
            return Err(params_invalid(format!(
                "{VIDEO_FILE} is missing in {folder:?}"
            )));
        }
        Ok(Self {
            folder: folder.to_path_buf(),
            manifest,
            frames,
        })
    }

    fn fps(&self) -> u64 {
        u64::from(self.manifest.encode.fps)
    }

    fn frame_ms(&self) -> u64 {
        (1_000 / self.fps()).max(1)
    }

    fn last_frame(&self) -> u64 {
        self.frames.len() as u64 - 1
    }

    /// Time of a frame on the recorder's constant-rate timeline.
    fn t_of(&self, frame: u64) -> u64 {
        frame * 1_000 / self.fps()
    }

    /// The frame on screen at `t_ms`.
    fn frame_at_ms(&self, t_ms: u64) -> u64 {
        (t_ms * self.fps() / 1_000).min(self.last_frame())
    }

    fn frame_of_sample(&self, t_hns: i64) -> u64 {
        let t_hns = u64::try_from(t_hns).unwrap_or(0);
        ((t_hns * self.fps() + 5_000_000) / 10_000_000).min(self.last_frame())
    }

    fn is_new(&self, frame: u64) -> bool {
        usize::try_from(frame)
            .ok()
            .and_then(|index| self.frames.get(index))
            .is_none_or(|line| line.new)
    }

    /// `true` when no new frame arrived in `(from, to]`: same picture.
    fn same_picture(&self, from: u64, to: u64) -> bool {
        (from + 1..=to).all(|frame| !self.is_new(frame))
    }

    fn duration_ms(&self) -> u64 {
        self.t_of(self.frames.len() as u64)
    }

    fn open_decoder(&self) -> Result<ClipDecoder, ErrorData> {
        ClipDecoder::open(&self.folder.join(VIDEO_FILE))
            .map_err(|error| internal(format!("could not open {VIDEO_FILE}: {error}")))
    }

    fn views_dir(&self) -> Result<PathBuf, ErrorData> {
        let views = self.folder.join(VIEWS_DIR);
        std::fs::create_dir_all(&views)
            .map_err(|error| internal(format!("could not create {views:?}: {error}")))?;
        Ok(views)
    }
}

// ---------------------------------------------------------------------------
// Change test
// ---------------------------------------------------------------------------

/// BT.601 luma of a BGRA frame, one byte per pixel.
fn luma(bgra: &[u8]) -> Vec<u8> {
    bgra.chunks_exact(4)
        .map(|px| {
            let value =
                (29 * u32::from(px[0]) + 150 * u32::from(px[1]) + 77 * u32::from(px[2])) >> 8;
            u8::try_from(value).unwrap_or(u8::MAX)
        })
        .collect()
}

/// Which 8x8 blocks differ between two pictures of the same size.
#[derive(Clone)]
struct BlockMap {
    cols: usize,
    rows: usize,
    width: usize,
    height: usize,
    changed: Vec<bool>,
    count: u64,
}

impl BlockMap {
    fn empty(width: usize, height: usize) -> Self {
        let (cols, rows) = (width.div_ceil(BLOCK), height.div_ceil(BLOCK));
        Self {
            cols,
            rows,
            width,
            height,
            changed: vec![false; cols * rows],
            count: 0,
        }
    }

    fn between(before: &[u8], after: &[u8], width: usize, height: usize) -> Self {
        let mut map = Self::empty(width, height);
        let mut counts = vec![0_u32; map.cols * map.rows];
        for y in 0..height {
            let row = y * width;
            let block_row = (y / BLOCK) * map.cols;
            let (Some(a), Some(b)) = (before.get(row..row + width), after.get(row..row + width))
            else {
                break;
            };
            for (x, (pa, pb)) in a.iter().zip(b).enumerate() {
                if pa.abs_diff(*pb) > PIXEL_DELTA {
                    counts[block_row + x / BLOCK] += 1;
                }
            }
        }
        for (flag, count) in map.changed.iter_mut().zip(counts) {
            *flag = count >= MIN_CHANGED_PIXELS;
        }
        map.count = map.changed.iter().filter(|flag| **flag).count() as u64;
        map
    }

    fn merge(&mut self, other: &Self) {
        for (flag, more) in self.changed.iter_mut().zip(&other.changed) {
            *flag |= *more;
        }
        self.count = self.changed.iter().filter(|flag| **flag).count() as u64;
    }

    fn percent(&self) -> f64 {
        (self.count as f64 * 1_000.0 / self.changed.len().max(1) as f64).round() / 10.0
    }

    fn block_rect(&self, col0: usize, row0: usize, col1: usize, row1: usize) -> Rect {
        let x = col0 * BLOCK;
        let y = row0 * BLOCK;
        let w = ((col1 + 1) * BLOCK).min(self.width) - x;
        let h = ((row1 + 1) * BLOCK).min(self.height) - y;
        rect(x, y, w, h)
    }

    fn bbox(&self) -> Option<Rect> {
        let mut bounds: Option<(usize, usize, usize, usize)> = None;
        for (index, _) in self.changed.iter().enumerate().filter(|(_, flag)| **flag) {
            let (col, row) = (index % self.cols, index / self.cols);
            bounds = Some(bounds.map_or((col, row, col, row), |(c0, r0, c1, r1)| {
                (c0.min(col), r0.min(row), c1.max(col), r1.max(row))
            }));
        }
        bounds.map(|(c0, r0, c1, r1)| self.block_rect(c0, r0, c1, r1))
    }

    /// Connected groups of changed blocks (8-neighbour), largest first.
    fn regions(&self) -> Vec<ClipChangedRegion> {
        let mut seen = vec![false; self.changed.len()];
        let mut regions = Vec::new();
        for start in 0..self.changed.len() {
            if !self.changed[start] || seen[start] {
                continue;
            }
            seen[start] = true;
            let mut stack = vec![start];
            let (mut c0, mut r0, mut c1, mut r1) = (usize::MAX, usize::MAX, 0, 0);
            let mut blocks = 0_u64;
            while let Some(index) = stack.pop() {
                let (col, row) = (index % self.cols, index / self.cols);
                (c0, r0, c1, r1) = (c0.min(col), r0.min(row), c1.max(col), r1.max(row));
                blocks += 1;
                for next_row in row.saturating_sub(1)..=(row + 1).min(self.rows - 1) {
                    for next_col in col.saturating_sub(1)..=(col + 1).min(self.cols - 1) {
                        let next = next_row * self.cols + next_col;
                        if self.changed[next] && !seen[next] {
                            seen[next] = true;
                            stack.push(next);
                        }
                    }
                }
            }
            regions.push(ClipChangedRegion {
                rect: self.block_rect(c0, r0, c1, r1),
                changed_blocks: blocks,
            });
        }
        regions.sort_by_key(|region| std::cmp::Reverse(region.changed_blocks));
        regions
    }
}

// ---------------------------------------------------------------------------
// Overview analysis
// ---------------------------------------------------------------------------

/// One change: consecutive changed frames until the picture settles.
struct Episode {
    start_frame: u64,
    last_change_frame: u64,
    blocks: BlockMap,
    /// Luma before the change (kept only while a revert can still follow).
    before: Vec<u8>,
    short_lived: bool,
    reverted: bool,
    settled: bool,
}

#[derive(Clone, Serialize, Deserialize)]
struct Pick {
    frame: u64,
    t_ms: u64,
    reason: String,
}

/// Overview result, cached as `views/overview_analysis.json`.
#[derive(Serialize, Deserialize)]
struct Analysis {
    version: u32,
    timeline: Vec<ClipTimelineEntry>,
    picks: Vec<Pick>,
    notes: Vec<String>,
}

/// One decode pass over the clip that finds the changes.
struct Analyzer<'a> {
    clip: &'a ClipFolder,
    width: usize,
    height: usize,
    shown: Vec<u8>,
    open: Option<Episode>,
    done: Vec<Episode>,
    decoded: u64,
    new_without_change: u64,
}

impl<'a> Analyzer<'a> {
    fn run(clip: &'a ClipFolder) -> Result<Analysis, ErrorData> {
        let mut decoder = clip.open_decoder()?;
        let first = decoder
            .next_frame()
            .map_err(decode_error)?
            .ok_or_else(|| internal(format!("{VIDEO_FILE} holds no frames")))?;
        let mut analyzer = Self {
            clip,
            width: first.width as usize,
            height: first.height as usize,
            shown: luma(&first.bgra),
            open: None,
            done: Vec::new(),
            decoded: 1,
            new_without_change: 0,
        };
        while let Some(frame) = decoder.next_frame().map_err(decode_error)? {
            analyzer.decoded += 1;
            analyzer.step(&frame);
        }
        if let Some(episode) = analyzer.open.take() {
            analyzer.close(episode, false);
        }
        Ok(analyzer.finish())
    }

    fn step(&mut self, frame: &DecodedFrame) {
        let number = self.clip.frame_of_sample(frame.t_hns);
        let t_ms = self.clip.t_of(number);
        let settled = self.open.as_ref().is_some_and(|episode| {
            t_ms.saturating_sub(self.clip.t_of(episode.last_change_frame)) >= SETTLE_MS
        });
        if settled && let Some(episode) = self.open.take() {
            self.close(episode, true);
        }
        if number == 0 || !self.clip.is_new(number) || frame.width as usize != self.width {
            return;
        }
        let current = luma(&frame.bgra);
        let blocks = BlockMap::between(&self.shown, &current, self.width, self.height);
        if blocks.count == 0 {
            self.new_without_change += 1;
            return;
        }
        let before = std::mem::replace(&mut self.shown, current);
        let (width, height) = (self.width, self.height);
        let episode = self.open.get_or_insert_with(|| Episode {
            start_frame: number,
            last_change_frame: number,
            blocks: BlockMap::empty(width, height),
            before,
            short_lived: false,
            reverted: false,
            settled: false,
        });
        episode.last_change_frame = number;
        episode.blocks.merge(&blocks);
    }

    /// Ends a change; marks it short-lived when the picture is back to the
    /// one before it within `SHORT_LIVED_MS`, also when the return came as a
    /// separate change right after.
    fn close(&mut self, mut episode: Episode, settled: bool) {
        let (width, height) = (self.width, self.height);
        let span = |clip: &ClipFolder, start: u64, end: u64| clip.t_of(end) - clip.t_of(start);
        episode.settled = settled;
        if BlockMap::between(&episode.before, &self.shown, width, height).count == 0 {
            episode.reverted = true;
            episode.short_lived =
                span(self.clip, episode.start_frame, episode.last_change_frame) <= SHORT_LIVED_MS;
        }
        if !episode.reverted
            && let Some(previous) = self.done.last_mut()
            && !previous.reverted
            && span(self.clip, previous.start_frame, episode.last_change_frame) <= SHORT_LIVED_MS
            && BlockMap::between(&previous.before, &self.shown, width, height).count == 0
        {
            previous.reverted = true;
            previous.short_lived = true;
            previous.settled = settled;
            previous.last_change_frame = episode.last_change_frame;
            previous.blocks.merge(&episode.blocks);
            previous.before = Vec::new();
            return;
        }
        if let Some(previous) = self.done.last_mut() {
            previous.before = Vec::new();
        }
        if episode.reverted {
            episode.before = Vec::new();
        }
        self.done.push(episode);
    }

    fn finish(self) -> Analysis {
        let clip = self.clip;
        let mut picks = vec![pick(clip, 0, "first frame")];
        let mut timeline = Vec::new();
        let mut quiet_from = 0_u64;
        for episode in &self.done {
            let start = episode.start_frame;
            let end = episode.last_change_frame;
            if clip.t_of(start).saturating_sub(clip.t_of(quiet_from)) >= STATIC_REPORT_MS {
                timeline.push(static_entry(clip, quiet_from, start.saturating_sub(1)));
            }
            quiet_from = end;
            picks.push(pick(
                clip,
                start.saturating_sub(1),
                "last frame before change",
            ));
            if episode.short_lived {
                picks.push(pick(clip, start, "short-lived change start"));
            } else {
                picks.push(pick(clip, end, "first settled frame after change"));
            }
            timeline.push(episode_entry(clip, episode));
        }
        let last = clip.last_frame();
        if clip.t_of(last).saturating_sub(clip.t_of(quiet_from)) >= STATIC_REPORT_MS {
            timeline.push(static_entry(clip, quiet_from, last));
        }
        for interval in &clip.manifest.health.minimized_intervals {
            timeline.push(ClipTimelineEntry {
                kind: "minimized".to_owned(),
                start_ms: interval.start_ms,
                end_ms: interval.end_ms,
                start_frame: interval.start_frame,
                end_frame: interval.end_frame.saturating_sub(1),
                changed_blocks: 0,
                bbox: None,
                text: format!(
                    "{}-{} gap: window minimized, the last picture was repeated",
                    fmt_time(interval.start_ms),
                    fmt_time(interval.end_ms)
                ),
            });
        }
        timeline.sort_by_key(|entry| (entry.start_ms, entry.end_ms));
        picks.push(pick(clip, last, "last frame"));
        let picks = with_floor(clip, merge_picks(picks));

        let repeated = clip.frames.len() as u64
            - clip
                .manifest
                .health
                .frames_new
                .min(clip.frames.len() as u64);
        let mut notes = vec![
            format!(
                "{} changes found; {repeated} repeated frames skipped by frames.jsonl flags; {} new frames showed no visible change",
                self.done.len(),
                self.new_without_change
            ),
            "change test: 8x8 blocks at native resolution, luma; settled after 200 ms, short-lived = back within 500 ms".to_owned(),
            "no cursor mask: clip format v1 does not record the cursor position, so cursor moves count as changes".to_owned(),
        ];
        if self.decoded < clip.frames.len() as u64 {
            notes.push(format!(
                "gap: {} of {} frames decoded; the rest could not be read from {VIDEO_FILE}",
                self.decoded,
                clip.frames.len()
            ));
        }
        if self.done.last().is_some_and(|episode| !episode.settled) {
            notes.push("the last change had not settled when the clip ended".to_owned());
        }
        let stopped = &clip.manifest.health.stopped_reason;
        if !stopped.is_empty() && stopped != "completed" {
            notes.push(format!("the recording stopped early: {stopped}"));
        }
        Analysis {
            version: ANALYSIS_VERSION,
            timeline,
            picks,
            notes,
        }
    }
}

fn pick(clip: &ClipFolder, frame: u64, reason: &str) -> Pick {
    Pick {
        frame,
        t_ms: clip.t_of(frame),
        reason: reason.to_owned(),
    }
}

/// Sorts picks by frame and joins the reasons of picks on the same frame.
fn merge_picks(mut picks: Vec<Pick>) -> Vec<Pick> {
    picks.sort_by_key(|pick| pick.frame);
    let mut merged: Vec<Pick> = Vec::with_capacity(picks.len());
    for pick in picks {
        match merged.last_mut() {
            Some(last) if last.frame == pick.frame => {
                if !last.reason.contains(&pick.reason) {
                    last.reason = format!("{} + {}", last.reason, pick.reason);
                }
            }
            _ => merged.push(pick),
        }
    }
    merged
}

/// Adds a frame wherever two picks are more than `FLOOR_INTERVAL_MS` apart.
fn with_floor(clip: &ClipFolder, picks: Vec<Pick>) -> Vec<Pick> {
    let mut out: Vec<Pick> = Vec::with_capacity(picks.len());
    for next in picks {
        while let Some(last) = out.last() {
            let floor_t = last.t_ms + FLOOR_INTERVAL_MS;
            if next.t_ms <= floor_t {
                break;
            }
            let frame = clip.frame_at_ms(floor_t);
            if frame <= last.frame {
                break;
            }
            out.push(pick(clip, frame, "10 s floor"));
        }
        out.push(next);
    }
    out
}

fn static_entry(clip: &ClipFolder, from: u64, to: u64) -> ClipTimelineEntry {
    let (start_ms, end_ms) = (clip.t_of(from), clip.t_of(to));
    ClipTimelineEntry {
        kind: "static".to_owned(),
        start_ms,
        end_ms,
        start_frame: from,
        end_frame: to,
        changed_blocks: 0,
        bbox: None,
        text: format!(
            "{}-{} no change ({:.1} s)",
            fmt_time(start_ms),
            fmt_time(end_ms),
            (end_ms - start_ms) as f64 / 1_000.0
        ),
    }
}

fn episode_entry(clip: &ClipFolder, episode: &Episode) -> ClipTimelineEntry {
    let (start_ms, end_ms) = (
        clip.t_of(episode.start_frame),
        clip.t_of(episode.last_change_frame),
    );
    let bbox = episode.blocks.bbox();
    let area = bbox.map_or_else(String::new, |r| {
        format!(" in x={} y={} w={} h={}", r.x, r.y, r.w, r.h)
    });
    let outcome = if episode.short_lived {
        format!(
            "short-lived: back to the previous picture at {}",
            fmt_time(end_ms)
        )
    } else if episode.reverted {
        format!("returned to the previous picture at {}", fmt_time(end_ms))
    } else if episode.settled {
        format!(
            "settled at {} (frame {})",
            fmt_time(end_ms),
            episode.last_change_frame
        )
    } else {
        "still changing when the clip ended".to_owned()
    };
    ClipTimelineEntry {
        kind: if episode.short_lived {
            "short_lived"
        } else {
            "change"
        }
        .to_owned(),
        start_ms,
        end_ms,
        start_frame: episode.start_frame,
        end_frame: episode.last_change_frame,
        changed_blocks: episode.blocks.count,
        bbox,
        text: format!(
            "{}-{} change: {} blocks ({}% of the picture){area}; {outcome}",
            fmt_time(start_ms),
            fmt_time(end_ms),
            episode.blocks.count,
            episode.blocks.percent()
        ),
    }
}

// ---------------------------------------------------------------------------
// The four operations
// ---------------------------------------------------------------------------

/// One viewer call: the clip, its decoder and the images made so far.
struct ClipView {
    clip: ClipFolder,
    decoder: Option<ClipDecoder>,
    images: Vec<ClipViewImage>,
    pngs: Vec<Vec<u8>>,
    timeline: Vec<ClipTimelineEntry>,
    regions: Vec<ClipChangedRegion>,
    paging: Option<(u32, u32)>,
    notes: Vec<String>,
    text: String,
}

impl ClipView {
    fn new(clip: ClipFolder) -> Self {
        Self {
            clip,
            decoder: None,
            images: Vec::new(),
            pngs: Vec::new(),
            timeline: Vec::new(),
            regions: Vec::new(),
            paging: None,
            notes: Vec::new(),
            text: String::new(),
        }
    }

    fn overview(&mut self, params: &ClipViewParams) -> Result<(), ErrorData> {
        let analysis = self.analysis()?;
        let pages = u32::try_from(analysis.picks.len().div_ceil(MAX_IMAGES_PER_CALL))
            .unwrap_or(u32::MAX)
            .max(1);
        let page = params.page.unwrap_or(1);
        if page == 0 || page > pages {
            return Err(params_invalid(format!(
                "page {page} is outside 1..={pages} for this clip"
            )));
        }
        let first = (page as usize - 1) * MAX_IMAGES_PER_CALL;
        let picks: Vec<Pick> = analysis
            .picks
            .iter()
            .skip(first)
            .take(MAX_IMAGES_PER_CALL)
            .cloned()
            .collect();
        let long_edge = params.max_long_edge.unwrap_or(DEFAULT_LONG_EDGE);
        let _ = writeln!(
            self.text,
            "Clip {} timeline ({} key frames, page {page}/{pages}):",
            self.clip.manifest.clip_id,
            analysis.picks.len()
        );
        for entry in &analysis.timeline {
            let _ = writeln!(self.text, "- {}", entry.text);
        }
        for (offset, pick) in picks.iter().enumerate() {
            let index = u32::try_from(first + offset + 1).unwrap_or(u32::MAX);
            let frame = self.decode(pick.frame)?;
            let image = fit(to_rgba(&frame), long_edge);
            let stem = format!("overview_{index:02}_f{}", pick.frame);
            self.export(image, index, pick.frame, &pick.reason, &stem)?;
        }
        self.timeline = analysis.timeline;
        self.notes = analysis.notes;
        self.paging = Some((page, pages));
        Ok(())
    }

    fn frames(&mut self, params: &ClipViewParams) -> Result<(), ErrorData> {
        let (start, end) = self.range(params)?;
        let fps = params
            .fps
            .unwrap_or(MAX_FRAMES_FPS)
            .clamp(1, MAX_FRAMES_FPS);
        let min_step = 1_000_u64.div_ceil(u64::from(fps));
        let fit_step = (end - start).div_ceil(MAX_IMAGES_PER_CALL as u64 - 1);
        let step = min_step.max(fit_step).max(1);
        if step > min_step {
            self.notes.push(format!(
                "sampled every {step} ms so the range fits {MAX_IMAGES_PER_CALL} images; narrow the range for more detail"
            ));
        }
        let long_edge = params.max_long_edge.unwrap_or(DEFAULT_LONG_EDGE);
        let _ = writeln!(
            self.text,
            "Clip {} frames {}-{} every {step} ms:",
            self.clip.manifest.clip_id,
            fmt_time(start),
            fmt_time(end)
        );
        let mut previous: Option<(u64, u32)> = None;
        let mut t_ms = start;
        while t_ms <= end && self.images.len() < MAX_IMAGES_PER_CALL {
            let number = self.clip.frame_at_ms(t_ms);
            t_ms += step;
            if let Some((last, index)) = previous {
                if number == last {
                    continue;
                }
                if self.clip.same_picture(last, number) {
                    let _ = writeln!(
                        self.text,
                        "- {} frame {number}: repeat, same picture as Image {index}",
                        fmt_time(self.clip.t_of(number))
                    );
                    continue;
                }
            }
            let index = u32::try_from(self.images.len() + 1).unwrap_or(u32::MAX);
            let frame = self.decode(number)?;
            let image = fit(to_rgba(&frame), long_edge);
            let stem = format!("frames_{start}-{end}_{index:02}_f{number}");
            self.export(image, index, number, "range sample", &stem)?;
            previous = Some((number, index));
        }
        Ok(())
    }

    fn frame(&mut self, params: &ClipViewParams) -> Result<(), ErrorData> {
        let t_ms = params.t_ms.unwrap_or(0);
        let last_t = self.clip.t_of(self.clip.last_frame());
        if t_ms > last_t {
            return Err(params_invalid(format!(
                "t_ms {t_ms} is after the last frame ({last_t} ms)"
            )));
        }
        let number = self.clip.frame_at_ms(t_ms);
        let frame = self.decode(number)?;
        let mut image = to_rgba(&frame);
        let mut stem = format!("frame_f{number}");
        if let Some(region) = params.region {
            let crop = clamp_rect(region, image.width(), image.height())?;
            let short = crop.w.min(crop.h);
            let possible = MIN_CROP_SHORT_SIDE.min(image.width()).min(image.height());
            if u32::try_from(short).unwrap_or(0) < possible {
                return Err(params_invalid(format!(
                    "the crop inside the {}x{} frame is {}x{}; its short side must be at least {MIN_CROP_SHORT_SIDE} px",
                    image.width(),
                    image.height(),
                    crop.w,
                    crop.h
                )));
            }
            image = crop_image(&image, crop);
            let _ = write!(stem, "_x{}_y{}_w{}_h{}", crop.x, crop.y, crop.w, crop.h);
        }
        let scale = params
            .scale
            .unwrap_or(MIN_SCALE)
            .clamp(MIN_SCALE, MAX_SCALE);
        let (width, height) = (scaled(image.width(), scale), scaled(image.height(), scale));
        if u64::from(width) * u64::from(height) > MAX_FRAME_OUTPUT_PIXELS {
            return Err(params_invalid(format!(
                "a {width}x{height} image is larger than {MAX_FRAME_OUTPUT_PIXELS} pixels; pass a smaller region or scale"
            )));
        }
        if (width, height) != image.dimensions() {
            image = image::imageops::resize(
                &image,
                width,
                height,
                image::imageops::FilterType::Triangle,
            );
            let _ = write!(stem, "_s{}", (scale * 100.0).round());
        }
        let _ = writeln!(
            self.text,
            "Clip {} frame {number} at {} ({}x{} px, scale {scale}):",
            self.clip.manifest.clip_id,
            fmt_time(self.clip.t_of(number)),
            width,
            height
        );
        let reason = if params.region.is_some() {
            "crop"
        } else {
            "native"
        };
        self.export(image, 1, number, reason, &stem)
    }

    fn diff(&mut self, params: &ClipViewParams) -> Result<(), ErrorData> {
        let (start, end) = self.range(params)?;
        let (from, to) = (self.clip.frame_at_ms(start), self.clip.frame_at_ms(end));
        let _ = writeln!(
            self.text,
            "Clip {} diff {} (frame {from}) -> {} (frame {to}):",
            self.clip.manifest.clip_id,
            fmt_time(self.clip.t_of(from)),
            fmt_time(self.clip.t_of(to))
        );
        if from == to || self.clip.same_picture(from, to) {
            let _ = writeln!(self.text, "- no new frame between the two times: no change");
            return Ok(());
        }
        let before = self.decode(from)?;
        let after = self.decode(to)?;
        if (before.width, before.height) != (after.width, after.height) {
            return Err(internal(
                "the two frames decode at different sizes".to_owned(),
            ));
        }
        let (width, height) = (before.width as usize, before.height as usize);
        let blocks = BlockMap::between(&luma(&before.bgra), &luma(&after.bgra), width, height);
        let mut regions = blocks.regions();
        let _ = writeln!(
            self.text,
            "- {} changed blocks ({}% of the picture) in {} regions",
            blocks.count,
            blocks.percent(),
            regions.len()
        );
        if regions.len() > MAX_DIFF_REGIONS {
            self.notes.push(format!(
                "{} regions found; the {MAX_DIFF_REGIONS} largest are listed",
                regions.len()
            ));
            regions.truncate(MAX_DIFF_REGIONS);
        }
        for region in &regions {
            let r = region.rect;
            let _ = writeln!(
                self.text,
                "- x={} y={} w={} h={} ({} blocks)",
                r.x, r.y, r.w, r.h, region.changed_blocks
            );
        }
        let Some(bbox) = blocks.bbox() else {
            let _ = writeln!(self.text, "- no visible change (below the block threshold)");
            return Ok(());
        };
        let crop = diff_crop(bbox, before.width, before.height);
        let mut left = crop_image(&to_rgba(&before), crop);
        let mut right = crop_image(&to_rgba(&after), crop);
        for region in &regions {
            outline(&mut left, region.rect, crop);
            outline(&mut right, region.rect, crop);
        }
        let image = fit(side_by_side(&left, &right), DIFF_LONG_EDGE);
        let reason = format!(
            "before {} / after {}, crop x={} y={} w={} h={}",
            fmt_time(self.clip.t_of(from)),
            fmt_time(self.clip.t_of(to)),
            crop.x,
            crop.y,
            crop.w,
            crop.h
        );
        self.regions = regions;
        self.export(image, 1, to, &reason, &format!("diff_f{from}-f{to}"))
    }

    /// `start_ms`..=`end_ms`, checked against the clip.
    fn range(&self, params: &ClipViewParams) -> Result<(u64, u64), ErrorData> {
        let (start, end) = (params.start_ms.unwrap_or(0), params.end_ms.unwrap_or(0));
        let last_t = self.clip.t_of(self.clip.last_frame());
        if start >= end {
            return Err(params_invalid(format!(
                "start_ms {start} must be before end_ms {end}"
            )));
        }
        if start > last_t {
            return Err(params_invalid(format!(
                "start_ms {start} is after the last frame ({last_t} ms)"
            )));
        }
        Ok((start, end.min(last_t)))
    }

    fn analysis(&self) -> Result<Analysis, ErrorData> {
        let cache = self.clip.views_dir()?.join(ANALYSIS_FILE);
        if let Some(analysis) = std::fs::read(&cache)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Analysis>(&bytes).ok())
            .filter(|analysis| analysis.version == ANALYSIS_VERSION)
        {
            return Ok(analysis);
        }
        let analysis = Analyzer::run(&self.clip)?;
        if let Ok(bytes) = serde_json::to_vec_pretty(&analysis) {
            let _ = std::fs::write(&cache, bytes);
        }
        Ok(analysis)
    }

    fn decode(&mut self, frame: u64) -> Result<DecodedFrame, ErrorData> {
        if self.decoder.is_none() {
            self.decoder = Some(self.clip.open_decoder()?);
        }
        let frame_ms = self.clip.frame_ms();
        let t_ms = self.clip.t_of(frame);
        self.decoder
            .as_mut()
            .map_or_else(
                || Err("decoder unavailable".to_owned()),
                |decoder| decoder.frame_at(t_ms, frame_ms),
            )
            .map_err(decode_error)
    }

    /// Labels the image (text and drawn copy), writes the PNG under `views/`
    /// and keeps it for the result.
    fn export(
        &mut self,
        mut image: image::RgbaImage,
        index: u32,
        frame: u64,
        reason: &str,
        stem: &str,
    ) -> Result<(), ErrorData> {
        let t_ms = self.clip.t_of(frame);
        let label = format!(
            "Image {index} (t={}, frame {frame}, {reason})",
            fmt_time(t_ms)
        );
        draw_label(&mut image, &format!("#{index} {} F{frame}", fmt_time(t_ms)));
        let path = self.clip.views_dir()?.join(format!("{stem}.png"));
        let rgb = image::DynamicImage::ImageRgba8(image).to_rgb8();
        let mut png = Vec::new();
        rgb.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .map_err(|error| internal(format!("PNG encode failed: {error}")))?;
        std::fs::write(&path, &png)
            .map_err(|error| internal(format!("could not write {path:?}: {error}")))?;
        let _ = writeln!(self.text, "{label}: {}", path.display());
        self.images.push(ClipViewImage {
            index,
            t_ms,
            frame,
            reason: reason.to_owned(),
            label,
            path: path.to_string_lossy().into_owned(),
            width: rgb.width(),
            height: rgb.height(),
        });
        self.pngs.push(png);
        Ok(())
    }

    fn finish(mut self) -> ViewOutput {
        for note in &self.notes {
            let _ = writeln!(self.text, "Note: {note}");
        }
        let (page, pages) = self.paging.unzip();
        let next_page = self
            .paging
            .and_then(|(page, pages)| (page < pages).then_some(page + 1));
        if let Some(next) = next_page {
            let _ = writeln!(self.text, "More key frames: call again with page={next}.");
        }
        let manifest = &self.clip.manifest;
        ViewOutput {
            response: ClipViewResponse {
                clip_id: manifest.clip_id.clone(),
                folder: self.clip.folder.to_string_lossy().into_owned(),
                duration_ms: self.clip.duration_ms(),
                width: manifest.encode.width,
                height: manifest.encode.height,
                fps: manifest.encode.fps,
                frames_total: self.clip.frames.len() as u64,
                frames_new: manifest.health.frames_new,
                timeline: self.timeline,
                images: self.images,
                regions: self.regions,
                page,
                pages,
                next_page,
                notes: self.notes,
            },
            text: self.text,
            pngs: self.pngs,
        }
    }
}

// ---------------------------------------------------------------------------
// Image helpers
// ---------------------------------------------------------------------------

fn to_rgba(frame: &DecodedFrame) -> image::RgbaImage {
    let mut rgba = frame.bgra.clone();
    for px in rgba.chunks_exact_mut(4) {
        px.swap(0, 2);
        px[3] = u8::MAX;
    }
    image::RgbaImage::from_raw(frame.width, frame.height, rgba)
        .unwrap_or_else(|| image::RgbaImage::new(frame.width.max(1), frame.height.max(1)))
}

/// Downscales (never upscales) so the long edge fits `long_edge`.
fn fit(image: image::RgbaImage, long_edge: u32) -> image::RgbaImage {
    let long = image.width().max(image.height());
    if long <= long_edge {
        return image;
    }
    let scale = f64::from(long_edge) / f64::from(long);
    let (width, height) = (scaled(image.width(), scale), scaled(image.height(), scale));
    image::imageops::resize(&image, width, height, image::imageops::FilterType::Triangle)
}

fn scaled(value: u32, scale: f64) -> u32 {
    ((f64::from(value) * scale).round() as u32).max(1)
}

fn rect(x: usize, y: usize, w: usize, h: usize) -> Rect {
    let to_i32 = |value: usize| i32::try_from(value).unwrap_or(i32::MAX);
    Rect {
        x: to_i32(x),
        y: to_i32(y),
        w: to_i32(w),
        h: to_i32(h),
    }
}

/// The part of `region` inside a `width`x`height` picture.
fn clamp_rect(region: Rect, width: u32, height: u32) -> Result<Rect, ErrorData> {
    let (width, height) = (
        i32::try_from(width).unwrap_or(i32::MAX),
        i32::try_from(height).unwrap_or(i32::MAX),
    );
    let x0 = region.x.clamp(0, width);
    let y0 = region.y.clamp(0, height);
    let x1 = region.x.saturating_add(region.w).clamp(0, width);
    let y1 = region.y.saturating_add(region.h).clamp(0, height);
    if x1 <= x0 || y1 <= y0 {
        return Err(params_invalid(format!(
            "region x={} y={} w={} h={} lies outside the {width}x{height} frame",
            region.x, region.y, region.w, region.h
        )));
    }
    Ok(Rect {
        x: x0,
        y: y0,
        w: x1 - x0,
        h: y1 - y0,
    })
}

fn crop_image(image: &image::RgbaImage, crop: Rect) -> image::RgbaImage {
    let to_u32 = |value: i32| u32::try_from(value).unwrap_or(0);
    image::imageops::crop_imm(
        image,
        to_u32(crop.x),
        to_u32(crop.y),
        to_u32(crop.w),
        to_u32(crop.h),
    )
    .to_image()
}

/// The changed area padded and grown to at least `MIN_CROP_SHORT_SIDE`.
fn diff_crop(bbox: Rect, width: u32, height: u32) -> Rect {
    let grow = |start: i32, size: i32, limit: i32| -> (i32, i32) {
        let min = i32::try_from(MIN_CROP_SHORT_SIDE)
            .unwrap_or(i32::MAX)
            .min(limit);
        let mut start = start - CROP_PADDING;
        let mut size = size + 2 * CROP_PADDING;
        if size < min {
            start -= (min - size) / 2;
            size = min;
        }
        let size = size.min(limit);
        (start.clamp(0, limit - size), size)
    };
    let (width, height) = (
        i32::try_from(width).unwrap_or(i32::MAX),
        i32::try_from(height).unwrap_or(i32::MAX),
    );
    let (x, w) = grow(bbox.x, bbox.w, width);
    let (y, h) = grow(bbox.y, bbox.h, height);
    Rect { x, y, w, h }
}

/// Draws a 2 px red outline of `region` (clip pixels) on a crop at `crop`.
fn outline(image: &mut image::RgbaImage, region: Rect, crop: Rect) {
    let red = image::Rgba([230, 30, 30, 255]);
    let x0 = i64::from(region.x - crop.x);
    let y0 = i64::from(region.y - crop.y);
    let x1 = x0 + i64::from(region.w) - 1;
    let y1 = y0 + i64::from(region.h) - 1;
    let (width, height) = (i64::from(image.width()), i64::from(image.height()));
    let mut put = |x: i64, y: i64| {
        if (0..width).contains(&x) && (0..height).contains(&y) {
            image.put_pixel(x as u32, y as u32, red);
        }
    };
    for t in 0..2 {
        for x in x0..=x1 {
            put(x, y0 + t);
            put(x, y1 - t);
        }
        for y in y0..=y1 {
            put(x0 + t, y);
            put(x1 - t, y);
        }
    }
}

/// `left` and `right` next to each other with an 8 px grey divider.
fn side_by_side(left: &image::RgbaImage, right: &image::RgbaImage) -> image::RgbaImage {
    const GAP: u32 = 8;
    let width = left.width() + GAP + right.width();
    let height = left.height().max(right.height());
    let mut out = image::RgbaImage::from_pixel(width, height, image::Rgba([128, 128, 128, 255]));
    image::imageops::replace(&mut out, left, 0, 0);
    image::imageops::replace(&mut out, right, i64::from(left.width() + GAP), 0);
    out
}

/// 3x5 bitmap glyphs (one row per byte, bit 2 = left column).
fn glyph(c: char) -> [u8; 5] {
    match c {
        '0' => [7, 5, 5, 5, 7],
        '1' => [2, 6, 2, 2, 7],
        '2' => [7, 1, 7, 4, 7],
        '3' => [7, 1, 7, 1, 7],
        '4' => [5, 5, 7, 1, 1],
        '5' => [7, 4, 7, 1, 7],
        '6' => [7, 4, 7, 5, 7],
        '7' => [7, 1, 1, 1, 1],
        '8' => [7, 5, 7, 5, 7],
        '9' => [7, 5, 7, 1, 7],
        ':' => [0, 2, 0, 2, 0],
        '.' => [0, 0, 0, 0, 2],
        '#' => [5, 7, 5, 7, 5],
        'F' => [7, 4, 6, 4, 4],
        _ => [0; 5],
    }
}

/// Draws `text` in white on a dark box at the top-left corner.
fn draw_label(image: &mut image::RgbaImage, text: &str) {
    let unit: u32 = if image.width().min(image.height()) >= 600 {
        3
    } else {
        2
    };
    let pad = unit * 2;
    let chars = u32::try_from(text.chars().count()).unwrap_or(0);
    let box_w = (pad * 2 + chars * unit * 4).min(image.width());
    let box_h = (pad * 2 + unit * 5).min(image.height());
    for y in 0..box_h {
        for x in 0..box_w {
            image.put_pixel(x, y, image::Rgba([20, 20, 20, 255]));
        }
    }
    for (position, c) in (0_u32..).zip(text.chars()) {
        let left = pad + position * unit * 4;
        for (row, bits) in (0_u32..).zip(glyph(c)) {
            for col in 0..3_u32 {
                if bits & (4 >> col) == 0 {
                    continue;
                }
                for dy in 0..unit {
                    for dx in 0..unit {
                        let (x, y) = (left + col * unit + dx, pad + row * unit + dy);
                        if x < box_w && y < box_h {
                            image.put_pixel(x, y, image::Rgba([255, 255, 255, 255]));
                        }
                    }
                }
            }
        }
    }
}

fn fmt_time(ms: u64) -> String {
    format!(
        "{:02}:{:02}.{:03}",
        ms / 60_000,
        (ms / 1_000) % 60,
        ms % 1_000
    )
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

fn source_of_truth(folder: &str) -> String {
    format!(
        "clip folder {folder} ({VIDEO_FILE}, {FRAMES_FILE}, {CLIP_JSON_FILE}); exported images under {VIEWS_DIR}\\"
    )
}

fn params_invalid(message: String) -> ErrorData {
    mcp_error(error_codes::TOOL_PARAMS_INVALID, message)
}

fn internal(message: String) -> ErrorData {
    mcp_error(error_codes::TOOL_INTERNAL_ERROR, message)
}

fn decode_error(message: String) -> ErrorData {
    internal(format!("{VIDEO_FILE} decode failed: {message}"))
}

/// Adds tool, operation, source of truth and a remediation to an error.
fn with_view_context(
    mut error: ErrorData,
    operation: ScreenshotOperation,
    folder: &str,
) -> ErrorData {
    let mut data = match error.data.take() {
        Some(serde_json::Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    };
    let code = data
        .get("code")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(error_codes::TOOL_INTERNAL_ERROR)
        .to_owned();
    let remediation = if code == error_codes::TOOL_PARAMS_INVALID {
        "pass a clip folder written by screenshot operation=record and times inside the clip (clip.json health.duration_ms)"
    } else {
        "check that clip.mp4 is a complete recording (Media Foundation H.264 decoder); record the clip again if it is damaged"
    };
    let operation = super::m1_tools::screenshot_operation_name(operation);
    error.message = format!("screenshot operation={operation}: {}", error.message).into();
    let defaults = [
        ("code", json!(code)),
        ("tool", json!("screenshot")),
        ("operation", json!(operation)),
        ("source_of_truth", json!(source_of_truth(folder))),
        ("remediation", json!(remediation)),
    ];
    for (key, value) in defaults {
        data.entry(key).or_insert(value);
    }
    error.data = Some(serde_json::Value::Object(data));
    error
}
