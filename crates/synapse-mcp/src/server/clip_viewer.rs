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
//! comparing them; new frames are compared block by block (8 × 8, recorded
//! resolution) by the D5 equation: a two-threshold block test set from the
//! measured encoder noise; each block against its anchor, its last settled
//! picture; a block back at its anchor within `SHORT_LIVED_MS` is a FLASH, a
//! block that keeps changing animates; block changes with the same outcome
//! join into events. The logged cursor is masked when Windows drew it into
//! the picture, and a blinking caret is ignored.
//!
//! Every image is a labelled PNG copy in the clip's `views` folder (the clip
//! files stay untouched). All decoding and image work runs on a blocking
//! thread, never on the async worker.

use std::collections::{BTreeMap, VecDeque};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use rmcp::model::{CallToolResult, Content};
use serde::{Deserialize, Serialize};
use serde_json::json;
use synapse_core::{Rect, error_codes};

use super::ErrorData;
use super::clip_decoder::{ClipDecoder, DecodedFrame};
use super::clip_exact::{ExactEntry, ExactReader};
use super::clip_motion::{BlockGrid as Grid, BLOCK, MotionTiming, MotionWindow, PixelRect, SHORT_LIVED_MS};
use super::clip_folder::{
    CLIP_JSON_FILE, ClipJson, ClipState, FRAMES_FILE, FrameLine, VIEWS_DIR,
    CommittedSegment, read_frame_lines,
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

/// Exact pixels need only the retained viewer threshold.
const DELTA_LO: u8 = 32;
/// A block has settled after this long the same: K_s = SETTLE_MS · f / 1000.
const SETTLE_MS: u64 = 200;
/// Block changes with the same outcome and overlapping intervals form one
/// event across at most this many empty blocks (columns, rows): wide along a
/// row, so the changed words of one text line are one event, and two blocks
/// between rows (the line gap of a text block) (D5 step 4).
const REGION_GAP_BLOCKS: (usize, usize) = (24, 2);
/// The box masked around the logged cursor, in window pixels (D5 step 5).
const CURSOR_MASK_PX: u32 = 48;
/// A caret (D5 step 5): a change at most this wide and this tall, in window
/// pixels, that alternates between two pictures for at least this many
/// cycles with a half-period in this range (the Windows default is 530 ms).
const CARET_MAX_WIDTH_PX: u32 = 3;
const CARET_HEIGHT_PX: std::ops::RangeInclusive<u32> = 8..=48;
const CARET_MIN_CYCLES: usize = 2;
const CARET_HALF_PERIOD_MS: std::ops::RangeInclusive<u64> = 400..=700;
/// At least one key frame per this interval.
const FLOOR_INTERVAL_MS: u64 = 10_000;
/// Unchanged stretches at least this long are listed in the timeline.
const STATIC_REPORT_MS: u64 = 1_000;
/// Padding around the changed area in the `clip_diff` crop.
const CROP_PADDING: i32 = 16;
const MAX_DIFF_REGIONS: usize = 20;

/// Cached overview analysis, so later pages skip the full decode.
const ANALYSIS_FILE: &str = "overview_analysis.json";
const ANALYSIS_VERSION: u32 = 14;

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
    let mut view = ClipView::new(clip)?;
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
        if viewed.clip.state != ClipState::Ready {
            return Err(viewed.waiting_error(0));
        }
        Ok(viewed)
    }

    fn fps(&self) -> u64 {
        u64::from(self.clip.fps)
    }


    /// The last instant that is in a committed segment.
    fn last_frame(&self) -> u64 { self.lines.last().map_or(0, |line| line.instant) }

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
        self.line(frame).is_some_and(FrameLine::shows_new_picture)
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

    fn scale_at(&self, _frame: u64) -> u32 { 1 }

    fn waiting_error(&self, t_ms: u64) -> ErrorData {
        params_invalid(format!("{t_ms} ms is not viewable: clip state {}; wait for record_status to report ready", self.clip.state.as_str()))
    }

    /// Size and modified time of the files the overview is computed from; a
    /// cached overview is reused only while this is unchanged (also when a new
    /// segment is committed).
    fn fingerprint(&self) -> String {
        let mut names = vec![FRAMES_FILE.to_owned(), CLIP_JSON_FILE.to_owned()];
        names.extend(self.clip.moving.segments.iter().map(|segment| segment.segment.file.clone()));
        let groups: std::collections::BTreeSet<_> = self.lines.iter().filter_map(|line| line.exact.map(|x| x.group)).collect();
        names.extend(groups.into_iter().map(|group| format!("frames/group-{group:06}.zst")));
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


    fn views_dir(&self) -> Result<PathBuf, ErrorData> {
        let views = self.folder.join(VIEWS_DIR);
        std::fs::create_dir_all(&views)
            .map_err(|error| internal(format!("could not create {views:?}: {error}")))?;
        Ok(views)
    }

    /// The largest recorded size among the committed segments.
    fn largest_recorded(&self) -> (u32, u32) {
        self.lines.iter().fold((0, 0), |(w, h), line| (w.max(line.width), h.max(line.height)))
    }

    /// D2: the long edge of overview and range images: at least
    /// `OVERVIEW_MIN_SCALE` of the window size, at most `VIEW_MAX_LONG_EDGE`.
    /// The second value is `true` when the ceiling cuts below the floor.
    fn view_long_edge(&self, requested: Option<u32>) -> (u32, bool) {
        let (width, height) = self.largest_recorded();
        let window_long = width.max(height);
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
        let scale = i64::from(self.scale_at(frame));
        Some((i64::from(x) * scale, i64::from(y) * scale))
    }
}

/// Reconstructs exact pixels, then overlays the numbered sample of the same instant.
struct ClipPictures {
    folder: PathBuf,
    exact: ExactReader,
    lines: Vec<FrameLine>,
    segments: Vec<CommittedSegment>,
    lost: std::collections::BTreeSet<u64>,
    decoder: Option<(u32, ClipDecoder)>,
    fps: u32,
}
impl ClipPictures {
    fn new(clip: &ViewedClip) -> Result<Self, ErrorData> {
        let entries = clip.lines.iter().filter_map(|line| {
            let exact = line.exact?;
            let size = std::fs::metadata(clip.folder.join(format!("frames/group-{:06}.zst", exact.group))).ok()?.len();
            (exact.offset + u64::from(exact.bytes) <= size).then_some(ExactEntry {
                instant: line.instant, exact, width: line.width, height: line.height,
            })
        }).collect();
        Ok(Self { folder: clip.folder.clone(), exact: ExactReader::open(&clip.folder, entries).map_err(decode_error)?,
            lines: clip.lines.clone(), segments: clip.clip.moving.segments.clone(),
            lost: clip.clip.lost.iter().map(|l| l.instant).collect(), decoder: None, fps: clip.clip.fps })
    }
    fn at(&mut self, instant: u64) -> Result<DecodedFrame, ErrorData> {
        const BGRA: usize = 4;
        const BGR: usize = 3;
        const HNS: u64 = 10_000_000;
        let line = self.lines.get(instant as usize).ok_or_else(|| params_invalid("instant outside clip".into()))?;
        let (exact, width, height) = self.exact.picture_at(instant).map_err(decode_error)?;
        let mut bgra: Vec<u8> = exact.chunks_exact(BGR).flat_map(|p| [p[0], p[1], p[2], u8::MAX]).collect();
        if !self.lost.contains(&instant) && let Some(reference) = line.moving
            && let Some(committed) = self.segments.iter().find(|s| s.segment.index == reference.segment && reference.frame < s.segment.frames) {
            let segment = &committed.segment;
            if self.folder.join(&segment.file).is_file() {
                if self.decoder.as_ref().is_none_or(|(index, _)| *index != segment.index) {
                    self.decoder = Some((segment.index, ClipDecoder::open(&self.folder.join(&segment.file)).map_err(decode_error)?));
                }
                let moving = self.decoder.as_mut().ok_or_else(|| internal("decoder unavailable".into()))?.1
                    .frame_number(reference.frame, &segment.key_frames).map_err(decode_error)?;
                let rect = segment.rect;
                if rect.right() > width || rect.bottom() > height || moving.width < rect.w || moving.height < rect.h {
                    return Err(decode_error("moving rectangle does not fit its pictures".into()));
                }
                for row in 0..rect.h as usize {
                    let source = row * moving.width as usize * BGRA;
                    let destination = ((row + rect.y as usize) * width as usize + rect.x as usize) * BGRA;
                    let bytes = rect.w as usize * BGRA;
                    bgra[destination..destination + bytes].copy_from_slice(&moving.bgra[source..source + bytes]);
                }
            }
        }
        Ok(DecodedFrame { t_hns: (instant * HNS / u64::from(self.fps)) as i64, width, height, bgra })
    }
}

// ---------------------------------------------------------------------------
// Change test (D5 step 1: the block test from the measured noise)
// ---------------------------------------------------------------------------

/// The pixel test: the largest R, G or B difference of two pixels.
fn pixel_delta(a: &[u8], b: &[u8]) -> u8 {
    a.iter()
        .zip(b)
        .map(|(x, y)| x.abs_diff(*y))
        .max()
        .unwrap_or(0)
}

#[derive(Clone)]
struct Picture {
    grid: Grid,
    rgb: Vec<u8>,
}

impl Picture {
    fn new(frame: &DecodedFrame) -> Self {
        Self {
            grid: Grid::new(frame.width as usize, frame.height as usize),
            rgb: frame
                .bgra
                .chunks_exact(4)
                .flat_map(|px| [px[2], px[1], px[0]])
                .collect(),
        }
    }

    /// D5 step 1 for block `b` of this picture and of `other` (same size).
    fn test(&self, other: &Self, b: usize) -> bool {
        self.grid.differs(&self.rgb, &other.rgb, b, DELTA_LO)
    }

    /// A copy of the pixels of block `b`, row after row.
    fn block(&self, b: usize) -> Vec<u8> {
        self.grid
            .rows_of(b)
            .flat_map(|row| self.rgb.get(row).unwrap_or_default().iter().copied())
            .collect()
    }

    /// Copies block `b` from `other` (same size).
    fn copy_block(&mut self, other: &Self, b: usize) {
        for row in self.grid.rows_of(b) {
            if let (Some(to), Some(from)) = (self.rgb.get_mut(row.clone()), other.rgb.get(row)) {
                to.copy_from_slice(from);
            }
        }
    }

    /// The box of the pixels of block `b` that differ from `other` by more
    /// than δ_lo: x0, y0, x1, y1 (inclusive, recorded pixels).
    fn footprint(&self, other: &Self, b: usize) -> Option<(usize, usize, usize, usize)> {
        let (x0, y0, ..) = self.grid.span(b);
        let mut bounds: Option<(usize, usize, usize, usize)> = None;
        for (y, row) in (y0..).zip(self.grid.rows_of(b)) {
            let mine = self.rgb.get(row.clone()).unwrap_or_default();
            let theirs = other.rgb.get(row).unwrap_or_default();
            for (x, (a, c)) in (x0..).zip(mine.chunks_exact(3).zip(theirs.chunks_exact(3))) {
                if pixel_delta(a, c) > DELTA_LO {
                    bounds = Some(bounds.map_or((x, y, x, y), |(a0, b0, a1, b1)| {
                        (a0.min(x), b0.min(y), a1.max(x), b1.max(y))
                    }));
                }
            }
        }
        bounds
    }
}

/// Which blocks differ between two pictures of the same size (`clip_diff`).
struct BlockMap {
    grid: Grid,
    changed: Vec<bool>,
    count: u64,
}

impl BlockMap {
    fn between(before: &Picture, after: &Picture) -> Self {
        let changed: Vec<bool> = (0..after.grid.blocks())
            .map(|b| after.test(before, b))
            .collect();
        let count = changed.iter().filter(|flag| **flag).count() as u64;
        Self {
            grid: after.grid,
            changed,
            count,
        }
    }

    /// The changed blocks, as indices.
    fn indices(&self) -> impl Iterator<Item = usize> + '_ {
        self.changed
            .iter()
            .enumerate()
            .filter_map(|(index, flag)| flag.then_some(index))
    }

    fn percent(&self) -> f64 {
        self.grid.percent(self.count)
    }

    fn bbox(&self) -> Option<Rect> {
        self.grid.bbox(self.indices()).map(view_rect)
    }

    /// Connected groups of changed blocks (8-neighbour), largest first, as
    /// rectangles.
    fn regions(&self) -> Vec<ClipChangedRegion> {
        let mut seen = vec![false; self.changed.len()];
        let mut regions = Vec::new();
        for start in self.indices() {
            if seen[start] {
                continue;
            }
            seen[start] = true;
            let (mut stack, mut group) = (vec![start], Vec::new());
            while let Some(b) = stack.pop() {
                group.push(b);
                let (col, row) = self.grid.place(b);
                for near_row in row.saturating_sub(1)..=(row + 1).min(self.grid.rows - 1) {
                    for near_col in col.saturating_sub(1)..=(col + 1).min(self.grid.cols - 1) {
                        let next = near_row * self.grid.cols + near_col;
                        if self.changed[next] && !seen[next] {
                            seen[next] = true;
                            stack.push(next);
                        }
                    }
                }
            }
            if let Some(rect) = self.grid.bbox(group.iter().copied()) {
                let rect = view_rect(rect);
                regions.push(ClipChangedRegion {
                    rect,
                    changed_blocks: group.len() as u64,
                });
            }
        }
        regions.sort_by_key(|region| std::cmp::Reverse(region.changed_blocks));
        regions
    }
}

// ---------------------------------------------------------------------------
// Overview analysis (D5 steps 2 to 6)
// ---------------------------------------------------------------------------

/// D5's times in instants of the clip (f instants per second).
#[derive(Clone, Copy)]
struct Instants {
    /// K_s = SETTLE_MS · f / 1000.
    settle: u64,
    motion: MotionTiming,
}

impl Instants {
    fn of(clip: &ViewedClip) -> Self {
        let per_ms = |ms: u64| (ms * clip.fps() / 1_000).max(1);
        Self { settle: per_ms(SETTLE_MS), motion: MotionTiming::of(clip.clip.fps) }
    }
}

/// The pictures shown in the last K_s instants, each with the instant it was
/// first shown, the newest last: a block settles against them (D5 step 2).
struct Recent {
    pictures: VecDeque<(u64, Picture)>,
    settle: u64,
}

impl Recent {
    fn latest(&self) -> Option<&(u64, Picture)> {
        self.pictures.back()
    }

    fn restart(&mut self, instant: u64, picture: Picture) {
        self.pictures.clear();
        self.pictures.push_back((instant, picture));
    }

    fn push(&mut self, instant: u64, picture: Picture) {
        self.pictures.push_back((instant, picture));
        // The oldest picture stays while it was on screen K_s instants ago.
        let settle = self.settle;
        while self
            .pictures
            .get(1)
            .is_some_and(|(next, _)| next + settle <= instant)
        {
            self.pictures.pop_front();
        }
    }

    /// The first instant since which block `b` has shown only pictures that
    /// are SAME as `picture` (`now` when the last one is not).
    fn stable_since(&self, picture: &Picture, b: usize, now: u64) -> u64 {
        let mut since = now;
        for (first, earlier) in self.pictures.iter().rev() {
            if picture.test(earlier, b) {
                break;
            }
            since = *first;
        }
        since
    }
}

/// What a change of a block turned out to be.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Outcome {
    /// It settled on a new picture.
    Changed,
    /// It came back to its anchor within `SHORT_LIVED_MS`.
    Flash,
    /// It came back to its anchor later.
    Reverted,
    /// It kept changing (D5 step 3).
    Animation,
}

/// An open change of one block: from the first instant it was not SAME as
/// its anchor until it settles.
#[derive(Clone)]
struct Activity {
    start: u64,
    /// The last instant it changed against the previous frame.
    last_move: u64,
    /// The last instant of this change at which it animated: the end of its
    /// animated stretch; `None` when it never animated.
    animated_until: Option<u64>,
    /// After its animated stretch: the first instant it was not SAME as its
    /// anchor, the start of a later excursion (D5 step 3, owner decision on
    /// issue #293).
    left_after_animation: Option<u64>,
    /// The instants at which it was DIFF from its anchor.
    differed: Vec<u64>,
}

/// How an open change of a block ends.
#[derive(Clone, Copy)]
enum Close {
    /// It settled; the instant its settling was confirmed.
    Settled(u64),
    /// The clip end or a resize cut it before it settled; the first instant
    /// after the boundary.
    Cut(u64),
}

/// The live state of one block (D5 steps 2 and 3).
#[derive(Clone, Default)]
struct BlockState {
    /// The instants of its last changes against the previous frame: at most
    /// as many as make it animate.
    moves: MotionWindow,
    recorded_until: Option<u64>,
    /// The first instant of the picture it shows now (SAME since then).
    stable_since: u64,
    /// While quiet: the first instant of its current run of pictures that
    /// are not SAME as its anchor. An undecided picture is not yet a change
    /// and never moves the anchor; it only dates the start of the change
    /// once the block becomes DIFF (D5 step 2).
    pending: Option<u64>,
    active: Option<Activity>,
}

impl BlockState {
    /// The first instant at which the block no longer animates: fewer than
    /// `Instants::animating` changes in the W_a that ends there.
    fn animating_until(&self, instants: Instants) -> u64 {
        self.moves.animating_until(&instants.motion).max(self.recorded_until.unwrap_or(0))
    }

    /// When an open change settles if nothing changes any more: SAME for K_s
    /// instants and not animating.
    fn settle_at(&self, instants: Instants) -> Option<u64> {
        self.active.as_ref()?;
        Some((self.stable_since + instants.settle).max(self.animating_until(instants)))
    }
}

/// How a change of one block ended: its outcome and interval, and what the
/// events, picks and caret test need.
#[derive(Clone)]
struct BlockEpisode {
    outcome: Outcome,
    block: usize,
    grid: Grid,
    generation: u64,
    start: u64,
    end: u64,
    /// The instant its settling was confirmed: quiet for K_s instants and
    /// not animating (D5 step 2); the last instant before the boundary when
    /// the clip end or a resize closed it. CHANGED picks it (D5 step 6).
    settle_frame: u64,
    /// Quiet for `SETTLE_MS` when it ended; `false` when the clip end or a
    /// resize closed it earlier.
    settled: bool,
    /// An animation still running when the clip end or a resize closed it.
    running: bool,
    /// FLASH, REVERTED: the instants at which the block was DIFF from its
    /// anchor.
    differed: Vec<u64>,
    /// CHANGED: the anchor and the new picture of the block, and the box of
    /// the pixels that changed.
    before: Vec<u8>,
    after: Vec<u8>,
    footprint: Option<(usize, usize, usize, usize)>,
}

/// One decode pass over the committed segments (D5 steps 2 and 3): each
/// block is compared with its anchor, and each change of a block ends as a
/// `BlockEpisode`; `finish` joins them into events.
struct Analyzer<'a> {
    clip: &'a ViewedClip,
    instants: Instants,
    /// A_b of every block: its last settled picture.
    anchor: Picture,
    recent: Recent,
    states: Vec<BlockState>,
    episodes: Vec<BlockEpisode>,
    resizes: Vec<(u64, usize, usize)>,
    /// Goes up on every size change; changes are compared and joined only
    /// within one generation, also when a later resize restores the size.
    generation: u64,
    /// Captured size (frames.jsonl) of the picture last decoded.
    captured: (u32, u32),
    /// New frames in which no block changed against the previous frame.
    still_frames: u64,
}

impl<'a> Analyzer<'a> {
    fn run(clip: &'a ViewedClip) -> Result<Analysis, ErrorData> {
        let instants = Instants::of(clip);
        let mut analyzer = Self {
            clip,
            instants,
            anchor: Picture {
                grid: Grid::new(0, 0),
                rgb: Vec::new(),
            },
            recent: Recent {
                pictures: VecDeque::new(),
                settle: instants.settle,
            },
            states: Vec::new(),
            episodes: Vec::new(),
            resizes: Vec::new(),
            generation: 0,
            captured: (0, 0),
            still_frames: 0,
        };
        let mut pictures = ClipPictures::new(clip)?;
        for line in &clip.lines {
            if line.shows_new_picture() {
                let frame = pictures.at(line.instant)?;
                analyzer.step(line.instant, &frame);
            }
        }
        analyzer.close_all(clip.last_frame() + 1);
        Ok(analyzer.finish())
    }

    fn step(&mut self, instant: u64, frame: &DecodedFrame) {
        let picture = Picture::new(frame);
        // A resize is a change of the captured size in frames.jsonl: an odd
        // size padded up to even can decode at the same size as before.
        let captured = self
            .clip
            .line(instant)
            .filter(|line| line.width != 0 && line.height != 0)
            .map_or(self.captured, |line| (line.width, line.height));
        if picture.grid != self.anchor.grid || captured != self.captured {
            self.captured = captured;
            if self.recent.latest().is_some() {
                self.close_all(instant);
                self.resizes
                    .push((instant, picture.grid.width, picture.grid.height));
            }
            self.generation += 1;
            self.states = vec![BlockState::default(); picture.grid.blocks()];
            self.anchor = picture.clone();
            self.recent.restart(instant, picture);
            return;
        }
        if !self.clip.is_new(instant) {
            return;
        }
        self.settle_due(instant.saturating_sub(1));
        let masked = self.cursor_blocks(instant);
        let recorded = self.clip.line(instant).and_then(|line| line.moving).and_then(|reference| self.clip.clip.moving.segments.iter().find(|s| s.segment.index == reference.segment)).map(|s| (s.segment.rect, s.segment.last_instant + 1));
        let instants = self.instants;
        let Some((_, previous)) = self.recent.latest() else {
            return;
        };
        let mut moved_any = false;
        for (b, state) in self.states.iter_mut().enumerate() {
            if masked.contains(&b) {
                // The cursor hides the block: an open change waits for it.
                if state.active.is_some() {
                    state.stable_since = instant;
                }
                continue;
            }
            let stored_moving = recorded.is_some_and(|(rect, _)| picture.grid.bbox([b]).is_some_and(|block| rect.intersect(&block).is_some()));
            let moved = stored_moving || picture.test(previous, b);
            if moved {
                moved_any = true;
                state.moves.moved(instant, &instants.motion);
                if stored_moving { state.recorded_until = recorded.map(|(_, until)| until); }
            }
            // D5 step 3 from the frame-to-frame test alone: a block can
            // animate while every picture stays SAME as its anchor.
            let animating = stored_moving || instant < state.animating_until(instants);
            let to_anchor = !stored_moving && picture.test(&self.anchor, b);
            if state.active.is_none() {
                if !to_anchor {
                    state.pending = None;
                } else {
                    state.pending = state.pending.or(Some(instant));
                }
                if !to_anchor && !animating {
                    continue;
                }
                // quiet -> active on DIFF, or once it animates; the start is
                // the first instant that was not SAME, or the first motion
                // of the animation when that is earlier.
                let first_move = state.moves.first_move().filter(|_| animating);
                let start = state
                    .pending
                    .take()
                    .into_iter()
                    .chain(first_move)
                    .min()
                    .unwrap_or(instant);
                state.active = Some(Activity {
                    start,
                    last_move: instant,
                    animated_until: None,
                    left_after_animation: None,
                    differed: Vec::new(),
                });
            }
            let Some(activity) = state.active.as_mut() else {
                continue;
            };
            if to_anchor {
                activity.differed.push(instant);
            }
            if moved {
                activity.last_move = instant;
            }
            if animating {
                activity.animated_until = Some(instant);
                activity.left_after_animation = None;
            } else if activity.animated_until.is_some() && to_anchor {
                activity.left_after_animation = activity.left_after_animation.or(Some(instant));
            }
            state.stable_since = if stored_moving { instant } else { self.recent.stable_since(&picture, b, instant) };
        }
        if !moved_any {
            self.still_frames += 1;
        }
        self.recent.push(instant, picture);
        self.settle_due(instant);
    }

    /// Ends the open change of every block that has settled by `now`.
    fn settle_due(&mut self, now: u64) {
        for b in 0..self.states.len() {
            if let Some(at) = self.states[b]
                .settle_at(self.instants)
                .filter(|at| *at <= now)
            {
                self.end_change(b, Close::Settled(at));
            }
        }
    }

    /// Ends every open change at a resize or at the clip end (`end`, the
    /// first instant after it). The quiet time is measured at the boundary:
    /// a block quiet for at least `SETTLE_MS` up to `end` has settled; its
    /// settle frame is then at most the last instant before `end`.
    fn close_all(&mut self, end: u64) {
        let last = end.saturating_sub(1);
        for b in 0..self.states.len() {
            match self.states[b].settle_at(self.instants) {
                Some(at) if at <= end => self.end_change(b, Close::Settled(at.min(last))),
                Some(_) => self.end_change(b, Close::Cut(end)),
                None => {}
            }
        }
    }

    /// D5 steps 2 and 3: block `b` has settled or is closed by the clip end
    /// or a resize; its picture is compared with its anchor.
    fn end_change(&mut self, b: usize, close: Close) {
        let Some(mut activity) = self.states[b].active.take() else {
            return;
        };
        let Some((shown_at, picture)) = self.recent.latest() else {
            return;
        };
        let state = &self.states[b];
        // Settling is confirmed no earlier than the picture it settles on was
        // decoded: a newer picture SAME as older ones moves stable_since back,
        // so `at` can lie before it. `shown_at` is before any boundary.
        let settle_frame = match close {
            Close::Settled(at) => at.max(*shown_at),
            Close::Cut(end) => end.saturating_sub(1),
        };
        let mut episode = BlockEpisode {
            outcome: Outcome::Changed,
            block: b,
            grid: picture.grid,
            generation: self.generation,
            start: activity.start,
            end: state.stable_since.max(activity.start),
            settle_frame: settle_frame.max(activity.start),
            settled: matches!(close, Close::Settled(_)),
            running: false,
            differed: Vec::new(),
            before: Vec::new(),
            after: Vec::new(),
            footprint: None,
        };
        // A return to the anchor starts at the first instant that was not
        // SAME; after an animation, at the first such instant after the
        // stretch.
        let mut return_start = activity.start;
        if let Some(animated_until) = activity.animated_until {
            let running = match close {
                Close::Cut(end) => end <= state.animating_until(self.instants),
                Close::Settled(_) => false,
            };
            let end = match close {
                Close::Cut(end) if running => end.saturating_sub(1),
                _ => activity.last_move,
            };
            self.episodes.push(BlockEpisode {
                outcome: Outcome::Animation,
                end,
                running,
                ..episode.clone()
            });
            // While it animates, its anchor stays frozen and it makes no event;
            // once it stops, it is compared like any settling block. That change
            // starts where the motion stopped, so a checkmark that replaces a
            // spinner starts at the checkmark. Back at its anchor, the animated
            // stretch makes no FLASH or REVERTED: the animation already reports
            // that interval (a playhead would otherwise leave a short-lived
            // change in every block it crossed). Owner decision, issue #293:
            // only that stretch is exempt; a later excursion of the same block
            // away from its anchor is classified as usual.
            if running {
                return;
            }
            episode.start = activity.last_move.min(episode.end);
            activity.differed.retain(|at| *at > animated_until);
            return_start = activity
                .left_after_animation
                .or(activity.differed.first().copied())
                .map_or(episode.start, |first| first.min(episode.end));
        }
        match picture.test(&self.anchor, b) {
            true => {
                episode.before = self.anchor.block(b);
                episode.after = picture.block(b);
                episode.footprint = picture.footprint(&self.anchor, b);
                self.anchor.copy_block(picture, b);
                self.episodes.push(episode);
            }
            false if !activity.differed.is_empty() => {
                episode.start = return_start;
                let span = self.clip.t_of(episode.end) - self.clip.t_of(episode.start);
                episode.outcome = if span <= SHORT_LIVED_MS {
                    Outcome::Flash
                } else {
                    Outcome::Reverted
                };
                episode.differed = activity.differed;
                self.episodes.push(episode);
            }
            false => {}
            // Drift or noise: the block takes the picture without an event.
        }
    }

    /// D5 step 5: the blocks under the logged cursor box, now and at the
    /// last decoded instant, when the cursor is in the picture.
    fn cursor_blocks(&self, instant: u64) -> Vec<usize> {
        let grid = self.anchor.grid;
        let previous = self.recent.latest().map_or(instant, |(at, _)| *at);
        let to_block = |value: i64| usize::try_from(value).unwrap_or(0) / BLOCK;
        let mut blocks = Vec::new();
        for at in [previous, instant] {
            let Some((x, y)) = self.clip.cursor_at(at) else {
                continue;
            };
            let box_px = i64::from(CURSOR_MASK_PX * self.clip.scale_at(at));
            let (x0, y0) = ((x - box_px / 2).max(0), (y - box_px / 2).max(0));
            for row in to_block(y0)..=to_block(y0 + box_px).min(grid.rows.saturating_sub(1)) {
                for col in to_block(x0)..=to_block(x0 + box_px).min(grid.cols.saturating_sub(1)) {
                    blocks.push(row * grid.cols + col);
                }
            }
        }
        blocks
    }

    fn finish(self) -> Analysis {
        let clip = self.clip;
        let mut events = Event::join_all(self.episodes);
        events.sort_by_key(|event| (event.start, event.end));
        let (events, carets) = Event::without_carets(events, clip);
        let first = 0;
        let mut picks = vec![pick(clip, first, "first frame")];
        let mut timeline = Vec::new();
        // D5 step 6: a stable interval that an animation overlaps is not
        // listed as "no change".
        let mut quiet_from = first;
        for event in &events {
            if clip.t_of(event.start).saturating_sub(clip.t_of(quiet_from)) >= STATIC_REPORT_MS {
                timeline.push(static_entry(
                    clip,
                    quiet_from,
                    event.start.saturating_sub(1),
                ));
            }
            quiet_from = quiet_from.max(event.end);
            picks.extend(event.picks(clip));
            timeline.push(event.entry(clip, self.generation));
        }
        let last = clip.last_frame();
        if clip.t_of(last).saturating_sub(clip.t_of(quiet_from)) >= STATIC_REPORT_MS {
            timeline.push(static_entry(clip, quiet_from, last));
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

        let count = |outcome: Outcome| {
            events
                .iter()
                .filter(|event| event.outcome == outcome)
                .count()
        };
        let repeated = clip.lines.iter().filter(|line| !line.new).count();
        let mut notes = vec![
            format!(
                "{} changes, {} short-lived changes, {} changes that returned and {} animations found; {repeated} repeated frames skipped by frames.jsonl flags; {} new frames showed no visible change",
                count(Outcome::Changed),
                count(Outcome::Flash),
                count(Outcome::Reverted),
                count(Outcome::Animation),
                self.still_frames
            ),
            format!(
                "change test: {BLOCK}x{BLOCK} exact blocks differ when any R/G/B byte differs by more than {DELTA_LO}; stored moving areas animate without pixel-noise tests; settled after {SETTLE_MS} ms without change; short-lived = back within {SHORT_LIVED_MS} ms"
            ),
        ];
        if clip.clip.observer.cursor_in_picture {
            notes.push(format!(
                "the cursor is in the picture on this Windows; a {CURSOR_MASK_PX} px box at its logged position is masked"
            ));
        }
        if carets > 0 {
            notes.push(format!("{carets} blinking caret toggles ignored"));
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
        let unsettled = events
            .iter()
            .filter(|event| event.outcome != Outcome::Animation && !event.settled)
            .count();
        if unsettled > 0 {
            notes.push(format!(
                "{unsettled} changes had not settled when the clip ended or the window size changed"
            ));
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

/// One reported change (D5 step 4): block episodes with the same outcome
/// whose intervals overlap and whose blocks are 8-neighbours, a gap of
/// `REGION_GAP_BLOCKS` allowed.
struct Event {
    outcome: Outcome,
    grid: Grid,
    generation: u64,
    start: u64,
    end: u64,
    /// Its blocks, each once, in index order.
    blocks: Vec<usize>,
    /// CHANGED: the latest settle frame of its blocks, the frame it is
    /// pictured at (D5 step 6).
    settle_frame: u64,
    settled: bool,
    running: bool,
    /// FLASH, REVERTED: the instant at which most of its blocks were DIFF
    /// from their anchors.
    peak: u64,
    /// CHANGED: per block (index order) the anchor and the new picture, and
    /// the box of the pixels that changed.
    pictures: Vec<(usize, Vec<u8>, Vec<u8>)>,
    footprint: Option<(usize, usize, usize, usize)>,
}

impl Event {
    fn join_all(mut episodes: Vec<BlockEpisode>) -> Vec<Self> {
        episodes.sort_by_key(|episode| (episode.generation, episode.outcome, episode.start));
        let mut root: Vec<usize> = (0..episodes.len()).collect();
        // Episodes of the same kind whose interval still reaches the start
        // of the next one.
        let mut open: Vec<usize> = Vec::new();
        for (index, episode) in episodes.iter().enumerate() {
            open.retain(|&other| {
                let other = &episodes[other];
                (other.generation, other.outcome) == (episode.generation, episode.outcome)
                    && other.end >= episode.start
            });
            for &other in &open {
                if episode
                    .grid
                    .near(episode.block, episodes[other].block, REGION_GAP_BLOCKS)
                {
                    let (a, b) = (find_root(&mut root, index), find_root(&mut root, other));
                    root[a] = b;
                }
            }
            open.push(index);
        }
        let mut groups: BTreeMap<usize, Vec<BlockEpisode>> = BTreeMap::new();
        for (index, episode) in episodes.into_iter().enumerate() {
            groups
                .entry(find_root(&mut root, index))
                .or_default()
                .push(episode);
        }
        groups.into_values().map(Self::from_episodes).collect()
    }

    fn from_episodes(mut group: Vec<BlockEpisode>) -> Self {
        group.sort_by_key(|episode| (episode.block, episode.start));
        let (outcome, grid, generation) = (group[0].outcome, group[0].grid, group[0].generation);
        let start = group.iter().map(|episode| episode.start).min().unwrap_or(0);
        let end = group
            .iter()
            .map(|episode| episode.end)
            .max()
            .unwrap_or(start);
        let mut differed: BTreeMap<u64, usize> = BTreeMap::new();
        for at in group.iter().flat_map(|episode| &episode.differed) {
            *differed.entry(*at).or_default() += 1;
        }
        let peak = differed
            .iter()
            .max_by_key(|(at, count)| (**count, std::cmp::Reverse(**at)))
            .map_or(start, |(at, _)| *at);
        let settle_frame = group
            .iter()
            .map(|episode| episode.settle_frame)
            .max()
            .unwrap_or(end);
        let mut blocks: Vec<usize> = group.iter().map(|episode| episode.block).collect();
        blocks.dedup();
        Self {
            outcome,
            grid,
            generation,
            start,
            end,
            blocks,
            settle_frame,
            settled: group.iter().all(|episode| episode.settled),
            running: group.iter().any(|episode| episode.running),
            peak,
            footprint: group.iter().filter_map(|episode| episode.footprint).reduce(
                |(a0, b0, a1, b1), (x0, y0, x1, y1)| {
                    (a0.min(x0), b0.min(y0), a1.max(x1), b1.max(y1))
                },
            ),
            pictures: group
                .into_iter()
                .filter(|episode| !episode.after.is_empty())
                .map(|episode| (episode.block, episode.before, episode.after))
                .collect(),
        }
    }

    /// D5 step 5: removes blinking carets, thin changes in one place that
    /// alternate between two pictures (one of them the anchor) for at least
    /// `CARET_MIN_CYCLES` cycles with a half-period in `CARET_HALF_PERIOD_MS`.
    /// `events` are in start order; returns the rest and the toggles removed.
    fn without_carets(events: Vec<Self>, clip: &ViewedClip) -> (Vec<Self>, usize) {
        let mut places: BTreeMap<(u64, &[usize]), Vec<usize>> = BTreeMap::new();
        for (index, event) in events.iter().enumerate() {
            if event.caret_shaped(clip.scale_at(event.start) as usize) {
                places
                    .entry((event.generation, event.blocks.as_slice()))
                    .or_default()
                    .push(index);
            }
        }
        let mut caret = vec![false; events.len()];
        for toggles in places.values() {
            let runs = toggles.chunk_by(|&earlier, &later| {
                let half = clip
                    .t_of(events[later].start)
                    .saturating_sub(clip.t_of(events[earlier].start));
                CARET_HALF_PERIOD_MS.contains(&half) && events[later].undoes(&events[earlier])
            });
            for run in runs.filter(|run| run.len() >= 2 * CARET_MIN_CYCLES) {
                for &index in run {
                    caret[index] = true;
                }
            }
        }
        let removed = caret.iter().filter(|caret| **caret).count();
        let kept = events
            .into_iter()
            .zip(caret)
            .filter_map(|(event, caret)| (!caret).then_some(event))
            .collect();
        (kept, removed)
    }

    /// A CHANGED event at most `CARET_MAX_WIDTH_PX` wide and
    /// `CARET_HEIGHT_PX` tall, each of its blocks changed once.
    fn caret_shaped(&self, scale: usize) -> bool {
        let to_window =
            |pixels: usize| u32::try_from(pixels.div_ceil(scale.max(1))).unwrap_or(u32::MAX);
        self.outcome == Outcome::Changed
            && self.pictures.len() == self.blocks.len()
            && self.footprint.is_some_and(|(x0, y0, x1, y1)| {
                to_window(x1 + 1 - x0) <= CARET_MAX_WIDTH_PX
                    && CARET_HEIGHT_PX.contains(&to_window(y1 + 1 - y0))
            })
    }

    /// `true` when this change brings every block back to the picture it
    /// had before `earlier`.
    fn undoes(&self, earlier: &Self) -> bool {
        self.pictures.len() == earlier.pictures.len()
            && self.pictures.iter().zip(&earlier.pictures).all(
                |((block, _, after), (earlier_block, before, _))| {
                    block == earlier_block
                        && after.iter().zip(before).all(|(a, b)| a.abs_diff(*b) <= DELTA_LO)
                },
            )
    }

    /// D5 step 6: one image per event.
    fn picks(&self, clip: &ViewedClip) -> Vec<Pick> {
        match self.outcome {
            Outcome::Changed => vec![pick(clip, self.settle_frame, "settled frame after change")],
            Outcome::Flash => vec![pick(clip, self.peak, "short-lived change")],
            Outcome::Reverted => vec![pick(clip, self.peak, "change that returned")],
            Outcome::Animation if self.running => vec![pick(clip, self.start, "animating region")],
            Outcome::Animation => vec![
                pick(clip, self.start, "animating region"),
                pick(clip, self.end, "animation end"),
            ],
        }
    }

    fn entry(&self, clip: &ViewedClip, last_generation: u64) -> ClipTimelineEntry {
        let bbox = self.grid.bbox(self.blocks.iter().copied()).map(view_rect);
        if self.outcome == Outcome::Animation {
            return region_entry(
                clip,
                "animating",
                self.start,
                self.end,
                self.blocks.len(),
                bbox,
                "region kept changing (animation)",
            );
        }
        let (start_ms, end_ms) = (clip.t_of(self.start), clip.t_of(self.end));
        let area = bbox.map_or_else(String::new, |r| {
            format!(" in x={} y={} w={} h={}", r.x, r.y, r.w, r.h)
        });
        let outcome = match self.outcome {
            Outcome::Flash => format!(
                "short-lived: back to the previous picture at {}",
                fmt_time(end_ms)
            ),
            Outcome::Reverted => {
                format!("returned to the previous picture at {}", fmt_time(end_ms))
            }
            _ if self.settled => format!(
                "settled at {} (frame {}), confirmed at frame {}",
                fmt_time(end_ms),
                self.end,
                self.settle_frame
            ),
            _ if self.generation == last_generation => {
                "still changing when the clip ended".to_owned()
            }
            _ => "still changing when the window size changed".to_owned(),
        };
        let count = self.blocks.len() as u64;
        ClipTimelineEntry {
            kind: if self.outcome == Outcome::Flash {
                "short_lived"
            } else {
                "change"
            }
            .to_owned(),
            start_ms,
            end_ms,
            start_frame: self.start,
            end_frame: self.end,
            changed_blocks: count,
            bbox,
            text: format!(
                "{}-{} change: {count} blocks ({}% of the picture){area}; {outcome}",
                fmt_time(start_ms),
                fmt_time(end_ms),
                self.grid.percent(count)
            ),
        }
    }
}

/// The root of `index` in a union-find forest, halving the path on the way.
fn find_root(root: &mut [usize], mut index: usize) -> usize {
    while root[index] != index {
        root[index] = root[root[index]];
        index = root[index];
    }
    index
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

// ---------------------------------------------------------------------------
// The four operations
// ---------------------------------------------------------------------------

/// One viewer call: the clip, the decoder of the segment in use and the images
/// made so far.
struct ClipView {
    clip: ViewedClip,
    pictures: ClipPictures,
    images: Vec<ClipViewImage>,
    pngs: Vec<Vec<u8>>,
    timeline: Vec<ClipTimelineEntry>,
    regions: Vec<ClipChangedRegion>,
    paging: Option<(u32, u32)>,
    notes: Vec<String>,
    text: String,
}

impl ClipView {
    fn new(clip: ViewedClip) -> Result<Self, ErrorData> {
        let pictures = ClipPictures::new(&clip)?;
        Ok(Self {
            clip,
            pictures,
            images: Vec::new(),
            pngs: Vec::new(),
            timeline: Vec::new(),
            regions: Vec::new(),
            paging: None,
            notes: Vec::new(),
            text: String::new(),
        })
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
            let frame = self.pictures.at(pick.frame)?;
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
            let frame = self.pictures.at(number)?;
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
        let frame = self.pictures.at(number)?;
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
        let before = self.pictures.at(from)?;
        let after = self.pictures.at(to)?;
        if (before.width, before.height) != (after.width, after.height) {
            let _ = writeln!(
                self.text,
                "- the window size changed between the two times ({}x{} -> {}x{}); compare them with clip_frame",
                before.width, before.height, after.width, after.height
            );
            return Ok(());
        }
        let mut blocks = BlockMap::between(&Picture::new(&before), &Picture::new(&after));
        for instant in [from, to] {
            let moving = self.clip.line(instant).and_then(|line| line.moving)
                .and_then(|reference| self.clip.clip.moving.segments.iter().find(|s| s.segment.index == reference.segment));
            if let Some(moving) = moving {
                for b in 0..blocks.grid.blocks() {
                    if !blocks.changed[b] && blocks.grid.bbox([b]).is_some_and(|r| moving.segment.rect.intersect(&r).is_some()) {
                        blocks.changed[b] = true; blocks.count += 1;
                    }
                }
            }
        }
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

fn view_rect(rectangle: PixelRect) -> Rect {
    rect(rectangle.x as usize, rectangle.y as usize, rectangle.w as usize, rectangle.h as usize)
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
        "clip folder {folder} (frames/, moving/, {FRAMES_FILE}, {CLIP_JSON_FILE}); exported images under {VIEWS_DIR}"
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
