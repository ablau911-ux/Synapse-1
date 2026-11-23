//! Picture geometry shared by the exact layer, moving rule and viewer.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

pub const BLOCK: usize = 8;
pub const MOVING_DELTA: u8 = 0;
pub const SHORT_LIVED_MS: u64 = 500;
pub const ANIMATING_CHANGE_SHARE: f64 = 0.5;
pub const MOVING_MIN_BLOCKS: usize = 64;
pub const MOVING_MIN_SIDE_PX: u32 = 64;
pub const MOVING_ALIGN_PX: u32 = 16;
pub const MOVING_HOLD_MS: u64 = 500;
pub const MOVING_COST_SHARE: f64 = 0.25;
const CHANNELS: usize = 3;
const MILLISECONDS_PER_SECOND: u64 = 1_000;
const PERCENT_TENTHS: f64 = 1_000.0;
const TENTHS: f64 = 10.0;
const CHROMA_SIDE: u32 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "[u32; 4]", into = "[u32; 4]")]
pub struct PixelRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl From<[u32; 4]> for PixelRect {
    fn from([x, y, w, h]: [u32; 4]) -> Self {
        Self { x, y, w, h }
    }
}

impl From<PixelRect> for [u32; 4] {
    fn from(rect: PixelRect) -> Self {
        [rect.x, rect.y, rect.w, rect.h]
    }
}

impl PixelRect {
    pub fn right(&self) -> u32 {
        self.x.saturating_add(self.w)
    }

    pub fn bottom(&self) -> u32 {
        self.y.saturating_add(self.h)
    }

    pub fn area(&self) -> u64 {
        u64::from(self.w) * u64::from(self.h)
    }

    pub fn contains(&self, other: &Self) -> bool {
        self.x <= other.x
            && self.y <= other.y
            && self.right() >= other.right()
            && self.bottom() >= other.bottom()
    }

    pub fn union(&self, other: &Self) -> Self {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        Self {
            x,
            y,
            w: self.right().max(other.right()) - x,
            h: self.bottom().max(other.bottom()) - y,
        }
    }

    pub fn intersect(&self, other: &Self) -> Option<Self> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        (right > x && bottom > y).then(|| Self {
            x,
            y,
            w: right - x,
            h: bottom - y,
        })
    }

    pub fn aligned(&self, step: u32, width: u32, height: u32) -> Self {
        let step = step.max(1);
        let x = (self.x / step * step).min(width);
        let y = (self.y / step * step).min(height);
        let right = self.right().div_ceil(step).saturating_mul(step).min(width);
        let bottom = self
            .bottom()
            .div_ceil(step)
            .saturating_mul(step)
            .min(height);
        Self {
            x,
            y,
            w: right.saturating_sub(x),
            h: bottom.saturating_sub(y),
        }
    }
}

/// The shared block layout and pixel comparison for recorder and viewer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockGrid {
    pub width: usize,
    pub height: usize,
    pub cols: usize,
    pub rows: usize,
}
impl BlockGrid {
    pub fn new(width: usize, height: usize) -> Self {
        Self { width, height, cols: width.div_ceil(BLOCK), rows: height.div_ceil(BLOCK) }
    }
    pub fn blocks(self) -> usize { self.cols * self.rows }
    pub fn place(self, b: usize) -> (usize, usize) { (b % self.cols.max(1), b / self.cols.max(1)) }
    pub fn span(self, b: usize) -> (usize, usize, usize, usize) {
        let (x, y) = self.place(b);
        (x * BLOCK, y * BLOCK, ((x + 1) * BLOCK).min(self.width), ((y + 1) * BLOCK).min(self.height))
    }
    pub fn rows_of(self, b: usize) -> impl Iterator<Item = std::ops::Range<usize>> {
        let (x, y, right, bottom) = self.span(b);
        (y..bottom).map(move |row| (row * self.width + x) * CHANNELS..(row * self.width + right) * CHANNELS)
    }
    pub fn near(self, a: usize, b: usize, gap: (usize, usize)) -> bool {
        let ((ax, ay), (bx, by)) = (self.place(a), self.place(b));
        ax.abs_diff(bx) <= gap.0 + 1 && ay.abs_diff(by) <= gap.1 + 1
    }
    pub fn bbox(self, blocks: impl IntoIterator<Item = usize>) -> Option<PixelRect> {
        blocks.into_iter().map(|b| {
            let (x, y, right, bottom) = self.span(b);
            PixelRect { x: x as u32, y: y as u32, w: (right - x) as u32, h: (bottom - y) as u32 }
        }).reduce(|a, b| a.union(&b))
    }
    pub fn percent(self, count: u64) -> f64 {
        (count as f64 * PERCENT_TENTHS / self.blocks().max(1) as f64).round() / TENTHS
    }
    pub fn differs(self, a: &[u8], b: &[u8], block: usize, delta: u8) -> bool {
        self.rows_of(block).any(|range| a[range.clone()].iter().zip(&b[range]).any(|(x, y)| x.abs_diff(*y) > delta))
    }
    pub fn groups(self, flags: &[bool]) -> Vec<Vec<usize>> {
        let mut seen = vec![false; self.blocks()];
        let mut groups = Vec::new();
        for first in 0..self.blocks() {
            if seen[first] || !flags[first] { continue; }
            seen[first] = true;
            let mut stack = vec![first];
            let mut group = Vec::new();
            while let Some(b) = stack.pop() {
                group.push(b);
                let (x, y) = self.place(b);
                for row in y.saturating_sub(1)..=(y + 1).min(self.rows - 1) {
                    for col in x.saturating_sub(1)..=(x + 1).min(self.cols - 1) {
                        let next = row * self.cols + col;
                        if flags[next] && !seen[next] { seen[next] = true; stack.push(next); }
                    }
                }
            }
            groups.push(group);
        }
        groups
    }
}

#[derive(Clone, Copy)]
pub struct MotionTiming { pub activity: u64, pub animating: usize, pub hold: u64 }
impl MotionTiming {
    pub fn of(fps: u32) -> Self {
        let activity = (SHORT_LIVED_MS * u64::from(fps) / MILLISECONDS_PER_SECOND).max(1);
        Self { activity, animating: (activity as f64 * ANIMATING_CHANGE_SHARE).ceil() as usize,
            hold: (MOVING_HOLD_MS * u64::from(fps) / MILLISECONDS_PER_SECOND).max(1) }
    }
}
#[derive(Clone, Default)]
pub struct MotionWindow { moves: VecDeque<u64> }
impl MotionWindow {
    pub fn moved(&mut self, instant: u64, timing: &MotionTiming) {
        if self.moves.len() == timing.animating { self.moves.pop_front(); }
        self.moves.push_back(instant);
    }
    pub fn animating_until(&self, timing: &MotionTiming) -> u64 {
        self.moves.front().filter(|_| self.moves.len() >= timing.animating).map_or(0, |first| first + timing.activity)
    }
    pub fn animating(&self, instant: u64, timing: &MotionTiming) -> bool { instant < self.animating_until(timing) }
    pub fn first_move(&self) -> Option<u64> { self.moves.front().copied() }
}

pub struct ChangeMap { pub changed: Vec<bool>, pub count: usize }
impl ChangeMap {
    pub fn none(grid: &BlockGrid) -> Self { Self { changed: vec![false; grid.blocks()], count: 0 } }
    pub fn between(previous: &[u8], current: &[u8], grid: &BlockGrid, delta: u8) -> Self {
        let mut map = Self::none(grid);
        let stride = grid.width * CHANNELS;
        for row in 0..grid.height {
            let start = row * stride;
            if previous[start..start + stride] == current[start..start + stride] { continue; }
            for col in 0..grid.cols {
                let b = row / BLOCK * grid.cols + col;
                if map.changed[b] { continue; }
                let first = start + col * BLOCK * CHANNELS;
                let end = (first + BLOCK * CHANNELS).min(start + stride);
                if previous[first..end].iter().zip(&current[first..end]).any(|(a, b)| a.abs_diff(*b) > delta) {
                    map.changed[b] = true;
                    map.count += 1;
                }
            }
        }
        map
    }
    pub fn is_changed(&self, b: usize) -> bool { self.changed[b] }
    pub fn count_outside(&self, grid: &BlockGrid, rect: Option<PixelRect>) -> u64 {
        self.changed.iter().enumerate().filter(|(b, changed)| **changed && !rect.is_some_and(|r| grid.bbox([*b]).is_some_and(|block| r.contains(&block)))).count() as u64
    }
}

#[derive(Default)]
struct CostWindow { records: VecDeque<(u64, u64, u64)> }
impl CostWindow {
    fn add(&mut self, instant: u64, bytes: u64, blocks: u64) { self.records.push_back((instant, bytes, blocks)); }
    fn costly(&mut self, instant: u64, timing: &MotionTiming) -> bool {
        while self.records.front().is_some_and(|(at, _, _)| at.saturating_add(timing.activity) <= instant) { self.records.pop_front(); }
        let (bytes, blocks) = self.records.iter().fold((0u64, 0u64), |(z, c), (_, bytes, blocks)| (z + bytes, c + blocks));
        blocks > 0 && bytes as f64 >= MOVING_COST_SHARE * (BLOCK * BLOCK * CHANNELS) as f64 * blocks as f64
    }
}

pub struct MovingAreas {
    grid: BlockGrid,
    timing: MotionTiming,
    windows: Vec<MotionWindow>,
    targets: VecDeque<(u64, PixelRect)>,
    cost: CostWindow,
    current: Option<PixelRect>,
    enabled: bool,
    fits_encoder: fn(u32, u32) -> bool,
}
impl MovingAreas {
    pub fn new(width: u32, height: u32, fps: u32, fits_encoder: fn(u32, u32) -> bool) -> Self {
        let grid = BlockGrid::new(width as usize, height as usize);
        Self { grid, timing: MotionTiming::of(fps), windows: vec![MotionWindow::default(); grid.blocks()], targets: VecDeque::new(), cost: CostWindow::default(), current: None, enabled: true, fits_encoder }
    }
    pub fn step(&mut self, instant: u64, changes: Option<&ChangeMap>) -> Option<PixelRect> {
        if !self.enabled { return None; }
        let costly = self.cost.costly(instant, &self.timing);
        let mut admitted = vec![false; self.grid.blocks()];
        for (b, window) in self.windows.iter_mut().enumerate() {
            if changes.is_some_and(|map| map.is_changed(b)) { window.moved(instant, &self.timing); }
            admitted[b] = window.animating(instant, &self.timing) && (costly || self.current.is_some_and(|rect| self.grid.bbox([b]).is_some_and(|block| rect.contains(&block))));
        }
        let groups = self.grid.groups(&admitted);
        // Groups begin in block-index order; strict improvement preserves the tie rule.
        let mut largest: &[usize] = &[];
        for group in &groups { if group.len() > largest.len() { largest = group; } }
        if largest.len() >= MOVING_MIN_BLOCKS && let Some(target) = self.grid.bbox(largest.iter().copied()) { self.targets.push_back((instant, target)); }
        while self.targets.front().is_some_and(|(at, _)| at.saturating_add(self.timing.hold) <= instant) { self.targets.pop_front(); }
        self.current = self.targets.iter().map(|(_, rect)| *rect).reduce(|a, b| a.union(&b))
            .map(|r| r.aligned(MOVING_ALIGN_PX, self.grid.width as u32, self.grid.height as u32))
            .filter(|r| r.w >= MOVING_MIN_SIDE_PX && r.h >= MOVING_MIN_SIDE_PX && (self.fits_encoder)(r.w.div_ceil(CHROMA_SIDE) * CHROMA_SIDE, r.h.div_ceil(CHROMA_SIDE) * CHROMA_SIDE));
        self.current
    }
    pub fn exact_written(&mut self, instant: u64, bytes: u64, changed_blocks: u64) { self.cost.add(instant, bytes, changed_blocks); }
    pub fn disable(&mut self) { self.enabled = false; self.current = None; self.targets.clear(); }
}
