//! Clip viewer: `screenshot operation=clip_overview | clip_frames | clip_frame
//! | clip_diff` (docs/clip-recorder-plan.md, version 4.1, section D).
//!
//! Agents cannot watch video, so a clip folder is read back as a text timeline
//! plus a few labelled PNG frames:
//!
//! - `clip_overview`: the change timeline (page 1 only) and key frames, as many
//!   per page as the token budget allows (D6).
//! - `clip_frames`: frames in a time range, as many as the call budget allows.
//! - `clip_frame`: one frame at recorded pixels, optional crop and ×2 close-up.
//! - `clip_diff`: changed regions between two times plus one before/after crop.
//!
//! D1: a clip is a list of committed segments (`segment-NNNN.mp4`) with
//! frames at uneven times; `frames.jsonl` has one line per instant k, and
//! instant k of a segment plays at k / f minus the segment's start. D4: only
//! committed segments are shown; a part still waiting is refused with the
//! count of frames waiting.
//!
//! Frame choice: `frames.jsonl` `new` flags skip repeated frames without
//! comparing them; new frames are compared with the picture on screen by an
//! 8 × 8 block test at recorded resolution. D5: the logged cursor is masked
//! when Windows drew it into the picture; a thin caret that toggles is
//! ignored; a block that returns to its previous picture within
//! `SHORT_LIVED_MS` is a short-lived change of its region, also while other
//! regions animate; a region that keeps changing is one animating interval
//! with one frame.
//!
//! Every image is a labelled PNG copy in the clip's `views` folder (the clip
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
use super::clip_decoder::{ClipDecoder, DecodedFrame};
use super::clip_folder::{
    CLIP_JSON_FILE, ClipJson, ClipState, FRAMES_FILE, FrameLine, SegmentCheckpoint, VIEWS_DIR,
    WAITING_DIR, is_waiting_frame, read_frame_lines,
};
use crate::m1::clip_view_limits::{MAX_FRAMES_FPS, MIN_CROP_SHORT_SIDE};
use crate::m1::{
    ClipChangedRegion, ClipTimelineEntry, ClipViewImage, ClipViewParams, ClipViewResponse,
    ScreenshotOperation, ScreenshotResponse, mcp_error,
};

/// N: upper bound on images per call; the token budget may allow fewer (D6).
const MAX_IMAGES_PER_CALL: usize = 8;
/// Image ceiling: standard-tier Claude models downsize anything larger (S4).
const VIEW_MAX_LONG_EDGE: u32 = 1_568;
/// σ_v lower bound: overview images keep at least this share of the window
/// size (h_leg / h_text = 9 / 12).
const OVERVIEW_MIN_SCALE: f64 = 0.75;
/// Token budget of the overview's images and of its text (S4).
const OVERVIEW_IMAGE_TOKEN_BUDGET: u64 = 20_000;
const OVERVIEW_TEXT_TOKEN_BUDGET: u64 = 4_000;
/// Token budget of every other viewer call (S4).
const CALL_TOKEN_BUDGET: u64 = 10_000;
/// The equation's token formula: tokens = N · w_v · h_v / 770 + 1500.
const PIXELS_PER_TOKEN: u64 = 770;
const RESPONSE_TEXT_TOKENS: u64 = 1_500;
/// Characters per token when the timeline text is held to its budget.
const TEXT_CHARS_PER_TOKEN: usize = 4;
/// Close-ups enlarge by whole-number duplication (D3).
const CLOSEUP_SCALE: u32 = 2;

/// Side of the change-test blocks, in recorded clip pixels.
const BLOCK: usize = 8;
/// Largest R, G or B difference that counts a pixel as changed (above
/// encoder noise).
const PIXEL_DELTA: u8 = 32;
/// Changed pixels that mark an 8x8 block as changed. A one-line text edit
/// changes whole glyph strokes (dozens of high-contrast pixels per block).
const MIN_CHANGED_PIXELS: u32 = 3;
/// A change is settled after this long without further change.
const SETTLE_MS: u64 = 200;
/// A change that returns to the previous picture within this is short-lived.
const SHORT_LIVED_MS: u64 = 500;
/// The box masked around the logged cursor, in window pixels (D5).
const CURSOR_MASK_PX: u32 = 48;
/// A caret is a change at most this wide (window pixels) that toggles about
/// every `CARET_PERIOD_MS` (D5).
const CARET_MAX_WIDTH_PX: u32 = 3;
const CARET_PERIOD_MS: u64 = 500;
/// At least one key frame per this interval.
const FLOOR_INTERVAL_MS: u64 = 10_000;
/// Unchanged stretches at least this long are listed in the timeline.
const STATIC_REPORT_MS: u64 = 1_000;
/// Padding around the changed area in the `clip_diff` crop.
const CROP_PADDING: i32 = 16;
const MAX_DIFF_REGIONS: usize = 20;

/// Cached overview analysis, so later pages skip the full decode.
const ANALYSIS_FILE: &str = "overview_analysis.json";
const ANALYSIS_VERSION: u32 = 6;

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
        record_status: None,
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
    let clip = ViewedClip::open(Path::new(&params.folder))?;
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

/// A clip folder as the viewer reads it: `clip.json`, one line per instant,
/// and the committed segments (D1).
struct ViewedClip {
    folder: PathBuf,
    clip: ClipJson,
    lines: Vec<FrameLine>,
}

impl ViewedClip {
    fn open(folder: &Path) -> Result<Self, ErrorData> {
        let clip = ClipJson::read(folder).map_err(|error| {
            params_invalid(format!(
                "{} is not a readable clip folder: {error}",
                folder.display()
            ))
        })?;
        if clip.fps == 0 {
            return Err(params_invalid(format!("{CLIP_JSON_FILE} has fps 0")));
        }
        let lines = read_frame_lines(folder)
            .map_err(|error| params_invalid(format!("{FRAMES_FILE} is not readable: {error}")))?;
        if lines.is_empty() {
            return Err(params_invalid(format!("{FRAMES_FILE} lists no instants")));
        }
        let viewed = Self {
            folder: folder.to_path_buf(),
            clip,
            lines,
        };
        if viewed.clip.segments.is_empty() {
            return Err(viewed.waiting_error(0));
        }
        Ok(viewed)
    }

    fn fps(&self) -> u64 {
        u64::from(self.clip.fps)
    }

    fn frame_ms(&self) -> u64 {
        (1_000 / self.fps()).max(1)
    }

    /// The last instant that is in a committed segment.
    fn last_frame(&self) -> u64 {
        self.clip
            .segments
            .last()
            .map_or(0, |segment| segment.last_instant)
            .min(self.lines.len() as u64 - 1)
    }

    /// Time of instant k: k / f.
    fn t_of(&self, frame: u64) -> u64 {
        frame * 1_000 / self.fps()
    }

    /// The instant on screen at `t_ms`.
    fn frame_at_ms(&self, t_ms: u64) -> u64 {
        (t_ms * self.fps() / 1_000).min(self.last_frame())
    }

    fn line(&self, frame: u64) -> Option<&FrameLine> {
        usize::try_from(frame)
            .ok()
            .and_then(|index| self.lines.get(index))
    }

    fn is_new(&self, frame: u64) -> bool {
        self.line(frame).is_none_or(|line| line.new)
    }

    /// `true` when no new frame arrived in `(from, to]`: same picture.
    fn same_picture(&self, from: u64, to: u64) -> bool {
        (from + 1..=to).all(|frame| !self.is_new(frame))
    }

    /// The viewable length: up to the end of the last committed segment.
    fn duration_ms(&self) -> u64 {
        self.t_of(self.last_frame() + 1)
    }

    /// The whole recorded length, committed or not.
    fn recorded_ms(&self) -> u64 {
        self.t_of(self.lines.len() as u64)
    }

    fn segment_of(&self, frame: u64) -> Option<&SegmentCheckpoint> {
        self.clip
            .segments
            .iter()
            .find(|segment| (segment.first_instant..=segment.last_instant).contains(&frame))
    }

    /// σ of the clip: recorded pixels per window pixel.
    fn scale(&self) -> u32 {
        self.clip.geometry.scale.max(1)
    }

    /// D4: the part asked for is not committed yet.
    fn waiting_error(&self, t_ms: u64) -> ErrorData {
        let waiting = std::fs::read_dir(self.folder.join(WAITING_DIR))
            .map(|read| {
                read.filter_map(Result::ok)
                    .filter(|entry| is_waiting_frame(&entry.path()))
                    .count()
            })
            .unwrap_or(0);
        params_invalid(format!(
            "{} ms of this clip is not committed yet (state {}, {} committed segments up to {} ms, {waiting} frames waiting); call screenshot operation=record_status and retry when the segment is committed",
            t_ms,
            self.clip.state.as_str(),
            self.clip.segments.len(),
            self.clip
                .segments
                .last()
                .map_or(0, |segment| segment.end_ms),
        ))
    }

    /// Size and modified time of the files the overview is computed from; a
    /// cached overview is reused only while this is unchanged (also when a new
    /// segment is committed).
    fn fingerprint(&self) -> String {
        let mut names = vec![FRAMES_FILE.to_owned(), CLIP_JSON_FILE.to_owned()];
        names.extend(
            self.clip
                .segments
                .iter()
                .map(|segment| segment.file.clone()),
        );
        names
            .iter()
            .map(|name| {
                let metadata = std::fs::metadata(self.folder.join(name)).ok();
                let modified = metadata
                    .as_ref()
                    .and_then(|metadata| metadata.modified().ok())
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |since| since.as_nanos());
                let size = metadata.map_or(0, |metadata| metadata.len());
                format!("{name}:{size}:{modified}")
            })
            .collect::<Vec<_>>()
            .join(";")
    }

    fn open_decoder(&self, segment: &SegmentCheckpoint) -> Result<ClipDecoder, ErrorData> {
        ClipDecoder::open(&self.folder.join(&segment.file))
            .map_err(|error| internal(format!("could not open {}: {error}", segment.file)))
    }

    fn views_dir(&self) -> Result<PathBuf, ErrorData> {
        let views = self.folder.join(VIEWS_DIR);
        std::fs::create_dir_all(&views)
            .map_err(|error| internal(format!("could not create {views:?}: {error}")))?;
        Ok(views)
    }

    /// The largest recorded size among the committed segments.
    fn largest_recorded(&self) -> (u32, u32) {
        self.clip.segments.iter().fold((0, 0), |(w, h), segment| {
            (w.max(segment.width), h.max(segment.height))
        })
    }

    /// D2: the long edge of overview and range images: at least
    /// `OVERVIEW_MIN_SCALE` of the window size, at most `VIEW_MAX_LONG_EDGE`.
    /// The second value is `true` when the ceiling cuts below the floor.
    fn view_long_edge(&self, requested: Option<u32>) -> (u32, bool) {
        let (width, height) = self.largest_recorded();
        let window_long = width.max(height) / self.scale();
        let floor = (f64::from(window_long) * OVERVIEW_MIN_SCALE).ceil() as u32;
        let wanted = requested.unwrap_or(VIEW_MAX_LONG_EDGE).max(floor);
        (wanted.min(VIEW_MAX_LONG_EDGE), floor > VIEW_MAX_LONG_EDGE)
    }

    /// D6: images per call so that N · w_v · h_v / 770 stays inside `budget`.
    fn images_per_call(&self, long_edge: u32, budget: u64) -> usize {
        let (width, height) = self.largest_recorded();
        let long = width.max(height).max(1);
        let factor = f64::from(long_edge.min(long)) / f64::from(long);
        let pixels = (f64::from(width) * factor) * (f64::from(height) * factor);
        let per_image = (pixels / PIXELS_PER_TOKEN as f64).max(1.0);
        ((budget as f64 / per_image) as usize).clamp(1, MAX_IMAGES_PER_CALL)
    }

    /// The cursor of instant k in recorded pixels, when Windows drew it into
    /// the picture (E9: otherwise it is not in the frames).
    fn cursor_at(&self, frame: u64) -> Option<(i64, i64)> {
        if !self.clip.observer.cursor_in_picture {
            return None;
        }
        let [x, y] = self.line(frame)?.cursor?;
        let scale = i64::from(self.scale());
        Some((i64::from(x) * scale, i64::from(y) * scale))
    }
}

// ---------------------------------------------------------------------------
// Change test
// ---------------------------------------------------------------------------

/// R, G, B of a BGRA frame, three bytes per pixel: the change test compares
/// colour, so a hue change with equal luma still counts.
fn rgb(bgra: &[u8]) -> Vec<u8> {
    bgra.chunks_exact(4)
        .flat_map(|px| [px[2], px[1], px[0]])
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
        let row_bytes = width * 3;
        for y in 0..height {
            let row = y * row_bytes;
            let block_row = (y / BLOCK) * map.cols;
            let (Some(a), Some(b)) = (
                before.get(row..row + row_bytes),
                after.get(row..row + row_bytes),
            ) else {
                break;
            };
            for (x, (pa, pb)) in a.chunks_exact(3).zip(b.chunks_exact(3)).enumerate() {
                let delta = pa
                    .iter()
                    .zip(pb)
                    .map(|(ca, cb)| ca.abs_diff(*cb))
                    .max()
                    .unwrap_or(0);
                if delta > PIXEL_DELTA {
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

    fn unset(&mut self, index: usize) {
        if let Some(flag) = self.changed.get_mut(index)
            && *flag
        {
            *flag = false;
            self.count -= 1;
        }
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

    /// Connected groups of changed blocks (8-neighbour), as block indices.
    fn block_groups(&self) -> Vec<Vec<usize>> {
        let mut seen = vec![false; self.changed.len()];
        let mut groups = Vec::new();
        for start in 0..self.changed.len() {
            if !self.changed[start] || seen[start] {
                continue;
            }
            seen[start] = true;
            let mut stack = vec![start];
            let mut group = Vec::new();
            while let Some(index) = stack.pop() {
                group.push(index);
                let (col, row) = (index % self.cols, index / self.cols);
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
            groups.push(group);
        }
        groups
    }

    /// Connected groups of changed blocks, largest first, as rectangles.
    fn regions(&self) -> Vec<ClipChangedRegion> {
        let mut regions: Vec<ClipChangedRegion> = self
            .block_groups()
            .iter()
            .filter_map(|group| {
                blocks_bbox(group, self.cols, self.width, self.height).map(|rect| {
                    ClipChangedRegion {
                        rect,
                        changed_blocks: group.len() as u64,
                    }
                })
            })
            .collect();
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
    /// RGB before the change (kept only while a revert can still follow).
    before: Vec<u8>,
    short_lived: bool,
    reverted: bool,
    settled: bool,
    /// The resize generation the change happened in (see `Analyzer`).
    generation: u64,
}

/// What one block went through (D5): its picture before a change still open,
/// its current run of changes, and when it last looked like a caret.
#[derive(Clone, Default)]
struct BlockState {
    baseline: Option<(Vec<u8>, u64)>,
    run_start: u64,
    last_change: Option<u64>,
    animating: bool,
    last_caret: Option<u64>,
}

/// A region that returned to its previous picture within `SHORT_LIVED_MS`.
struct Transient {
    start: u64,
    end: u64,
    blocks: Vec<usize>,
    /// The picture size the block indices refer to (a resize changes it).
    size: (usize, usize),
}

/// A region that kept changing for longer than `SHORT_LIVED_MS`.
struct Animation {
    start: u64,
    end: u64,
    blocks: Vec<usize>,
    /// The picture size the block indices refer to (a resize changes it).
    size: (usize, usize),
    /// The resize generation of the run (see `Analyzer`).
    generation: u64,
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
    /// `ViewedClip::fingerprint` of the files this was computed from.
    #[serde(default)]
    inputs: String,
    timeline: Vec<ClipTimelineEntry>,
    picks: Vec<Pick>,
    notes: Vec<String>,
}

/// One decode pass over the committed segments that finds the changes.
struct Analyzer<'a> {
    clip: &'a ViewedClip,
    width: usize,
    height: usize,
    shown: Vec<u8>,
    states: Vec<BlockState>,
    open: Option<Episode>,
    done: Vec<Episode>,
    transients: Vec<Transient>,
    animations: Vec<Animation>,
    resizes: Vec<(u64, usize, usize)>,
    /// Goes up on every size change; changes are compared and merged only
    /// within one generation, also when a later resize restores the size.
    generation: u64,
    /// Captured size (frames.jsonl) of the picture last decoded.
    captured: (u32, u32),
    previous: u64,
    decoded: u64,
    new_without_change: u64,
    masked_carets: u64,
}

impl<'a> Analyzer<'a> {
    fn run(clip: &'a ViewedClip) -> Result<Analysis, ErrorData> {
        let mut analyzer = Self {
            clip,
            width: 0,
            height: 0,
            shown: Vec::new(),
            states: Vec::new(),
            open: None,
            done: Vec::new(),
            transients: Vec::new(),
            animations: Vec::new(),
            resizes: Vec::new(),
            generation: 0,
            captured: (0, 0),
            previous: 0,
            decoded: 0,
            new_without_change: 0,
            masked_carets: 0,
        };
        for segment in &clip.clip.segments {
            let mut decoder = clip.open_decoder(segment)?;
            while let Some(frame) = decoder.next_frame().map_err(decode_error)? {
                analyzer.decoded += 1;
                let local = frame.t_ms() * clip.fps() / 1_000;
                let instant = (segment.first_instant + local).min(segment.last_instant);
                analyzer.step(instant, &frame);
            }
        }
        if let Some(episode) = analyzer.open.take() {
            analyzer.close(episode, false);
        }
        let end = clip.last_frame();
        analyzer.end_animations(end, true);
        Ok(analyzer.finish())
    }

    fn step(&mut self, instant: u64, frame: &DecodedFrame) {
        let (width, height) = (frame.width as usize, frame.height as usize);
        let current = rgb(&frame.bgra);
        // A resize is a change of the captured size in frames.jsonl: an odd
        // size padded up to even can decode at the same size as before.
        let captured = self
            .clip
            .line(instant)
            .filter(|line| line.width != 0 && line.height != 0)
            .map_or(self.captured, |line| (line.width, line.height));
        if (width, height) != (self.width, self.height) || captured != self.captured {
            self.captured = captured;
            if self.width != 0 {
                if let Some(episode) = self.open.take() {
                    self.close(episode, true);
                }
                self.end_animations(instant, true);
                self.resizes.push((instant, width, height));
            }
            self.width = width;
            self.height = height;
            self.generation += 1;
            self.states =
                vec![BlockState::default(); width.div_ceil(BLOCK) * height.div_ceil(BLOCK)];
            self.shown = current;
            self.previous = instant;
            return;
        }
        let t_ms = self.clip.t_of(instant);
        let settled = self.open.as_ref().is_some_and(|episode| {
            t_ms.saturating_sub(self.clip.t_of(episode.last_change_frame)) >= SETTLE_MS
        });
        if settled && let Some(episode) = self.open.take() {
            self.close(episode, true);
        }
        self.end_animations(instant, false);
        if !self.clip.is_new(instant) {
            return;
        }
        let mut blocks = BlockMap::between(&self.shown, &current, width, height);
        self.mask_cursor(&mut blocks, instant);
        self.mask_caret(&mut blocks, &current, instant);
        self.track_blocks(&blocks, &current, instant);
        let mut episode_blocks = blocks.clone();
        for (index, state) in self.states.iter().enumerate() {
            if state.animating {
                episode_blocks.unset(index);
            }
        }
        self.previous = instant;
        if episode_blocks.count == 0 {
            if blocks.count == 0 {
                self.new_without_change += 1;
            }
            self.shown = current;
            return;
        }
        let before = std::mem::replace(&mut self.shown, current);
        let episode = self.open.get_or_insert_with(|| Episode {
            start_frame: instant,
            last_change_frame: instant,
            blocks: BlockMap::empty(width, height),
            before,
            short_lived: false,
            reverted: false,
            settled: false,
            generation: self.generation,
        });
        episode.last_change_frame = instant;
        episode.blocks.merge(&episode_blocks);
    }

    /// D5: the blocks under the logged cursor (now and at the last instant)
    /// do not count as changed when the cursor is in the picture.
    fn mask_cursor(&self, blocks: &mut BlockMap, instant: u64) {
        let box_px = i64::from(CURSOR_MASK_PX * self.clip.scale());
        for at in [self.previous, instant] {
            let Some((x, y)) = self.clip.cursor_at(at) else {
                continue;
            };
            let (x0, y0) = ((x - box_px / 2).max(0), (y - box_px / 2).max(0));
            let to_block = |value: i64| usize::try_from(value).unwrap_or(0) / BLOCK;
            for row in to_block(y0)..=to_block(y0 + box_px).min(blocks.rows.saturating_sub(1)) {
                for col in to_block(x0)..=to_block(x0 + box_px).min(blocks.cols.saturating_sub(1)) {
                    blocks.unset(row * blocks.cols + col);
                }
            }
        }
    }

    /// D5: a changed region at most `CARET_MAX_WIDTH_PX` wide that changed
    /// in the same blocks within two caret periods is a blinking caret.
    fn mask_caret(&mut self, blocks: &mut BlockMap, current: &[u8], instant: u64) {
        let caret_width =
            usize::try_from(CARET_MAX_WIDTH_PX * self.clip.scale()).unwrap_or(usize::MAX);
        let period = 2 * CARET_PERIOD_MS;
        for region in blocks.block_groups() {
            let width = pixel_width(
                &self.shown,
                current,
                self.width,
                self.height,
                blocks.cols,
                &region,
            );
            if width == 0 || width > caret_width {
                continue;
            }
            let toggled = region.iter().any(|&index| {
                self.states[index].last_caret.is_some_and(|at| {
                    self.clip.t_of(instant).saturating_sub(self.clip.t_of(at)) <= period
                })
            });
            for &index in &region {
                self.states[index].last_caret = Some(instant);
                if toggled {
                    blocks.unset(index);
                }
            }
            if toggled {
                self.masked_carets += 1;
            }
        }
    }

    /// D5: per block, a return to the picture before its change within
    /// `SHORT_LIVED_MS` is a short-lived change of that block; changes that
    /// go on for longer than `SHORT_LIVED_MS` without a `SETTLE_MS` pause
    /// make the block animating.
    fn track_blocks(&mut self, blocks: &BlockMap, current: &[u8], instant: u64) {
        let t_ms = self.clip.t_of(instant);
        let (width, height, cols) = (self.width, self.height, blocks.cols);
        let mut returned: Vec<(u64, usize)> = Vec::new();
        for index in 0..self.states.len() {
            let clip = self.clip;
            let state = &mut self.states[index];
            if state
                .baseline
                .as_ref()
                .is_some_and(|(_, at)| t_ms.saturating_sub(clip.t_of(*at)) > SHORT_LIVED_MS)
            {
                state.baseline = None;
            }
            if !blocks.changed[index] {
                continue;
            }
            let quiet = state
                .last_change
                .is_none_or(|last| t_ms.saturating_sub(clip.t_of(last)) > SETTLE_MS);
            if quiet {
                state.run_start = instant;
            }
            state.last_change = Some(instant);
            if t_ms.saturating_sub(clip.t_of(state.run_start)) > SHORT_LIVED_MS {
                state.animating = true;
            }
            let now = block_pixels(current, width, height, cols, index);
            match state.baseline.take() {
                Some((before, at)) if !state.animating && block_equal(&before, &now) => {
                    returned.push((at, index));
                }
                Some(baseline) => state.baseline = Some(baseline),
                None => {
                    state.baseline = Some((
                        block_pixels(&self.shown, width, height, cols, index),
                        instant,
                    ));
                }
            }
        }
        returned.sort_unstable();
        for (start, index) in returned {
            match self.transients.last_mut() {
                Some(last) if last.start == start && last.end == instant => last.blocks.push(index),
                _ => self.transients.push(Transient {
                    start,
                    end: instant,
                    blocks: vec![index],
                    size: (width, height),
                }),
            }
        }
    }

    /// Ends the animating runs that paused for `SETTLE_MS` (or all at the end)
    /// and keeps each as one interval.
    fn end_animations(&mut self, instant: u64, all: bool) {
        let t_ms = self.clip.t_of(instant);
        let mut ended: Vec<(u64, u64, usize)> = Vec::new();
        for (index, state) in self.states.iter_mut().enumerate() {
            let Some(last) = state.last_change else {
                continue;
            };
            if state.animating && (all || t_ms.saturating_sub(self.clip.t_of(last)) > SETTLE_MS) {
                ended.push((state.run_start, last, index));
                state.animating = false;
            }
        }
        // The block indices refer to the current picture size; animations of
        // an earlier resize generation are never merged with them.
        let (size, generation) = ((self.width, self.height), self.generation);
        for (start, end, index) in ended {
            match self.animations.iter_mut().find(|animation| {
                animation.generation == generation
                    && animation.start <= end
                    && start <= animation.end
            }) {
                Some(animation) => {
                    animation.start = animation.start.min(start);
                    animation.end = animation.end.max(end);
                    animation.blocks.push(index);
                }
                None => self.animations.push(Animation {
                    start,
                    end,
                    blocks: vec![index],
                    size,
                    generation,
                }),
            }
        }
    }

    /// Ends a change; marks it short-lived when the picture is back to the
    /// one before it within `SHORT_LIVED_MS`, also when the return came as a
    /// separate change right after.
    fn close(&mut self, mut episode: Episode, settled: bool) {
        let (width, height) = (self.width, self.height);
        let span = |clip: &ViewedClip, start: u64, end: u64| clip.t_of(end) - clip.t_of(start);
        episode.settled = settled;
        if BlockMap::between(&episode.before, &self.shown, width, height).count == 0 {
            episode.reverted = true;
            episode.short_lived =
                span(self.clip, episode.start_frame, episode.last_change_frame) <= SHORT_LIVED_MS;
        }
        // A resize ends comparison continuity: a change before it is never
        // compared with, or merged into, a picture of another size.
        if !episode.reverted
            && let Some(previous) = self.done.last_mut()
            && !previous.reverted
            && previous.generation == episode.generation
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
        let first = clip
            .clip
            .segments
            .first()
            .map_or(0, |segment| segment.first_instant);
        let mut picks = vec![pick(clip, first, "first frame")];
        let mut timeline = Vec::new();
        let mut quiet_from = first;
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
        let sized_bbox = |blocks: &[usize], (width, height): (usize, usize)| {
            blocks_bbox(blocks, width.div_ceil(BLOCK), width, height)
        };
        let covered = |start: u64| {
            self.done
                .iter()
                .any(|episode| episode.short_lived && episode.start_frame == start)
        };
        for transient in self
            .transients
            .iter()
            .filter(|transient| !covered(transient.start))
        {
            let bbox = sized_bbox(&transient.blocks, transient.size);
            picks.push(pick(clip, transient.start, "short-lived region change"));
            timeline.push(region_entry(
                clip,
                "short_lived",
                transient.start,
                transient.end,
                transient.blocks.len(),
                bbox,
                "short-lived region change, back to its previous picture",
            ));
        }
        for animation in &self.animations {
            let bbox = sized_bbox(&animation.blocks, animation.size);
            picks.push(pick(clip, animation.start, "animating region"));
            timeline.push(region_entry(
                clip,
                "animating",
                animation.start,
                animation.end,
                animation.blocks.len(),
                bbox,
                "region kept changing (animation); one frame shown",
            ));
        }
        for &(instant, width, height) in &self.resizes {
            picks.push(pick(clip, instant, "first frame after resize"));
            timeline.push(region_entry(
                clip,
                "change",
                instant,
                instant,
                0,
                None,
                &format!("window size changed; recorded size now {width}x{height}"),
            ));
        }
        timeline.extend(minimized_entries(clip));
        timeline.sort_by_key(|entry| (entry.start_ms, entry.end_ms));
        picks.push(pick(clip, last, "last frame"));
        let picks = with_floor(clip, merge_picks(picks));

        let repeated = clip.lines.iter().filter(|line| !line.new).count();
        let mut notes = vec![
            format!(
                "{} changes, {} short-lived region changes and {} animating regions found; {repeated} repeated frames skipped by frames.jsonl flags; {} new frames showed no visible change",
                self.done.len(),
                self.transients.len(),
                self.animations.len(),
                self.new_without_change
            ),
            "change test: 8x8 blocks at recorded resolution, largest R/G/B difference; settled after 200 ms, short-lived = back within 500 ms, per region".to_owned(),
        ];
        if clip.clip.observer.cursor_in_picture {
            notes.push("the cursor is in the picture on this Windows; a 48 px box at its logged position is masked".to_owned());
        }
        if self.masked_carets > 0 {
            notes.push(format!(
                "{} blinking caret toggles ignored",
                self.masked_carets
            ));
        }
        let lost = clip.clip.lost.len();
        if lost > 0 {
            notes.push(format!(
                "{lost} instants are lost (clip.json lost lists each with its cause)"
            ));
        }
        if clip.last_frame() + 1 < clip.lines.len() as u64 {
            notes.push(format!(
                "only the committed part is shown: {} of {} ms (state {})",
                clip.duration_ms(),
                clip.recorded_ms(),
                clip.clip.state.as_str()
            ));
        }
        if self.done.last().is_some_and(|episode| !episode.settled) {
            notes.push("the last change had not settled when the clip ended".to_owned());
        }
        if let Some(reason) = clip.clip.stop_reason
            && reason != super::clip_folder::StopReason::Completed
        {
            notes.push(format!("the recording stopped: {}", reason.as_str()));
        }
        Analysis {
            version: ANALYSIS_VERSION,
            inputs: String::new(),
            timeline,
            picks,
            notes,
        }
    }
}

/// The `minimized` intervals: consecutive instants marked minimized.
fn minimized_entries(clip: &ViewedClip) -> Vec<ClipTimelineEntry> {
    let mut entries = Vec::new();
    let mut start: Option<u64> = None;
    let last = clip.last_frame();
    for line in clip.lines.iter().take_while(|line| line.instant <= last) {
        match (line.minimized, start) {
            (true, None) => start = Some(line.instant),
            (false, Some(from)) => {
                entries.push(minimized_entry(clip, from, line.instant));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(from) = start {
        entries.push(minimized_entry(clip, from, last + 1));
    }
    entries
}

fn minimized_entry(clip: &ViewedClip, from: u64, to: u64) -> ClipTimelineEntry {
    let (start_ms, end_ms) = (clip.t_of(from), clip.t_of(to));
    ClipTimelineEntry {
        kind: "minimized".to_owned(),
        start_ms,
        end_ms,
        start_frame: from,
        end_frame: to.saturating_sub(1),
        changed_blocks: 0,
        bbox: None,
        text: format!(
            "{}-{} gap: window minimized, the last picture was repeated",
            fmt_time(start_ms),
            fmt_time(end_ms)
        ),
    }
}

fn region_entry(
    clip: &ViewedClip,
    kind: &str,
    start: u64,
    end: u64,
    blocks: usize,
    bbox: Option<Rect>,
    what: &str,
) -> ClipTimelineEntry {
    let (start_ms, end_ms) = (clip.t_of(start), clip.t_of(end));
    let area = bbox.map_or_else(String::new, |r| {
        format!(" in x={} y={} w={} h={}", r.x, r.y, r.w, r.h)
    });
    ClipTimelineEntry {
        kind: kind.to_owned(),
        start_ms,
        end_ms,
        start_frame: start,
        end_frame: end,
        changed_blocks: blocks as u64,
        bbox,
        text: format!(
            "{}-{} {what}{area} (frame {start})",
            fmt_time(start_ms),
            fmt_time(end_ms)
        ),
    }
}

/// The bounding box of a set of blocks, in recorded pixels.
fn blocks_bbox(blocks: &[usize], cols: usize, width: usize, height: usize) -> Option<Rect> {
    let cols = cols.max(1);
    let (mut c0, mut r0, mut c1, mut r1) = (usize::MAX, usize::MAX, 0, 0);
    for &index in blocks {
        let (col, row) = (index % cols, index / cols);
        (c0, r0, c1, r1) = (c0.min(col), r0.min(row), c1.max(col), r1.max(row));
    }
    (c0 != usize::MAX).then(|| {
        let x = c0 * BLOCK;
        let y = r0 * BLOCK;
        rect(
            x,
            y,
            ((c1 + 1) * BLOCK).min(width) - x,
            ((r1 + 1) * BLOCK).min(height) - y,
        )
    })
}

/// The RGB pixels of one block.
fn block_pixels(rgb: &[u8], width: usize, height: usize, cols: usize, index: usize) -> Vec<u8> {
    let (col, row) = (index % cols.max(1), index / cols.max(1));
    let mut out = Vec::with_capacity(BLOCK * BLOCK * 3);
    for y in row * BLOCK..((row + 1) * BLOCK).min(height) {
        let start = (y * width + col * BLOCK) * 3;
        let end = (y * width + ((col + 1) * BLOCK).min(width)) * 3;
        out.extend_from_slice(rgb.get(start..end).unwrap_or_default());
    }
    out
}

/// `true` when two blocks pass the change test as unchanged.
fn block_equal(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len()
        && a.chunks_exact(3)
            .zip(b.chunks_exact(3))
            .filter(|(pa, pb)| {
                pa.iter()
                    .zip(pb.iter())
                    .map(|(ca, cb)| ca.abs_diff(*cb))
                    .max()
                    .unwrap_or(0)
                    > PIXEL_DELTA
            })
            .count()
            < MIN_CHANGED_PIXELS as usize
}

/// The width in pixels of the changed pixels inside a group of blocks.
fn pixel_width(
    before: &[u8],
    after: &[u8],
    width: usize,
    height: usize,
    cols: usize,
    region: &[usize],
) -> usize {
    let (mut x0, mut x1) = (usize::MAX, 0);
    for &index in region {
        let (col, row) = (index % cols.max(1), index / cols.max(1));
        for y in row * BLOCK..((row + 1) * BLOCK).min(height) {
            for x in col * BLOCK..((col + 1) * BLOCK).min(width) {
                let at = (y * width + x) * 3;
                let (Some(a), Some(b)) = (before.get(at..at + 3), after.get(at..at + 3)) else {
                    continue;
                };
                let delta = a
                    .iter()
                    .zip(b)
                    .map(|(ca, cb)| ca.abs_diff(*cb))
                    .max()
                    .unwrap_or(0);
                if delta > PIXEL_DELTA {
                    x0 = x0.min(x);
                    x1 = x1.max(x);
                }
            }
        }
    }
    if x0 == usize::MAX { 0 } else { x1 - x0 + 1 }
}

fn pick(clip: &ViewedClip, frame: u64, reason: &str) -> Pick {
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
fn with_floor(clip: &ViewedClip, picks: Vec<Pick>) -> Vec<Pick> {
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

fn static_entry(clip: &ViewedClip, from: u64, to: u64) -> ClipTimelineEntry {
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

fn episode_entry(clip: &ViewedClip, episode: &Episode) -> ClipTimelineEntry {
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

/// One viewer call: the clip, the decoder of the segment in use and the images
/// made so far.
struct ClipView {
    clip: ViewedClip,
    decoder: Option<(u32, ClipDecoder)>,
    images: Vec<ClipViewImage>,
    pngs: Vec<Vec<u8>>,
    timeline: Vec<ClipTimelineEntry>,
    regions: Vec<ClipChangedRegion>,
    paging: Option<(u32, u32)>,
    notes: Vec<String>,
    text: String,
}

impl ClipView {
    fn new(clip: ViewedClip) -> Self {
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

    /// D2, D6: key frames shrunk by averaging, never below 75 % of the window
    /// size and never above the ceiling, as many per page as the image budget
    /// allows; the timeline on page 1 only, held to the text budget.
    fn overview(&mut self, params: &ClipViewParams) -> Result<(), ErrorData> {
        let analysis = self.analysis()?;
        let (long_edge, ceiling_below_floor) = self.clip.view_long_edge(params.max_long_edge);
        let per_page = self
            .clip
            .images_per_call(long_edge, OVERVIEW_IMAGE_TOKEN_BUDGET);
        let pages = u32::try_from(analysis.picks.len().div_ceil(per_page))
            .unwrap_or(u32::MAX)
            .max(1);
        let page = params.page.unwrap_or(1);
        if page == 0 || page > pages {
            return Err(params_invalid(format!(
                "page {page} is outside 1..={pages} for this clip"
            )));
        }
        let first = (page as usize - 1) * per_page;
        let picks: Vec<Pick> = analysis
            .picks
            .iter()
            .skip(first)
            .take(per_page)
            .cloned()
            .collect();
        let _ = writeln!(
            self.text,
            "Clip {} ({} key frames, page {page}/{pages}):",
            self.clip.clip.clip_id,
            analysis.picks.len()
        );
        if page == 1 {
            let budget_chars = usize::try_from(OVERVIEW_TEXT_TOKEN_BUDGET).unwrap_or(usize::MAX)
                * TEXT_CHARS_PER_TOKEN;
            let mut kept = Vec::new();
            for entry in analysis.timeline {
                if self.text.len() + entry.text.len() + 3 > budget_chars {
                    self.notes.push(format!(
                        "the timeline was cut at {} to stay inside its token budget; call clip_frames or clip_diff for later times",
                        fmt_time(entry.start_ms)
                    ));
                    break;
                }
                let _ = writeln!(self.text, "- {}", entry.text);
                kept.push(entry);
            }
            self.timeline = kept;
        }
        if ceiling_below_floor {
            self.notes.push(format!(
                "this window is too large for 75 % of its size inside the {VIEW_MAX_LONG_EDGE} px image ceiling; small text may be unreadable here, use clip_frame (recorded pixels, region crop, close-up) to read it"
            ));
        }
        for (offset, pick) in picks.iter().enumerate() {
            let index = u32::try_from(first + offset + 1).unwrap_or(u32::MAX);
            let frame = self.decode(pick.frame)?;
            let image = fit(to_rgba(&frame), long_edge);
            let stem = format!("overview_{index:02}_f{}", pick.frame);
            self.export(image, index, pick.frame, &pick.reason, &stem)?;
        }
        let mut notes = analysis.notes;
        notes.append(&mut self.notes);
        self.notes = notes;
        self.paging = Some((page, pages));
        Ok(())
    }

    fn frames(&mut self, params: &ClipViewParams) -> Result<(), ErrorData> {
        let (start, end) = self.range(params)?;
        let (long_edge, _) = self.clip.view_long_edge(params.max_long_edge);
        let budget = CALL_TOKEN_BUDGET.saturating_sub(RESPONSE_TEXT_TOKENS);
        let max_images = self.clip.images_per_call(long_edge, budget);
        let fps = params
            .fps
            .unwrap_or(MAX_FRAMES_FPS)
            .clamp(1, MAX_FRAMES_FPS);
        let min_step = 1_000_u64.div_ceil(u64::from(fps));
        let fit_step = (end - start).div_ceil(max_images.saturating_sub(1).max(1) as u64);
        let step = min_step.max(fit_step).max(1);
        if step > min_step {
            self.notes.push(format!(
                "sampled every {step} ms so the range fits {max_images} images; narrow the range for more detail"
            ));
        }
        let _ = writeln!(
            self.text,
            "Clip {} frames {}-{} every {step} ms:",
            self.clip.clip.clip_id,
            fmt_time(start),
            fmt_time(end)
        );
        let mut previous: Option<(u64, u32)> = None;
        let mut t_ms = start;
        while t_ms <= end && self.images.len() < max_images {
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

    /// One frame at recorded pixels; D3: a close-up duplicates every pixel
    /// `CLOSEUP_SCALE` times, never blurs.
    fn frame(&mut self, params: &ClipViewParams) -> Result<(), ErrorData> {
        let t_ms = params.t_ms.unwrap_or(0);
        self.check_committed(t_ms)?;
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
        let closeup = params.scale.is_some_and(|scale| scale > 1.0);
        let factor = if closeup { CLOSEUP_SCALE } else { 1 };
        let (width, height) = (image.width() * factor, image.height() * factor);
        // D6: the call stays inside its token budget (tokens = w · h / 770 + 1500).
        let tokens = u64::from(width) * u64::from(height) / PIXELS_PER_TOKEN + RESPONSE_TEXT_TOKENS;
        if tokens > CALL_TOKEN_BUDGET {
            let max_pixels =
                CALL_TOKEN_BUDGET.saturating_sub(RESPONSE_TEXT_TOKENS) * PIXELS_PER_TOKEN;
            return Err(params_invalid(format!(
                "a {width}x{height} image costs about {tokens} tokens, over the {CALL_TOKEN_BUDGET}-token call budget (at most {max_pixels} pixels); pass a smaller region (x, y, w, h in recorded pixels) or no close-up"
            )));
        }
        if closeup {
            image = image::imageops::resize(
                &image,
                width,
                height,
                image::imageops::FilterType::Nearest,
            );
            let _ = write!(stem, "_x{CLOSEUP_SCALE}");
            if params
                .scale
                .is_some_and(|scale| (scale - f64::from(CLOSEUP_SCALE)).abs() > f64::EPSILON)
            {
                self.notes.push(format!(
                    "close-ups duplicate pixels by a whole number: scale {CLOSEUP_SCALE} was used"
                ));
            }
        }
        let _ = writeln!(
            self.text,
            "Clip {} frame {number} at {} ({}x{} px, close-up x{factor}):",
            self.clip.clip.clip_id,
            fmt_time(self.clip.t_of(number)),
            width,
            height
        );
        let reason = if params.region.is_some() {
            "crop"
        } else {
            "recorded pixels"
        };
        self.export(image, 1, number, reason, &stem)
    }

    fn diff(&mut self, params: &ClipViewParams) -> Result<(), ErrorData> {
        let (start, end) = self.range(params)?;
        let (from, to) = (self.clip.frame_at_ms(start), self.clip.frame_at_ms(end));
        let _ = writeln!(
            self.text,
            "Clip {} diff {} (frame {from}) -> {} (frame {to}):",
            self.clip.clip.clip_id,
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
            let _ = writeln!(
                self.text,
                "- the window size changed between the two times ({}x{} -> {}x{}); compare them with clip_frame",
                before.width, before.height, after.width, after.height
            );
            return Ok(());
        }
        let (width, height) = (before.width as usize, before.height as usize);
        let blocks = BlockMap::between(&rgb(&before.bgra), &rgb(&after.bgra), width, height);
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
        let image = fit(side_by_side(&left, &right), VIEW_MAX_LONG_EDGE);
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

    /// D4: a time past the committed segments is refused, saying how many
    /// frames wait.
    fn check_committed(&self, t_ms: u64) -> Result<(), ErrorData> {
        if t_ms < self.clip.duration_ms() {
            return Ok(());
        }
        if t_ms < self.clip.recorded_ms() || self.clip.clip.state != ClipState::Ready {
            return Err(self.clip.waiting_error(t_ms));
        }
        Err(params_invalid(format!(
            "t_ms {t_ms} is at or after the clip end ({} ms)",
            self.clip.duration_ms()
        )))
    }

    /// `start_ms`..=`end_ms`, checked against the committed part of the clip.
    fn range(&self, params: &ClipViewParams) -> Result<(u64, u64), ErrorData> {
        let (start, end) = (params.start_ms.unwrap_or(0), params.end_ms.unwrap_or(0));
        if start >= end {
            return Err(params_invalid(format!(
                "start_ms {start} must be before end_ms {end}"
            )));
        }
        self.check_committed(start)?;
        let duration = self.clip.duration_ms();
        // D4: a range that reaches uncommitted time (its end included) is
        // refused; only a finished clip clamps the end to its last frame.
        let uncommitted =
            self.clip.recorded_ms() > duration || self.clip.clip.state != ClipState::Ready;
        if end >= duration && uncommitted {
            return Err(self.clip.waiting_error(end));
        }
        Ok((start, end.min(duration - 1)))
    }

    fn analysis(&self) -> Result<Analysis, ErrorData> {
        let cache = self.clip.views_dir()?.join(ANALYSIS_FILE);
        let inputs = self.clip.fingerprint();
        if let Some(analysis) = std::fs::read(&cache)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Analysis>(&bytes).ok())
            .filter(|analysis| analysis.version == ANALYSIS_VERSION && analysis.inputs == inputs)
        {
            return Ok(analysis);
        }
        let mut analysis = Analyzer::run(&self.clip)?;
        analysis.inputs = inputs;
        if let Ok(bytes) = serde_json::to_vec_pretty(&analysis) {
            let _ = std::fs::write(&cache, bytes);
        }
        Ok(analysis)
    }

    /// The picture on screen at instant `frame`, from its committed segment.
    fn decode(&mut self, frame: u64) -> Result<DecodedFrame, ErrorData> {
        let segment = self
            .clip
            .segment_of(frame)
            .cloned()
            .ok_or_else(|| self.clip.waiting_error(self.clip.t_of(frame)))?;
        if self
            .decoder
            .as_ref()
            .is_none_or(|(index, _)| *index != segment.index)
        {
            self.decoder = Some((segment.index, self.clip.open_decoder(&segment)?));
        }
        let frame_ms = self.clip.frame_ms();
        let local_ms = self.clip.t_of(frame).saturating_sub(segment.start_ms);
        self.decoder
            .as_mut()
            .map_or_else(
                || Err("decoder unavailable".to_owned()),
                |(_, decoder)| decoder.frame_at(local_ms, frame_ms),
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
        let (width, height) = self.clip.largest_recorded();
        ViewOutput {
            response: ClipViewResponse {
                clip_id: self.clip.clip.clip_id.clone(),
                folder: self.clip.folder.to_string_lossy().into_owned(),
                duration_ms: self.clip.duration_ms(),
                width,
                height,
                fps: self.clip.clip.fps,
                frames_total: self.clip.lines.len() as u64,
                frames_new: self.clip.lines.iter().filter(|line| line.new).count() as u64,
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

/// Shrinks (never enlarges) so the long edge fits `long_edge`; every output
/// pixel is the average of all source pixels it covers (D2).
fn fit(image: image::RgbaImage, long_edge: u32) -> image::RgbaImage {
    let long = image.width().max(image.height());
    if long <= long_edge {
        return image;
    }
    let scale = f64::from(long_edge) / f64::from(long);
    let (width, height) = (scaled(image.width(), scale), scaled(image.height(), scale));
    let (source_width, source_height) = (image.width(), image.height());
    let span = |out: u32, size: u32, source: u32| {
        let start = u64::from(out) * u64::from(source) / u64::from(size);
        let end = (u64::from(out) + 1) * u64::from(source) / u64::from(size);
        (
            u32::try_from(start).unwrap_or(0),
            u32::try_from(end.max(start + 1))
                .unwrap_or(source)
                .min(source),
        )
    };
    image::RgbaImage::from_fn(width, height, |x, y| {
        let (x0, x1) = span(x, width, source_width);
        let (y0, y1) = span(y, height, source_height);
        let mut sum = [0_u64; 4];
        for sy in y0..y1 {
            for sx in x0..x1 {
                for (total, channel) in sum.iter_mut().zip(image.get_pixel(sx, sy).0) {
                    *total += u64::from(channel);
                }
            }
        }
        let count = u64::from((x1 - x0) * (y1 - y0)).max(1);
        image::Rgba(sum.map(|total| u8::try_from(total / count).unwrap_or(u8::MAX)))
    })
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
        "clip folder {folder} (segment-NNNN.mp4, {FRAMES_FILE}, {CLIP_JSON_FILE}); exported images under {VIEWS_DIR}"
    )
}

fn params_invalid(message: String) -> ErrorData {
    mcp_error(error_codes::TOOL_PARAMS_INVALID, message)
}

fn internal(message: String) -> ErrorData {
    mcp_error(error_codes::TOOL_INTERNAL_ERROR, message)
}

fn decode_error(message: String) -> ErrorData {
    internal(format!("segment decode failed: {message}"))
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
        "pass a clip folder written by screenshot operation=record and times inside its committed segments (record_status lists them)"
    } else {
        "check that the segment files are complete (record_status lists their size and SHA-256); record the clip again if one is damaged"
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
