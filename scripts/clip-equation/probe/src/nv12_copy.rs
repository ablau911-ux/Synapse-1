//! M13: the product's NV12 converter, so the probe can show that a decoded
//! waiting frame gives the encoder exactly the picture the source gives.
//!
//! `RecordedSize`, `even_up`, `even_down`, `Nv12Canvas` and `clamp_u8` are
//! copied verbatim from `crates/synapse-mcp/src/server/clip_encoder.rs`
//! (branch feature/clip-capture, 2026-11-22). The only addition is the
//! three-field `SegmentInput` stand-in that `RecordedSize::of` reads, and
//! `Nv12Hasher` below, which drives the copy and hashes its output.

use std::hash::{DefaultHasher, Hash, Hasher};

/// The chosen encoder bound (C16): at most this many 16 × 16 macroblocks.
const MAX_ENCODE_MACROBLOCKS: u64 = 36_864;
/// The chosen encoder bound (C16): each side at most this many pixels.
const MAX_ENCODE_SIDE: u32 = 4_096;

/// The three fields of the product's `SegmentInput` that `RecordedSize::of` reads.
struct SegmentInput {
    canvas_width: u32,
    canvas_height: u32,
    scale: u32,
}

// ---- copied verbatim from clip_encoder.rs ----

/// B3: the recorded size σ · canvas, or the largest size within the encoder
/// bound at the same aspect ratio; both even.
struct RecordedSize {
    width: u32,
    height: u32,
    bound_held: bool,
}

impl RecordedSize {
    fn of(input: &SegmentInput) -> Self {
        let width = u64::from(input.canvas_width.max(1)) * u64::from(input.scale.max(1));
        let height = u64::from(input.canvas_height.max(1)) * u64::from(input.scale.max(1));
        let macroblocks = width.div_ceil(16) * height.div_ceil(16);
        let side = u64::from(MAX_ENCODE_SIDE);
        if macroblocks <= MAX_ENCODE_MACROBLOCKS && width <= side && height <= side {
            return Self {
                width: even_up(width),
                height: even_up(height),
                bound_held: true,
            };
        }
        let area_factor = (MAX_ENCODE_MACROBLOCKS as f64 * 256.0 / (width * height) as f64).sqrt();
        let factor = area_factor
            .min(side as f64 / width as f64)
            .min(side as f64 / height as f64);
        Self {
            width: even_down((width as f64 * factor).floor() as u64),
            height: even_down((height as f64 * factor).floor() as u64),
            bound_held: false,
        }
    }
}

/// NV12 needs even sides. Within the bound an odd size is padded up by one
/// black column or row, so every captured pixel stays (B3, G). Padding never
/// crosses the bound: the side limit is even and a 16-pixel macroblock count
/// does not change.
fn even_up(value: u64) -> u32 {
    u32::try_from((value.max(1) + 1) & !1).unwrap_or(!1)
}

/// Above the bound the picture is already shrunk; rounding down keeps it
/// inside the bound.
fn even_down(value: u64) -> u32 {
    u32::try_from(value.max(2) & !1).unwrap_or(!1)
}

/// The picture the encoder sees, kept as NV12 (BT.709 limited range). Frames
/// are placed at the top left with each pixel duplicated σ × σ, never scaled
/// otherwise (E4); the rest of a larger canvas is black. Above the encoder
/// bound the same map picks the nearest source pixel.
struct Nv12Canvas {
    width: usize,
    height: usize,
    /// Source column and row of each output column and row.
    column_source: Vec<usize>,
    row_source: Vec<usize>,
    nv12: Vec<u8>,
}

impl Nv12Canvas {
    fn new(size: &RecordedSize, canvas_width: u32, canvas_height: u32, scale: u32) -> Self {
        let (width, height) = (size.width as usize, size.height as usize);
        let map = |out: usize, canvas: u32| -> Vec<usize> {
            (0..out)
                .map(|at| {
                    if size.bound_held {
                        at / scale.max(1) as usize
                    } else {
                        at * canvas as usize / out.max(1)
                    }
                })
                .collect()
        };
        Self {
            width,
            height,
            column_source: map(width, canvas_width),
            row_source: map(height, canvas_height),
            nv12: vec![0; width * height * 3 / 2],
        }
    }

    /// Draws a BGRA frame of `source_width` × `source_height`, converting to
    /// NV12 with chroma averaged over each 2 × 2 block.
    fn draw(&mut self, bgra: &[u8], source_width: u32, source_height: u32) {
        let (width, height) = (self.width, self.height);
        let (source_width, source_height) = (source_width as usize, source_height as usize);
        let (columns, rows) = (&self.column_source, &self.row_source);
        let rgb_at = |col: usize, row: usize| -> (i32, i32, i32) {
            let (x, y) = (columns[col.min(width - 1)], rows[row.min(height - 1)]);
            if x >= source_width || y >= source_height {
                return (0, 0, 0);
            }
            let offset = (y * source_width + x) * 4;
            bgra.get(offset..offset + 3).map_or((0, 0, 0), |pixel| {
                (
                    i32::from(pixel[2]),
                    i32::from(pixel[1]),
                    i32::from(pixel[0]),
                )
            })
        };
        let (luma, chroma) = self.nv12.split_at_mut(width * height);
        for row in 0..height {
            for col in 0..width {
                let (red, green, blue) = rgb_at(col, row);
                luma[row * width + col] =
                    clamp_u8(((47 * red + 157 * green + 16 * blue + 128) >> 8) + 16);
            }
        }
        for row in (0..height).step_by(2) {
            for col in (0..width).step_by(2) {
                let mut sum = (0, 0, 0);
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let (red, green, blue) = rgb_at(col + dx, row + dy);
                    sum = (sum.0 + red, sum.1 + green, sum.2 + blue);
                }
                let (red, green, blue) = (sum.0 / 4, sum.1 / 4, sum.2 / 4);
                let at = (row / 2) * width + col;
                chroma[at] = clamp_u8(((-26 * red - 87 * green + 112 * blue + 128) >> 8) + 128);
                chroma[at + 1] = clamp_u8(((112 * red - 102 * green - 10 * blue + 128) >> 8) + 128);
            }
        }
    }
}

fn clamp_u8(value: i32) -> u8 {
    u8::try_from(value.clamp(0, 255)).unwrap_or(u8::MAX)
}

// ---- end of the verbatim copy ----

/// Draws frames through the copied converter at one σ and hashes the NV12
/// picture. The canvas is the frame's own size (one window, one segment).
pub struct Nv12Hasher {
    scale: u32,
    canvas: Option<((u32, u32), Nv12Canvas)>,
}

impl Nv12Hasher {
    pub const fn new(scale: u32) -> Self {
        Self {
            scale,
            canvas: None,
        }
    }

    /// SipHash of the NV12 picture `Nv12Canvas::draw` makes from `bgra`.
    pub fn hash(&mut self, bgra: &[u8], width: u32, height: u32) -> u64 {
        if self
            .canvas
            .as_ref()
            .is_none_or(|(size, _)| *size != (width, height))
        {
            let input = SegmentInput {
                canvas_width: width,
                canvas_height: height,
                scale: self.scale,
            };
            let size = RecordedSize::of(&input);
            self.canvas = Some((
                (width, height),
                Nv12Canvas::new(&size, width, height, self.scale),
            ));
        }
        let Some((_, canvas)) = self.canvas.as_mut() else {
            return 0;
        };
        canvas.draw(bgra, width, height);
        let mut hasher = DefaultHasher::new();
        canvas.nv12.hash(&mut hasher);
        hasher.finish()
    }
}
