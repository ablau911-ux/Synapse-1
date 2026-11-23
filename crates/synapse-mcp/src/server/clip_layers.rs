//! The ordered split of each source instant into exact and moving pixels.
use std::path::Path;

use super::clip_encoder::ClipEncoder;
use super::clip_exact::{ExactRef, ExactStream, ExactSummary};
use super::clip_motion::{BlockGrid, ChangeMap, MOVING_DELTA, MovingAreas, PixelRect};
use super::clip_moving::{
    MovingAvailability, MovingCoder, MovingRef, MovingSegment, MovingSummary,
};

const BGRA_CHANNELS: usize = 4;
const BGR_CHANNELS: usize = 3;

#[derive(Default)]
struct SourcePictures {
    width: u32,
    height: u32,
    current: Vec<u8>,
    previous: Vec<u8>,
    has_previous: bool,
}
impl SourcePictures {
    fn accept(&mut self, bgra: &[u8], width: u32, height: u32) -> Result<bool, String> {
        if width == 0
            || height == 0
            || bgra.len() != width as usize * height as usize * BGRA_CHANNELS
        {
            return Err("source pixels do not match dimensions".into());
        }
        let resized = (self.width, self.height) != (width, height);
        self.has_previous = !resized && !self.current.is_empty();
        std::mem::swap(&mut self.current, &mut self.previous);
        self.current.clear();
        self.current.extend(
            bgra.chunks_exact(BGRA_CHANNELS)
                .flat_map(|p| p[..BGR_CHANNELS].iter().copied()),
        );
        self.width = width;
        self.height = height;
        Ok(resized)
    }
    fn current(&self) -> &[u8] {
        &self.current
    }
    fn previous(&self) -> Option<&[u8]> {
        self.has_previous.then_some(self.previous.as_slice())
    }
}

pub(super) struct Written {
    pub exact: Option<ExactRef>,
    pub moving: Option<MovingRef>,
    pub moving_rect: Option<PixelRect>,
    pub moving_behind: bool,
    pub committed: Vec<MovingSegment>,
    pub bytes: u64,
}
#[derive(serde::Serialize)]
pub(super) struct LayersSummary {
    pub exact: ExactSummary,
    pub moving: MovingSummary,
}

pub(super) struct ClipLayers {
    source: SourcePictures,
    grid: BlockGrid,
    areas: MovingAreas,
    exact: ExactStream,
    moving: MovingCoder,
    current: Option<PixelRect>,
    fps: u32,
}
impl ClipLayers {
    pub fn start(folder: &Path, width: u32, height: u32, fps: u32) -> Result<Self, String> {
        Ok(Self {
            source: SourcePictures::default(),
            grid: BlockGrid::new(width as usize, height as usize),
            areas: MovingAreas::new(width, height, fps, ClipEncoder::accepts),
            exact: ExactStream::create(folder, width, height)?,
            moving: MovingCoder::start(folder, fps)?,
            current: None,
            fps,
        })
    }
    pub fn picture(
        &mut self,
        instant: u64,
        bgra: &[u8],
        width: u32,
        height: u32,
    ) -> Result<Written, String> {
        if self.source.accept(bgra, width, height)? {
            self.exact.resize(width, height)?;
            self.grid = BlockGrid::new(width as usize, height as usize);
            self.areas = MovingAreas::new(width, height, self.fps, ClipEncoder::accepts);
            self.moving
                .follow(instant, None, self.source.current(), width, false);
            self.current = None;
        }
        let changes = self.source.previous().map_or_else(
            || ChangeMap::none(&self.grid),
            |previous| {
                ChangeMap::between(previous, self.source.current(), &self.grid, MOVING_DELTA)
            },
        );
        self.write(instant, Some(&changes))
    }
    pub fn repeat(&mut self, instant: u64) -> Result<Written, String> {
        self.write(instant, None)
    }
    fn write(&mut self, instant: u64, changes: Option<&ChangeMap>) -> Result<Written, String> {
        if matches!(
            MovingAvailability::get(),
            MovingAvailability::Unavailable(_)
        ) {
            self.areas.disable();
        }
        let rect = self.areas.step(instant, changes);
        let step = self.moving.follow(
            instant,
            rect,
            self.source.current(),
            self.source.width,
            changes.is_some(),
        );
        // A failure observed by follow must stop freezing pixels immediately.
        let rect = step.reference.and(rect);
        let exact = self.exact.write(self.source.current(), rect)?;
        if let (Some(record), Some(changes)) = (exact, changes)
            && !record.is_key()
            && rect == self.current
        {
            self.areas.exact_written(
                instant,
                u64::from(record.bytes),
                changes.count_outside(&self.grid, rect),
            );
        }
        self.current = rect;
        Ok(Written {
            exact,
            moving: step.reference,
            moving_rect: step.began,
            moving_behind: step.behind,
            committed: self.moving.take_committed(),
            bytes: exact.map_or(0, |r| u64::from(r.bytes)),
        })
    }
    pub fn cpu_hns(&self) -> i64 {
        self.moving.cpu_hns()
    }
    pub fn finish(mut self) -> Result<LayersSummary, String> {
        let exact = self.exact.finish()?;
        let moving = self.moving.finish();
        Ok(LayersSummary { exact, moving })
    }
}
