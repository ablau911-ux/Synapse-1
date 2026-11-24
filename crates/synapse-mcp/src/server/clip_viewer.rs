//! Clip viewer: `screenshot operation=clip_overview | clip_frames | clip_frame
//! | clip_diff` (docs/clip-recorder-plan-5.0.md, section 5).
//!
//! The reader of a clip is an AI model, so a clip folder is read back as a
//! text timeline plus a few labelled PNG pictures, and every answer is true
//! to what was recorded:
//!
//! - every picture says what it is: `exact still` (a lossless still of a
//!   settle point: the window's exact pixels) or `video` (a decoded AV1
//!   picture: video quality); a `lost` instant is never returned as a picture,
//!   the answer names the loss, its cause and the nearest recorded instants;
//! - `clip_overview`: the timeline straight from `frames.jsonl` (changed
//!   stretches, settle points, unchanged stretches, lost ranges; nothing is
//!   decoded for it) and key pictures spread over the whole clip on every
//!   page;
//! - `clip_frames`: pictures in a time range, as many as the call budget
//!   allows; unchanged repeats are named, not sent again;
//! - `clip_frame`: one picture at recorded pixels, optional crop and x2
//!   close-up;
//! - `clip_diff`: changed regions between two exact stills. Change analysis
//!   on video is not measured yet (plan open item O2), so video pictures are
//!   not compared.
//!
//! Every image is a labelled PNG copy in the clip's `views` folder (the clip
//! files stay untouched); the label sits in a band above the picture, never
//! over it. All decoding and image work runs on a blocking thread.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use image::RgbaImage;
use rmcp::model::{CallToolResult, Content};
use serde_json::json;
use synapse_core::{Rect, error_codes};

use super::ErrorData;
use super::clip_decoder::{ClipDecoder, ClipPicture, PictureKind, PictureSource};
use super::clip_folder::{
    CLIP_JSON_FILE, CLIP_METHOD, CLIP_SCHEMA_VERSION, ClipJson, ClipState, FRAMES_FILE, IndexLine,
    STILLS_DIR, VIDEO_DIR, VIEWS_DIR, read_index,
};
use super::clip_stream::SETTLE_INSTANTS;
use crate::m1::clip_view_limits::{MAX_FRAMES_FPS, MIN_CROP_SHORT_SIDE};
use crate::m1::{
    ClipChangedRegion, ClipLostInstant, ClipTimelineEntry, ClipViewImage, ClipViewParams,
    ClipViewResponse, ScreenshotOperation, ScreenshotResponse, mcp_error,
};

/// N: upper bound on images per call; the token budget may allow fewer.
const MAX_IMAGES_PER_CALL: usize = 8;
/// Default image long edge: standard-tier Claude models downsize anything
/// larger. `max_long_edge` overrides it.
const VIEW_MAX_LONG_EDGE: u32 = 1_568;
/// Below this share of the window size small text may become unreadable.
const READABLE_MIN_SCALE: f64 = 0.75;
/// Token budget of the overview's images and of its text.
const OVERVIEW_IMAGE_TOKEN_BUDGET: u64 = 20_000;
const OVERVIEW_TEXT_TOKEN_BUDGET: u64 = 4_000;
/// Token budget of every other viewer call.
const CALL_TOKEN_BUDGET: u64 = 10_000;
/// The token formula: tokens = N · w_v · h_v / 770 + 1500.
const PIXELS_PER_TOKEN: u64 = 770;
const RESPONSE_TEXT_TOKENS: u64 = 1_500;
/// Characters per token when the timeline text is held to its budget.
const TEXT_CHARS_PER_TOKEN: usize = 4;
/// Close-ups enlarge by whole-number duplication.
const CLOSEUP_SCALE: u32 = 2;
const MS_PER_SECOND: u64 = 1_000;

/// At least one key picture per this interval in the overview.
const FLOOR_INTERVAL_MS: u64 = 10_000;
/// Unchanged stretches at least this long are listed in the timeline.
const STATIC_REPORT_MS: u64 = 1_000;

/// `clip_diff`: blocks of this many pixels a side; changed blocks within
/// this many empty blocks (columns, rows) join one region: wide along a
/// row, so the changed words of one text line are one region, and two
/// blocks between rows (the line gap of a text block).
const BLOCK_PX: usize = 8;
const REGION_GAP_BLOCKS: (usize, usize) = (24, 2);
/// Padding around the changed area in the `clip_diff` crop.
const CROP_PADDING: i32 = 16;
const MAX_DIFF_REGIONS: usize = 20;
const PERCENT: u64 = 100;
const RGBA_BYTES: usize = 4;
const RGB_BYTES: usize = 3;

/// The label band above every picture: glyph unit 3 px on pictures whose
/// short side is at least this long, else 2 px.
const LARGE_LABEL_MIN_PX: u32 = 600;
const LARGE_LABEL_UNIT: u32 = 3;
const SMALL_LABEL_UNIT: u32 = 2;
const GLYPH_ROWS: u32 = 5;
const GLYPH_COLUMNS: u32 = 3;
/// A glyph advances by its 3 columns plus one blank column.
const GLYPH_ADVANCE: u32 = 4;
const LABEL_BACKGROUND: image::Rgba<u8> = image::Rgba([20, 20, 20, 255]);
const LABEL_INK: image::Rgba<u8> = image::Rgba([255, 255, 255, 255]);
const OUTLINE_RED: image::Rgba<u8> = image::Rgba([230, 30, 30, 255]);
const OUTLINE_PX: i64 = 2;
const DIVIDER_GREY: image::Rgba<u8> = image::Rgba([128, 128, 128, 255]);
const DIVIDER_PX: u32 = 8;

/// Runs a `clip_*` operation and builds the tool result: the text answer,
/// the response JSON, then `Image k` label + PNG pairs.
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

/// Refuses any folder that is not a plan 5.0 clip, naming the format it
/// holds, before the typed `clip.json` is read.
fn check_format(folder: &Path) -> Result<(), ErrorData> {
    let path = folder.join(CLIP_JSON_FILE);
    let text = std::fs::read_to_string(&path).map_err(|error| {
        params_invalid(format!(
            "{} is not a readable clip folder: could not read {}: {error}",
            folder.display(),
            path.display()
        ))
    })?;
    let header: serde_json::Value = serde_json::from_str(&text).map_err(|error| {
        params_invalid(format!("{} is not valid JSON: {error}", path.display()))
    })?;
    let version = header.get("schema_version").and_then(serde_json::Value::as_u64);
    let method = header.get("method").and_then(serde_json::Value::as_str);
    if version == Some(u64::from(CLIP_SCHEMA_VERSION)) && method == Some(CLIP_METHOD) {
        return Ok(());
    }
    let format = match (header.get("way").and_then(serde_json::Value::as_str), method) {
        (Some(way), _) => format!(
            "the older {way} format (exact zstd rows under frames/ plus H.264 segments under moving/)"
        ),
        (None, Some(method)) => format!("the format {method}"),
        (None, None) => "an unknown format".to_owned(),
    };
    let version = version.map_or_else(|| "missing".to_owned(), |version| version.to_string());
    Err(params_invalid(format!(
        "{} holds a clip in {format}, schema_version {version}; this viewer reads only the {CLIP_METHOD} format (schema_version {CLIP_SCHEMA_VERSION}: {VIDEO_DIR}/part-NNNN.ivf, {STILLS_DIR}/still-NNNNNNNN.webp, {FRAMES_FILE}); record the clip again",
        folder.display()
    )))
}

/// A clip folder as the viewer reads it: `clip.json` and one index line per
/// instant. It owns the timeline read from the index.
struct ViewedClip {
    folder: PathBuf,
    clip: ClipJson,
    lines: Vec<IndexLine>,
}

impl ViewedClip {
    fn open(folder: &Path) -> Result<Self, ErrorData> {
        check_format(folder)?;
        let clip = ClipJson::read(folder).map_err(|error| {
            params_invalid(format!(
                "{} is not a readable clip folder: {error}",
                folder.display()
            ))
        })?;
        if clip.fps == 0 {
            return Err(params_invalid(format!("{CLIP_JSON_FILE} has fps 0")));
        }
        if clip.state == ClipState::Recording {
            return Err(params_invalid(format!(
                "clip {} is still recording; wait for record_status to report ready",
                clip.clip_id
            )));
        }
        let lines = read_index(folder)
            .map_err(|error| params_invalid(format!("{FRAMES_FILE} is not readable: {error}")))?;
        if lines.is_empty() {
            return Err(params_invalid(format!("{FRAMES_FILE} lists no instants")));
        }
        if let Some(line) = lines.iter().find(|line| marks(line) != 1) {
            return Err(params_invalid(format!(
                "{FRAMES_FILE} line of instant {} must hold exactly one of enc, same and lost; it is not the {CLIP_METHOD} format",
                line.instant
            )));
        }
        Ok(Self {
            folder: folder.to_path_buf(),
            clip,
            lines,
        })
    }

    fn fps(&self) -> u64 {
        u64::from(self.clip.fps)
    }

    fn last_instant(&self) -> u64 {
        self.lines.last().map_or(0, |line| line.instant)
    }

    /// Time of instant k: k / f.
    fn t_of(&self, instant: u64) -> u64 {
        instant * MS_PER_SECOND / self.fps()
    }

    /// The instant on screen at `t_ms`.
    fn instant_at_ms(&self, t_ms: u64) -> u64 {
        (t_ms * self.fps() / MS_PER_SECOND).min(self.last_instant())
    }

    fn duration_ms(&self) -> u64 {
        self.t_of(self.last_instant() + 1)
    }

    fn size(&self) -> (u32, u32) {
        (
            self.clip.geometry.recorded_width,
            self.clip.geometry.recorded_height,
        )
    }

    fn changed_count(&self) -> u64 {
        self.lines.iter().filter(|line| line.enc.is_some()).count() as u64
    }

    fn still_instants(&self) -> impl Iterator<Item = u64> + '_ {
        self.lines
            .iter()
            .filter(|line| line.still == Some(true))
            .map(|line| line.instant)
    }

    fn source(&self, instant: u64) -> Result<PictureSource, ErrorData> {
        PictureSource::resolve(&self.folder, &self.lines, instant).map_err(decode_error)
    }

    fn is_recorded(&self, instant: u64) -> bool {
        self.source(instant)
            .is_ok_and(|source| source.shown_instant().is_some())
    }

    /// The nearest instants before and after `instant` that show a picture.
    fn nearest_recorded(&self, instant: u64) -> (Option<u64>, Option<u64>) {
        let before = (0..instant).rev().find(|k| self.is_recorded(*k));
        let after = (instant + 1..=self.last_instant()).find(|k| self.is_recorded(*k));
        (before, after)
    }

    /// The nearest settle points (instants with an exact still) at or
    /// before and after `instant`.
    fn settle_points_around(&self, instant: u64) -> (Option<u64>, Option<u64>) {
        let before = self.still_instants().filter(|k| *k <= instant).last();
        let after = self.still_instants().find(|k| *k > instant);
        (before, after)
    }

    fn views_dir(&self) -> Result<PathBuf, ErrorData> {
        let views = self.folder.join(VIEWS_DIR);
        std::fs::create_dir_all(&views)
            .map_err(|error| internal(format!("could not create {views:?}: {error}")))?;
        Ok(views)
    }

    /// The long edge of overview and range images: `max_long_edge` when
    /// given, else `VIEW_MAX_LONG_EDGE`. The second value is `true` when it
    /// shrinks the window below `READABLE_MIN_SCALE`.
    fn view_long_edge(&self, requested: Option<u32>) -> (u32, bool) {
        let (width, height) = self.size();
        let long_edge = requested.unwrap_or(VIEW_MAX_LONG_EDGE);
        let readable = (f64::from(width.max(height)) * READABLE_MIN_SCALE).ceil();
        (long_edge, f64::from(long_edge) < readable)
    }

    /// Images per call so that N · w_v · h_v / 770 stays inside `budget`.
    fn images_per_call(&self, long_edge: u32, budget: u64) -> usize {
        let (width, height) = self.size();
        let long = width.max(height).max(1);
        let factor = f64::from(long_edge.min(long)) / f64::from(long);
        let pixels = (f64::from(width) * factor) * (f64::from(height) * factor);
        let per_image = (pixels / PIXELS_PER_TOKEN as f64).max(1.0);
        ((budget as f64 / per_image) as usize).clamp(1, MAX_IMAGES_PER_CALL)
    }

    /// The timeline of the clip, read from the index without decoding:
    /// changed stretches, settle points, unchanged stretches and lost
    /// ranges, in time order.
    fn timeline(&self) -> Vec<ClipTimelineEntry> {
        let mut entries = self.change_entries();
        entries.extend(self.still_instants().map(|instant| {
            self.entry(
                "settled",
                instant,
                instant,
                0,
                format!(
                    "{} settled: exact still of instant {instant}",
                    fmt_time(self.t_of(instant))
                ),
            )
        }));
        let static_instants = STATIC_REPORT_MS * self.fps() / MS_PER_SECOND;
        for (start, end, ()) in runs(&self.lines, |line| (line.same == Some(true)).then_some(())) {
            if end + 1 - start >= static_instants.max(1) {
                let seconds = (self.t_of(end + 1) - self.t_of(start)) as f64 / MS_PER_SECOND as f64;
                let text = format!(
                    "{}-{} no change ({seconds:.1} s)",
                    fmt_time(self.t_of(start)),
                    fmt_time(self.t_of(end + 1))
                );
                entries.push(self.entry("static", start, end, 0, text));
            }
        }
        for (start, end, cause) in runs(&self.lines, |line| line.lost.clone()) {
            let (before, after) = self.nearest_recorded(start);
            let mut text = format!(
                "{}-{} lost: {} instants ({cause}), no picture recorded",
                fmt_time(self.t_of(start)),
                fmt_time(self.t_of(end + 1)),
                end + 1 - start
            );
            let _ = write!(text, "; {}", nearest_text(before, after));
            entries.push(self.entry("lost", start, end, 0, text));
        }
        entries.sort_by_key(|entry| (entry.start_frame, timeline_rank(&entry.kind)));
        entries
    }

    /// Changed instants joined into stretches: two changes fewer than
    /// `SETTLE_INSTANTS` apart belong to one stretch; a lost instant ends it.
    fn change_entries(&self) -> Vec<ClipTimelineEntry> {
        let mut stretches: Vec<(u64, u64, u64)> = Vec::new();
        let mut open: Option<(u64, u64, u64)> = None;
        for line in &self.lines {
            if line.lost.is_some() {
                stretches.extend(open.take());
            } else if line.enc.is_some() {
                open = match open {
                    Some((start, last, count)) if line.instant - last < SETTLE_INSTANTS => {
                        Some((start, line.instant, count + 1))
                    }
                    other => {
                        stretches.extend(other);
                        Some((line.instant, line.instant, 1))
                    }
                };
            }
        }
        stretches.extend(open);
        stretches
            .into_iter()
            .map(|(start, end, count)| {
                let text = if count == 1 {
                    format!(
                        "{} changed: 1 new picture (video) at instant {start}",
                        fmt_time(self.t_of(start))
                    )
                } else {
                    format!(
                        "{}-{} changing: {count} new pictures (video), instants {start}-{end}",
                        fmt_time(self.t_of(start)),
                        fmt_time(self.t_of(end + 1))
                    )
                };
                self.entry("change", start, end, count, text)
            })
            .collect()
    }

    fn entry(&self, kind: &str, start: u64, end: u64, count: u64, text: String) -> ClipTimelineEntry {
        ClipTimelineEntry {
            kind: kind.to_owned(),
            start_ms: self.t_of(start),
            end_ms: self.t_of(end + 1),
            start_frame: start,
            end_frame: end,
            changed_blocks: count,
            bbox: None,
            text,
        }
    }

    /// Key pictures of the overview, in time order: the first and last
    /// recorded instants, every settle point, the end of every changed
    /// stretch, and one picture per `FLOOR_INTERVAL_MS` where none falls.
    /// Picks that show the same recorded picture are joined; lost instants
    /// are never picked.
    fn overview_picks(&self) -> Vec<Pick> {
        let mut picks: Vec<Pick> = Vec::new();
        let first = (0..=self.last_instant()).find(|k| self.is_recorded(*k));
        let last = (0..=self.last_instant()).rev().find(|k| self.is_recorded(*k));
        picks.extend(first.map(|instant| Pick::new(instant, "first")));
        picks.extend(self.still_instants().map(|instant| Pick::new(instant, "settled")));
        picks.extend(
            self.change_entries()
                .iter()
                .map(|entry| Pick::new(entry.end_frame, "end of a change")),
        );
        picks.extend(last.map(|instant| Pick::new(instant, "last")));
        picks.sort_by_key(|pick| pick.instant);
        let mut floored: Vec<Pick> = Vec::with_capacity(picks.len());
        for next in picks {
            while let Some(previous) = floored.last() {
                let floor = self.instant_at_ms(self.t_of(previous.instant) + FLOOR_INTERVAL_MS);
                if self.t_of(next.instant) <= self.t_of(previous.instant) + FLOOR_INTERVAL_MS
                    || floor <= previous.instant
                {
                    break;
                }
                floored.push(Pick::new(floor, "10 s floor"));
            }
            floored.push(next);
        }
        let mut joined: Vec<(u64, Pick)> = Vec::with_capacity(floored.len());
        for pick in floored {
            let Some(shown) = self
                .source(pick.instant)
                .ok()
                .and_then(|source| source.shown_instant())
            else {
                continue;
            };
            match joined.last_mut() {
                Some((last_shown, last)) if *last_shown == shown => last.join(&pick.reason),
                _ => joined.push((shown, pick)),
            }
        }
        joined.into_iter().map(|(_, pick)| pick).collect()
    }
}

/// How many of `enc`, `same` and `lost` an index line holds.
fn marks(line: &IndexLine) -> usize {
    [
        line.enc.is_some(),
        line.same == Some(true),
        line.lost.is_some(),
    ]
    .into_iter()
    .filter(|mark| *mark)
    .count()
}

/// Runs of consecutive lines with the same `Some` key: (first, last, key).
fn runs<T: PartialEq>(lines: &[IndexLine], key: impl Fn(&IndexLine) -> Option<T>) -> Vec<(u64, u64, T)> {
    let mut runs: Vec<(u64, u64, T)> = Vec::new();
    for line in lines {
        let Some(value) = key(line) else {
            continue;
        };
        match runs.last_mut() {
            Some((_, end, last)) if *end + 1 == line.instant && *last == value => *end = line.instant,
            _ => runs.push((line.instant, line.instant, value)),
        }
    }
    runs
}

/// Order of timeline entries that start at the same instant.
fn timeline_rank(kind: &str) -> u8 {
    match kind {
        "lost" => 0,
        "change" => 1,
        "settled" => 2,
        _ => 3,
    }
}

fn nearest_text(before: Option<u64>, after: Option<u64>) -> String {
    let side = |instant: Option<u64>| instant.map_or_else(|| "none".to_owned(), |k| format!("instant {k}"));
    format!(
        "nearest recorded: {} before, {} after",
        side(before),
        side(after)
    )
}

/// One key picture of the overview and why it was chosen.
struct Pick {
    instant: u64,
    reason: String,
}

impl Pick {
    fn new(instant: u64, reason: &str) -> Self {
        Self {
            instant,
            reason: reason.to_owned(),
        }
    }

    fn join(&mut self, reason: &str) {
        if !self.reason.contains(reason) {
            self.reason = format!("{} + {reason}", self.reason);
        }
    }
}

// ---------------------------------------------------------------------------
// The four operations
// ---------------------------------------------------------------------------

/// One viewer call: the clip, its decoder and the answer built so far.
struct ClipView {
    clip: ViewedClip,
    decoder: ClipDecoder,
    images: Vec<ClipViewImage>,
    pngs: Vec<Vec<u8>>,
    timeline: Vec<ClipTimelineEntry>,
    lost: Vec<ClipLostInstant>,
    regions: Vec<ClipChangedRegion>,
    paging: Option<(u32, u32)>,
    notes: Vec<String>,
    text: String,
}

impl ClipView {
    fn new(clip: ViewedClip) -> Self {
        let decoder = ClipDecoder::new(&clip.folder, clip.lines.clone());
        let mut notes = Vec::new();
        if clip.clip.state == ClipState::Failed {
            notes.push(format!(
                "the recording ended early ({}); instants after the last complete video part are named lost",
                clip.clip.failure.as_deref().unwrap_or("no cause recorded")
            ));
        }
        Self {
            clip,
            decoder,
            images: Vec::new(),
            pngs: Vec::new(),
            timeline: Vec::new(),
            lost: Vec::new(),
            regions: Vec::new(),
            paging: None,
            notes,
            text: String::new(),
        }
    }

    /// The timeline on page 1 (held to the text budget) and key pictures,
    /// as many per page as the image budget allows. Page p holds every
    /// `pages`-th pick from pick p, so each page spreads over the whole clip.
    fn overview(&mut self, params: &ClipViewParams) -> Result<(), ErrorData> {
        let picks = self.clip.overview_picks();
        let (long_edge, unreadable) = self.clip.view_long_edge(params.max_long_edge);
        let per_page = self
            .clip
            .images_per_call(long_edge, OVERVIEW_IMAGE_TOKEN_BUDGET);
        let pages = u32::try_from(picks.len().div_ceil(per_page))
            .unwrap_or(u32::MAX)
            .max(1);
        let page = params.page.unwrap_or(1);
        if page == 0 || page > pages {
            return Err(params_invalid(format!(
                "page {page} is outside 1..={pages} for this clip"
            )));
        }
        let stills = self.clip.still_instants().count();
        let lost = self.clip.lines.iter().filter(|line| line.lost.is_some()).count();
        let _ = writeln!(
            self.text,
            "Clip {}: {} instants over {}, {} changed (video), {stills} settle points (exact stills), {lost} lost; {} key pictures, page {page}/{pages} (every page spreads over the whole clip):",
            self.clip.clip.clip_id,
            self.clip.lines.len(),
            fmt_time(self.clip.duration_ms()),
            self.clip.changed_count(),
            picks.len()
        );
        if page == 1 {
            self.write_timeline();
        }
        if unreadable {
            self.notes.push(format!(
                "images are shrunk to a {long_edge} px long edge, below {}% of the window size; small text may be unreadable, use clip_frame (recorded pixels, region crop, close-up) to read it",
                (READABLE_MIN_SCALE * PERCENT as f64) as u64
            ));
        }
        let stride = pages as usize;
        let chosen: Vec<(usize, &Pick)> = picks
            .iter()
            .enumerate()
            .skip(page as usize - 1)
            .step_by(stride)
            .collect();
        for (position, pick) in chosen {
            let index = u32::try_from(position + 1).unwrap_or(u32::MAX);
            let mut picture = self.picture(pick.instant)?;
            let Some(image) = picture.image.take() else {
                continue;
            };
            let stem = format!("overview_{index:02}_f{}", pick.instant);
            self.export(fit(image, long_edge), index, &picture, &pick.reason, &stem)?;
        }
        self.paging = Some((page, pages));
        Ok(())
    }

    fn write_timeline(&mut self) {
        let budget_chars =
            usize::try_from(OVERVIEW_TEXT_TOKEN_BUDGET).unwrap_or(usize::MAX) * TEXT_CHARS_PER_TOKEN;
        let mut kept = Vec::new();
        for entry in self.clip.timeline() {
            if self.text.len() + entry.text.len() + 3 > budget_chars {
                self.notes.push(format!(
                    "the timeline was cut at {} to stay inside its token budget; call clip_frames or clip_frame for later times",
                    fmt_time(entry.start_ms)
                ));
                break;
            }
            let _ = writeln!(self.text, "- {}", entry.text);
            kept.push(entry);
        }
        self.timeline = kept;
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
        let min_step = MS_PER_SECOND.div_ceil(u64::from(fps));
        let fit_step = (end - start).div_ceil(max_images.saturating_sub(1).max(1) as u64);
        let step = min_step.max(fit_step).max(1);
        if step > min_step {
            self.notes.push(format!(
                "sampled every {step} ms so the range fits {max_images} images; narrow the range for more detail"
            ));
        }
        let _ = writeln!(
            self.text,
            "Clip {} pictures {}-{} every {step} ms:",
            self.clip.clip.clip_id,
            fmt_time(start),
            fmt_time(end)
        );
        let mut previous: Option<(u64, u32)> = None;
        let mut last_instant: Option<u64> = None;
        let mut t_ms = start;
        while t_ms <= end && self.images.len() < max_images {
            let instant = self.clip.instant_at_ms(t_ms);
            t_ms += step;
            if last_instant == Some(instant) {
                continue;
            }
            last_instant = Some(instant);
            let source = self.clip.source(instant)?;
            let Some(shown) = source.shown_instant() else {
                if let PictureSource::Lost { cause } = &source {
                    self.name_lost(instant, cause);
                }
                continue;
            };
            if let Some((previous_shown, index)) = previous
                && previous_shown == shown
            {
                let _ = writeln!(
                    self.text,
                    "- {} frame {instant}: unchanged, the same recorded picture as Image {index}",
                    fmt_time(self.clip.t_of(instant))
                );
                continue;
            }
            let index = u32::try_from(self.images.len() + 1).unwrap_or(u32::MAX);
            let mut picture = self.picture(instant)?;
            let Some(image) = picture.image.take() else {
                continue;
            };
            let stem = format!("frames_{start}-{end}_{index:02}_f{instant}");
            self.export(fit(image, long_edge), index, &picture, "range sample", &stem)?;
            previous = Some((shown, index));
        }
        Ok(())
    }

    /// One picture at recorded pixels; a close-up duplicates every pixel
    /// `CLOSEUP_SCALE` times, never blurs.
    fn frame(&mut self, params: &ClipViewParams) -> Result<(), ErrorData> {
        let t_ms = params.t_ms.unwrap_or(0);
        self.check_inside(t_ms)?;
        let instant = self.clip.instant_at_ms(t_ms);
        let mut picture = self.picture(instant)?;
        let Some(mut image) = picture.image.take() else {
            if let PictureSource::Lost { cause } = &picture.source {
                let cause = cause.clone();
                self.name_lost(instant, &cause);
            }
            return Ok(());
        };
        let mut stem = format!("frame_f{instant}");
        if let Some(region) = params.region {
            let crop = clamp_rect(region, image.width(), image.height())?;
            let short = crop.w.min(crop.h);
            let possible = MIN_CROP_SHORT_SIDE.min(image.width()).min(image.height());
            if u32::try_from(short).unwrap_or(0) < possible {
                return Err(params_invalid(format!(
                    "the crop inside the {}x{} picture is {}x{}; its short side must be at least {MIN_CROP_SHORT_SIDE} px",
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
        // The call stays inside its token budget (tokens = w · h / 770 + 1500).
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
            "Clip {} frame {instant} at {} ({}x{} px, close-up x{factor}):",
            self.clip.clip.clip_id,
            fmt_time(picture.t_ms),
            width,
            height
        );
        let reason = if params.region.is_some() {
            "crop"
        } else {
            "recorded pixels"
        };
        self.export(image, 1, &picture, reason, &stem)
    }

    /// Changed regions between the pictures at `start_ms` and `end_ms`;
    /// only two exact stills are compared.
    fn diff(&mut self, params: &ClipViewParams) -> Result<(), ErrorData> {
        let (start, end) = self.range(params)?;
        let (from, to) = (self.clip.instant_at_ms(start), self.clip.instant_at_ms(end));
        let _ = writeln!(
            self.text,
            "Clip {} diff {} (frame {from}) -> {} (frame {to}):",
            self.clip.clip.clip_id,
            fmt_time(self.clip.t_of(from)),
            fmt_time(self.clip.t_of(to))
        );
        let sources = [(from, self.clip.source(from)?), (to, self.clip.source(to)?)];
        let mut any_lost = false;
        for (instant, source) in &sources {
            if let PictureSource::Lost { cause } = source {
                self.name_lost(*instant, cause);
                any_lost = true;
            }
        }
        if any_lost {
            let _ = writeln!(self.text, "- no comparison: a lost instant holds no picture");
            return Ok(());
        }
        let [(_, before_source), (_, after_source)] = &sources;
        if before_source.shown_instant() == after_source.shown_instant() {
            let _ = writeln!(
                self.text,
                "- the same recorded picture at both times: no change"
            );
            return Ok(());
        }
        if before_source.kind() != PictureKind::ExactStill
            || after_source.kind() != PictureKind::ExactStill
        {
            self.refuse_video_diff(&sources);
            return Ok(());
        }
        let mut before = self.picture(from)?;
        let mut after = self.picture(to)?;
        let (Some(before_image), Some(after_image)) = (before.image.take(), after.image.take())
        else {
            return Err(decode_error("a still decoded to no picture".to_owned()));
        };
        if before_image.dimensions() != after_image.dimensions() {
            let _ = writeln!(
                self.text,
                "- the window size changed between the two times ({}x{} -> {}x{}); compare them with clip_frame",
                before_image.width(),
                before_image.height(),
                after_image.width(),
                after_image.height()
            );
            return Ok(());
        }
        let blocks = ChangedBlocks::between(&before_image, &after_image);
        let mut regions = blocks.regions();
        let _ = writeln!(
            self.text,
            "- exact still to exact still: {} changed {BLOCK_PX}x{BLOCK_PX} blocks ({}% of the picture) in {} regions",
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
            let _ = writeln!(self.text, "- no pixel differs");
            return Ok(());
        };
        let crop = diff_crop(bbox, before_image.width(), before_image.height());
        let mut left = crop_image(&before_image, crop);
        let mut right = crop_image(&after_image, crop);
        for region in &regions {
            outline(&mut left, region.rect, crop);
            outline(&mut right, region.rect, crop);
        }
        let image = fit(side_by_side(&left, &right), VIEW_MAX_LONG_EDGE);
        let reason = format!(
            "before {} / after {}, crop x={} y={} w={} h={}",
            fmt_time(before.t_ms),
            fmt_time(after.t_ms),
            crop.x,
            crop.y,
            crop.w,
            crop.h
        );
        self.regions = regions;
        self.export(image, 1, &after, &reason, &format!("diff_f{from}-f{to}"))
    }

    /// Names why a video picture is not compared, and the settle points an
    /// agent can compare instead.
    fn refuse_video_diff(&mut self, sources: &[(u64, PictureSource); 2]) {
        let _ = writeln!(
            self.text,
            "- clip_diff compares exact stills only: change analysis on video pictures is not measured yet (plan 5.0, open item O2)"
        );
        for (instant, source) in sources {
            let (before, after) = self.clip.settle_points_around(*instant);
            let point = |settle: Option<u64>| {
                settle.map_or_else(
                    || "none".to_owned(),
                    |k| format!("instant {k} ({})", fmt_time(self.clip.t_of(k))),
                )
            };
            let _ = writeln!(
                self.text,
                "- {} (frame {instant}) shows {}; nearest settle points: {} before, {} after",
                fmt_time(self.clip.t_of(*instant)),
                source.kind().as_str(),
                point(before),
                point(after)
            );
        }
        let _ = writeln!(
            self.text,
            "- call clip_diff with the times of two settle points, or clip_frames to see the video pictures"
        );
    }

    fn picture(&mut self, instant: u64) -> Result<ClipPicture, ErrorData> {
        self.decoder.picture_at(instant).map_err(decode_error)
    }

    /// Answers a lost instant: its cause and the nearest recorded instants;
    /// no picture.
    fn name_lost(&mut self, instant: u64, cause: &str) {
        let (before, after) = self.clip.nearest_recorded(instant);
        let t_ms = self.clip.t_of(instant);
        let _ = writeln!(
            self.text,
            "- {} frame {instant}: lost ({cause}); no picture was recorded; {}",
            fmt_time(t_ms),
            nearest_text(before, after)
        );
        self.lost.push(ClipLostInstant {
            kind: PictureKind::Lost.as_str().to_owned(),
            frame: instant,
            t_ms,
            cause: cause.to_owned(),
            recorded_before: before,
            recorded_after: after,
        });
    }

    /// A time inside the clip.
    fn check_inside(&self, t_ms: u64) -> Result<(), ErrorData> {
        if t_ms < self.clip.duration_ms() {
            return Ok(());
        }
        Err(params_invalid(format!(
            "t_ms {t_ms} is at or after the clip end ({} ms)",
            self.clip.duration_ms()
        )))
    }

    /// `start_ms`..=`end_ms`, the end clamped to the clip's last instant.
    fn range(&self, params: &ClipViewParams) -> Result<(u64, u64), ErrorData> {
        let (start, end) = (params.start_ms.unwrap_or(0), params.end_ms.unwrap_or(0));
        if start >= end {
            return Err(params_invalid(format!(
                "start_ms {start} must be before end_ms {end}"
            )));
        }
        self.check_inside(start)?;
        Ok((start, end.min(self.clip.duration_ms() - 1)))
    }

    /// Labels the picture (text and a band above it), writes the PNG under
    /// `views/` and keeps it for the result.
    fn export(
        &mut self,
        image: RgbaImage,
        index: u32,
        picture: &ClipPicture,
        reason: &str,
        stem: &str,
    ) -> Result<(), ErrorData> {
        let kind = picture.kind();
        let shown = picture.source.shown_instant().unwrap_or(picture.instant);
        let unchanged = if shown == picture.instant {
            String::new()
        } else {
            format!(", unchanged since frame {shown}")
        };
        let label = format!(
            "Image {index} ({}, t={}, frame {}{unchanged}, {reason})",
            kind.as_str(),
            fmt_time(picture.t_ms),
            picture.instant
        );
        let image = labelled(
            &image,
            &format!(
                "#{index} {} F{} {}",
                fmt_time(picture.t_ms),
                picture.instant,
                label_word(kind)
            ),
        );
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
            t_ms: picture.t_ms,
            frame: picture.instant,
            kind: kind.as_str().to_owned(),
            shown_frame: shown,
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
            let _ = writeln!(self.text, "More key pictures: call again with page={next}.");
        }
        let (width, height) = self.clip.size();
        ViewOutput {
            response: ClipViewResponse {
                clip_id: self.clip.clip.clip_id.clone(),
                folder: self.clip.folder.to_string_lossy().into_owned(),
                duration_ms: self.clip.duration_ms(),
                width,
                height,
                fps: self.clip.clip.fps,
                frames_total: self.clip.lines.len() as u64,
                frames_new: self.clip.changed_count(),
                timeline: self.timeline,
                images: self.images,
                lost: self.lost,
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
// Change between two exact stills
// ---------------------------------------------------------------------------

/// The `BLOCK_PX` blocks in which any pixel differs between two pictures of
/// the same size. Exact stills hold the window's exact pixels, so any
/// difference is a real change.
struct ChangedBlocks {
    width: u32,
    height: u32,
    columns: usize,
    rows: usize,
    changed: Vec<bool>,
    count: u64,
}

impl ChangedBlocks {
    fn between(before: &RgbaImage, after: &RgbaImage) -> Self {
        let (width, height) = before.dimensions();
        let columns = (width as usize).div_ceil(BLOCK_PX);
        let rows = (height as usize).div_ceil(BLOCK_PX);
        let mut changed = vec![false; columns * rows];
        let row_bytes = width as usize * RGBA_BYTES;
        if row_bytes > 0 {
            let row_pairs = before
                .as_raw()
                .chunks_exact(row_bytes)
                .zip(after.as_raw().chunks_exact(row_bytes));
            for (y, (above, below)) in row_pairs.enumerate() {
                let pixels = above
                    .chunks_exact(RGBA_BYTES)
                    .zip(below.chunks_exact(RGBA_BYTES));
                for (x, (a, b)) in pixels.enumerate() {
                    if a[..RGB_BYTES] != b[..RGB_BYTES]
                        && let Some(block) = changed.get_mut((y / BLOCK_PX) * columns + x / BLOCK_PX)
                    {
                        *block = true;
                    }
                }
            }
        }
        let count = changed.iter().filter(|block| **block).count() as u64;
        Self {
            width,
            height,
            columns,
            rows,
            changed,
            count,
        }
    }

    fn percent(&self) -> u64 {
        self.count * PERCENT / (self.changed.len() as u64).max(1)
    }

    /// Changed blocks joined across at most `REGION_GAP_BLOCKS` empty
    /// blocks, largest region first.
    fn regions(&self) -> Vec<ClipChangedRegion> {
        let (gap_columns, gap_rows) = REGION_GAP_BLOCKS;
        let mut seen = vec![false; self.changed.len()];
        let mut regions = Vec::new();
        for start in 0..self.changed.len() {
            if !self.changed[start] || seen[start] {
                continue;
            }
            seen[start] = true;
            let mut stack = vec![start];
            let (mut first, mut last) = (self.cell(start), self.cell(start));
            let mut count = 0_u64;
            while let Some(block) = stack.pop() {
                count += 1;
                let (column, row) = self.cell(block);
                first = (first.0.min(column), first.1.min(row));
                last = (last.0.max(column), last.1.max(row));
                let near_rows = row.saturating_sub(gap_rows)..=(row + gap_rows).min(self.rows - 1);
                for near_row in near_rows {
                    let near_columns =
                        column.saturating_sub(gap_columns)..=(column + gap_columns).min(self.columns - 1);
                    for near_column in near_columns {
                        let near = near_row * self.columns + near_column;
                        if self.changed[near] && !seen[near] {
                            seen[near] = true;
                            stack.push(near);
                        }
                    }
                }
            }
            regions.push(ClipChangedRegion {
                rect: self.pixels(first, last),
                changed_blocks: count,
            });
        }
        regions.sort_by(|a, b| b.changed_blocks.cmp(&a.changed_blocks));
        regions
    }

    /// The box around every changed block, in pixels.
    fn bbox(&self) -> Option<Rect> {
        let mut cells = (0..self.changed.len())
            .filter(|block| self.changed[*block])
            .map(|block| self.cell(block));
        let first = cells.next()?;
        let (low, high) = cells.fold((first, first), |(low, high), (column, row)| {
            (
                (low.0.min(column), low.1.min(row)),
                (high.0.max(column), high.1.max(row)),
            )
        });
        Some(self.pixels(low, high))
    }

    /// Column and row of block `block`.
    fn cell(&self, block: usize) -> (usize, usize) {
        (block % self.columns, block / self.columns)
    }

    /// The pixels of the blocks from `first` to `last` (column, row), inclusive.
    fn pixels(&self, first: (usize, usize), last: (usize, usize)) -> Rect {
        let x = first.0 * BLOCK_PX;
        let y = first.1 * BLOCK_PX;
        let right = ((last.0 + 1) * BLOCK_PX).min(self.width as usize);
        let bottom = ((last.1 + 1) * BLOCK_PX).min(self.height as usize);
        rect(x, y, right.saturating_sub(x), bottom.saturating_sub(y))
    }
}

// ---------------------------------------------------------------------------
// Image helpers
// ---------------------------------------------------------------------------

/// Shrinks (never enlarges) so the long edge fits `long_edge`; every output
/// pixel is the average of all source pixels it covers.
fn fit(image: RgbaImage, long_edge: u32) -> RgbaImage {
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
    RgbaImage::from_fn(width, height, |x, y| {
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
            "region x={} y={} w={} h={} lies outside the {width}x{height} picture",
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

fn crop_image(image: &RgbaImage, crop: Rect) -> RgbaImage {
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

/// Draws an `OUTLINE_PX` red outline of `region` (clip pixels) on a crop at `crop`.
fn outline(image: &mut RgbaImage, region: Rect, crop: Rect) {
    let x0 = i64::from(region.x - crop.x);
    let y0 = i64::from(region.y - crop.y);
    let x1 = x0 + i64::from(region.w) - 1;
    let y1 = y0 + i64::from(region.h) - 1;
    let (width, height) = (i64::from(image.width()), i64::from(image.height()));
    let mut put = |x: i64, y: i64| {
        if let (Ok(x), Ok(y)) = (u32::try_from(x), u32::try_from(y))
            && i64::from(x) < width
            && i64::from(y) < height
        {
            image.put_pixel(x, y, OUTLINE_RED);
        }
    };
    for t in 0..OUTLINE_PX {
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

/// `left` and `right` next to each other with a grey divider.
fn side_by_side(left: &RgbaImage, right: &RgbaImage) -> RgbaImage {
    let width = left.width() + DIVIDER_PX + right.width();
    let height = left.height().max(right.height());
    let mut out = RgbaImage::from_pixel(width, height, DIVIDER_GREY);
    image::imageops::replace(&mut out, left, 0, 0);
    image::imageops::replace(&mut out, right, i64::from(left.width() + DIVIDER_PX), 0);
    out
}

/// The word drawn in the label band for each kind of picture.
const fn label_word(kind: PictureKind) -> &'static str {
    match kind {
        PictureKind::ExactStill => "STILL",
        PictureKind::Video => "VIDEO",
        PictureKind::Lost => "LOST",
    }
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
        'S' => [3, 4, 2, 1, 6],
        'T' => [7, 2, 2, 2, 2],
        'I' => [7, 2, 2, 2, 7],
        'L' => [4, 4, 4, 4, 7],
        'V' => [5, 5, 5, 5, 2],
        'D' => [6, 5, 5, 5, 6],
        'E' => [7, 4, 6, 4, 7],
        'O' => [2, 5, 5, 5, 2],
        _ => [0; 5],
    }
}

/// `image` below a dark band that carries `text` in white, so the label
/// never covers recorded pixels.
fn labelled(image: &RgbaImage, text: &str) -> RgbaImage {
    let unit = if image.width().min(image.height()) >= LARGE_LABEL_MIN_PX {
        LARGE_LABEL_UNIT
    } else {
        SMALL_LABEL_UNIT
    };
    let pad = unit * 2;
    let band = pad * 2 + unit * GLYPH_ROWS;
    let width = image.width();
    let mut out = RgbaImage::from_pixel(width, image.height() + band, LABEL_BACKGROUND);
    image::imageops::replace(&mut out, image, 0, i64::from(band));
    for (position, c) in (0_u32..).zip(text.chars()) {
        let left = pad + position * unit * GLYPH_ADVANCE;
        for (row, bits) in (0_u32..).zip(glyph(c)) {
            for column in 0..GLYPH_COLUMNS {
                if bits & (4 >> column) == 0 {
                    continue;
                }
                for dy in 0..unit {
                    for dx in 0..unit {
                        let (x, y) = (left + column * unit + dx, pad + row * unit + dy);
                        if x < width && y < band {
                            out.put_pixel(x, y, LABEL_INK);
                        }
                    }
                }
            }
        }
    }
    out
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
        "clip folder {folder} ({VIDEO_DIR}/, {STILLS_DIR}/, {FRAMES_FILE}, {CLIP_JSON_FILE}); exported images under {VIEWS_DIR}"
    )
}

fn params_invalid(message: String) -> ErrorData {
    mcp_error(error_codes::TOOL_PARAMS_INVALID, message)
}

fn internal(message: String) -> ErrorData {
    mcp_error(error_codes::TOOL_INTERNAL_ERROR, message)
}

fn decode_error(message: String) -> ErrorData {
    internal(format!("clip decode failed: {message}"))
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
        "pass a ready clip folder written by screenshot operation=record and times inside it (record_status gives its length)"
    } else {
        "check that the video parts and stills are complete (record_status lists their size and SHA-256); record the clip again if one is damaged"
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
