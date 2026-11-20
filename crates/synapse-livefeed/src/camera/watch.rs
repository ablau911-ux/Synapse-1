//! `watch`: a span of time in ONE picture. Samples are change-gated (a cell is added only when
//! a new frame was stored since the previous cell) at a fixed cadence, then tiled row-major
//! into a grid with thin separators and no text, so spatial detail stays readable.

use std::time::{Duration, Instant};

use image::{
    DynamicImage, Rgba, RgbaImage,
    imageops::{FilterType, replace, resize},
};
use synapse_core::Rect;

use super::{Camera, FrameMeta, ScreenState};
use crate::{
    camera::lifecycle::{CameraState, lock_state, wait_while},
    config::{CameraConfig, ONE_U64, ONE_USIZE, WATCH_SEPARATOR_PX, ZERO_U32, ZERO_U64},
    error::{LivefeedError, Result},
    history::{
        RawBgraFrame, current_unix_ms, downscaled_rgba, encode_dynamic_jpeg, scaled_dimensions,
    },
    tiles::{crop_bgra_to_rgba, translate_to_frame_rect, validate_rect_in_frame},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WatchFrame {
    pub index: usize,
    pub seq: u64,
    pub ts_ms: u64,
}

#[derive(Clone, Debug)]
pub struct Watch {
    pub image_jpeg: Vec<u8>,
    pub cols: usize,
    pub rows: usize,
    pub cell_w: u32,
    pub cell_h: u32,
    pub frames: Vec<WatchFrame>,
    pub ms_elapsed: u64,
    pub monitor: u32,
    pub origin_x: i32,
    pub origin_y: i32,
    /// The filmed area in virtual-screen px; `None` means the whole monitor.
    pub region: Option<Rect>,
    pub screen_state: ScreenState,
}

struct Sample {
    image: RgbaImage,
    seq: u64,
    ts_ms: u64,
}

struct Plan {
    budget: Duration,
    cadence: Duration,
    cap: usize,
    cell_long_edge: u32,
    quality: u8,
    /// Filmed area in frame px.
    frame_region: Option<Rect>,
}

/// Pixels copied out of the camera lock for one cell.
enum CellSource {
    Frame {
        bgra: Vec<u8>,
        width: u32,
        height: u32,
    },
    Crop {
        rgba: Vec<u8>,
        width: u32,
        height: u32,
    },
}

impl Plan {
    fn from_config(cfg: &CameraConfig, ms: Option<u64>, max_frames: Option<usize>) -> Self {
        let budget_ms = ms
            .unwrap_or(cfg.watch_default_ms)
            .min(cfg.wait_max_ms)
            .max(ONE_U64);
        let cap = max_frames
            .unwrap_or(cfg.watch_max_frames)
            .min(cfg.watch_max_frames)
            .max(ONE_USIZE);
        let cadence_ms = (budget_ms / u64::try_from(cap).unwrap_or(ONE_U64)).max(ONE_U64);
        Self {
            budget: Duration::from_millis(budget_ms),
            cadence: Duration::from_millis(cadence_ms),
            cap,
            cell_long_edge: cfg.watch_cell_long_edge,
            quality: cfg.jpeg_quality,
            frame_region: None,
        }
    }
}

pub(super) fn watch(
    camera: &Camera,
    ms: Option<u64>,
    max_frames: Option<usize>,
    region: Option<Rect>,
) -> Result<Watch> {
    let started = Instant::now();
    let (plan, meta, generation) = {
        let mut s = lock_state(&camera.shared);
        if !s.running {
            return Err(LivefeedError::NotRunning);
        }
        s.history.evict(current_unix_ms());
        if s.screen_state == ScreenState::CaptureLost {
            return Err(LivefeedError::capture_failed("capture lost"));
        }
        if s.screen_state != ScreenState::Live {
            return Ok(non_live_watch(&s, region));
        }
        let mut plan = Plan::from_config(&s.cfg, ms, max_frames);
        plan.frame_region = frame_region(region, &s)?;
        (plan, FrameMeta::from_state(&s), s.generation)
    };
    let deadline = started + plan.budget;
    let mut samples: Vec<Sample> = Vec::with_capacity(plan.cap);
    loop {
        take_sample(camera, generation, &plan, &mut samples)?;
        let now = Instant::now();
        if samples.len() >= plan.cap || now >= deadline {
            break;
        }
        sleep_until(camera, generation, (now + plan.cadence).min(deadline))?;
    }
    compose(samples, &plan, meta, region, started.elapsed())
}

/// Validates a virtual-screen region against the current frame and converts it to frame px.
fn frame_region(region: Option<Rect>, s: &CameraState) -> Result<Option<Rect>> {
    let Some(rect) = region else {
        return Ok(None);
    };
    let frame_rect = translate_to_frame_rect(rect, s.origin_x, s.origin_y)?;
    validate_rect_in_frame(frame_rect, s.width, s.height)?;
    Ok(Some(frame_rect))
}

/// Adds a cell when a new frame was stored since the last cell (the first cell is always taken).
fn take_sample(
    camera: &Camera,
    generation: u64,
    plan: &Plan,
    samples: &mut Vec<Sample>,
) -> Result<()> {
    let (source, seq, ts_ms) = {
        let s = lock_state(&camera.shared);
        if !s.running || s.generation != generation {
            return Err(LivefeedError::NotRunning);
        }
        if s.screen_state == ScreenState::CaptureLost {
            return Err(LivefeedError::capture_failed("capture lost"));
        }
        let advanced = samples.last().is_none_or(|last| s.latest_seq > last.seq);
        if s.screen_state != ScreenState::Live || !advanced {
            return Ok(());
        }
        let Some(raw) = s.latest_raw.as_ref() else {
            return Ok(());
        };
        (
            copy_source(raw, plan.frame_region)?,
            s.latest_seq,
            s.latest_ts_ms,
        )
    };
    let image = cell_image(source, plan.cell_long_edge)?;
    samples.push(Sample { image, seq, ts_ms });
    Ok(())
}

/// Copies only what the cell needs while the lock is held: the region crop, or the frame.
fn copy_source(raw: &RawBgraFrame, region: Option<Rect>) -> Result<CellSource> {
    match region {
        Some(rect) => {
            let (width, height, rgba) =
                crop_bgra_to_rgba(&raw.bgra, raw.width, raw.height, raw.row_stride, rect)?;
            Ok(CellSource::Crop {
                rgba,
                width,
                height,
            })
        }
        None => Ok(CellSource::Frame {
            bgra: raw.bgra.clone(),
            width: raw.width,
            height: raw.height,
        }),
    }
}

/// Shrinks to the cell long edge; a region smaller than the cell keeps its native pixels.
fn cell_image(source: CellSource, long_edge: u32) -> Result<RgbaImage> {
    match source {
        CellSource::Frame {
            bgra,
            width,
            height,
        } => downscaled_rgba(&bgra, width, height, long_edge),
        CellSource::Crop {
            rgba,
            width,
            height,
        } => {
            let image = RgbaImage::from_raw(width, height, rgba)
                .ok_or_else(|| LivefeedError::capture_failed("invalid watch crop buffer"))?;
            let (out_w, out_h, _) = scaled_dimensions(width, height, long_edge);
            if out_w == width && out_h == height {
                return Ok(image);
            }
            Ok(resize(&image, out_w, out_h, FilterType::Triangle))
        }
    }
}

/// Waits on the camera condvar until `tick`; stored frames may wake it early, so it re-waits.
fn sleep_until(camera: &Camera, generation: u64, tick: Instant) -> Result<()> {
    let s = lock_state(&camera.shared);
    let wait_ms = u64::try_from(tick.saturating_duration_since(Instant::now()).as_millis())
        .unwrap_or(u64::MAX);
    let s = wait_while(&camera.shared, s, wait_ms, |st| {
        st.running && st.generation == generation
    });
    if !s.running || s.generation != generation {
        return Err(LivefeedError::NotRunning);
    }
    Ok(())
}

fn compose(
    samples: Vec<Sample>,
    plan: &Plan,
    meta: FrameMeta,
    region: Option<Rect>,
    elapsed: Duration,
) -> Result<Watch> {
    let (cols, rows) = grid_layout(samples.len());
    let (cell_w, cell_h) = samples.first().map_or((ZERO_U32, ZERO_U32), |s| {
        (s.image.width(), s.image.height())
    });
    let canvas_w = span(cols, cell_w).max(1);
    let canvas_h = span(rows, cell_h).max(1);
    let mut canvas = RgbaImage::from_pixel(canvas_w, canvas_h, Rgba([0, 0, 0, u8::MAX]));
    let mut frames = Vec::with_capacity(samples.len());
    for (index, sample) in samples.into_iter().enumerate() {
        let x = i64::from(span(index % cols, cell_w)) + i64::from(sep(index % cols));
        let y = i64::from(span(index / cols, cell_h)) + i64::from(sep(index / cols));
        replace(&mut canvas, &sample.image, x, y);
        frames.push(WatchFrame {
            index,
            seq: sample.seq,
            ts_ms: sample.ts_ms,
        });
    }
    let image_jpeg = encode_dynamic_jpeg(&DynamicImage::from(canvas), plan.quality)?;
    Ok(Watch {
        image_jpeg,
        cols,
        rows,
        cell_w,
        cell_h,
        frames,
        ms_elapsed: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
        monitor: meta.monitor,
        origin_x: meta.origin_x,
        origin_y: meta.origin_y,
        region,
        screen_state: ScreenState::Live,
    })
}

/// Square-ish grid: `cols = ceil(sqrt(n))`, `rows = ceil(n / cols)`; `(0, 0)` for no frames.
pub(crate) fn grid_layout(n: usize) -> (usize, usize) {
    if n == 0 {
        return (0, 0);
    }
    let mut cols = n.isqrt();
    if cols * cols < n {
        cols += ONE_USIZE;
    }
    (cols, n.div_ceil(cols))
}

/// Pixel span of `count` cells plus the separators between them.
fn span(count: usize, cell: u32) -> u32 {
    let count = u32::try_from(count).unwrap_or(u32::MAX);
    count
        .saturating_mul(cell)
        .saturating_add(count.saturating_sub(1).saturating_mul(WATCH_SEPARATOR_PX))
}

/// One separator precedes every cell except the first in its row/column.
const fn sep(index: usize) -> u32 {
    if index == 0 {
        ZERO_U32
    } else {
        WATCH_SEPARATOR_PX
    }
}

const fn non_live_watch(state: &CameraState, region: Option<Rect>) -> Watch {
    Watch {
        image_jpeg: Vec::new(),
        cols: 0,
        rows: 0,
        cell_w: ZERO_U32,
        cell_h: ZERO_U32,
        frames: Vec::new(),
        ms_elapsed: ZERO_U64,
        monitor: state.monitor,
        origin_x: state.origin_x,
        origin_y: state.origin_y,
        region,
        screen_state: state.screen_state,
    }
}
