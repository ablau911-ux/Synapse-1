//! Picture geometry shared by the exact layer, moving rule and viewer.

use serde::{Deserialize, Serialize};

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
