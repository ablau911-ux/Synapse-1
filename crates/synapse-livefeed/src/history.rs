use std::{
    mem::size_of,
    time::{SystemTime, UNIX_EPOCH},
};

use image::{DynamicImage, RgbaImage, codecs::jpeg::JpegEncoder, imageops::FilterType};
use synapse_capture::{CapturedFrame, DxgiFormat};
use synapse_core::Rect;

use crate::{
    config::{
        BYTES_PER_PIXEL, BYTES_PER_PIXEL_U8, CameraConfig, DEFAULT_HISTORY_MAX_BYTES,
        DEFAULT_HISTORY_MS, ONE_F32, ONE_USIZE, ZERO_U32, ZERO_U64, ZERO_USIZE,
    },
    error::{LivefeedError, Result},
    tiles::{bgra_to_rgba_in_place, crop_bgra_to_rgba, validate_rect_in_frame},
};

#[derive(Clone, Debug)]
pub(crate) struct RawBgraFrame {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) row_stride: usize,
    pub(crate) bgra: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChangeRecord {
    pub seq: u64,
    pub ts_ms: u64,
    pub width: u32,
    pub height: u32,
    pub rects: Vec<Rect>,
}

impl ChangeRecord {
    #[must_use]
    pub const fn new(seq: u64, ts_ms: u64, width: u32, height: u32, rects: Vec<Rect>) -> Self {
        Self {
            seq,
            ts_ms,
            width,
            height,
            rects,
        }
    }

    #[must_use]
    pub fn byte_size(&self) -> usize {
        size_of::<Self>().saturating_add(self.rects.len().saturating_mul(size_of::<Rect>()))
    }
}

#[derive(Clone, Debug)]
pub struct History {
    entries: Vec<ChangeRecord>,
    history_ms: u64,
    history_max_bytes: usize,
    total_bytes: usize,
}

impl Default for History {
    fn default() -> Self {
        Self::new(DEFAULT_HISTORY_MS, DEFAULT_HISTORY_MAX_BYTES)
    }
}

impl History {
    #[must_use]
    pub const fn new(history_ms: u64, history_max_bytes: usize) -> Self {
        Self {
            entries: Vec::new(),
            history_ms,
            history_max_bytes,
            total_bytes: ZERO_USIZE,
        }
    }

    #[must_use]
    pub const fn from_config(cfg: &CameraConfig) -> Self {
        Self::new(cfg.history_ms, cfg.history_max_bytes)
    }

    pub fn push(&mut self, record: ChangeRecord) {
        self.evict(record.ts_ms);
        let rec_bytes = record.byte_size();
        while !self.entries.is_empty()
            && self.total_bytes.saturating_add(rec_bytes) > self.history_max_bytes
        {
            self.pop_oldest();
        }
        if rec_bytes <= self.history_max_bytes {
            self.total_bytes = self.total_bytes.saturating_add(rec_bytes);
            self.entries.push(record);
        }
    }

    pub fn evict(&mut self, now_ms: u64) {
        while let Some(oldest) = self.entries.first() {
            if now_ms.saturating_sub(oldest.ts_ms) > self.history_ms {
                self.pop_oldest();
            } else {
                break;
            }
        }
    }

    #[must_use]
    pub fn oldest_seq(&self) -> Option<u64> {
        self.entries.first().map(|record| record.seq)
    }

    #[must_use]
    pub fn records_after(&self, seq: u64) -> Option<&[ChangeRecord]> {
        let idx = self.entries.iter().position(|record| record.seq == seq)?;
        self.entries.get(idx.saturating_add(ONE_USIZE)..)
    }

    #[must_use]
    pub(crate) fn record_at(&self, seq: u64) -> Option<&ChangeRecord> {
        self.entries.iter().find(|record| record.seq == seq)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[must_use]
    pub const fn bytes(&self) -> usize {
        self.total_bytes
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.total_bytes = ZERO_USIZE;
    }

    fn pop_oldest(&mut self) {
        if !self.entries.is_empty() {
            let removed = self.entries.remove(ZERO_USIZE);
            self.total_bytes = self.total_bytes.saturating_sub(removed.byte_size());
        }
    }
}

#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
pub(crate) fn scaled_dimensions(width: u32, height: u32, max_long_edge: u32) -> (u32, u32, f32) {
    let long_edge = width.max(height);
    if long_edge <= max_long_edge || long_edge == ZERO_U32 {
        return (width, height, ONE_F32);
    }
    let scale = (max_long_edge as f32) / (long_edge as f32);
    let scaled_w = ((width as f32) * scale).round().max(ONE_F32) as u32;
    let scaled_h = ((height as f32) * scale).round().max(ONE_F32) as u32;
    (scaled_w, scaled_h, scale)
}

/// Converts a tight BGRA frame to RGBA, downscaled so its long edge is at most `long_edge`.
pub(crate) fn downscaled_rgba(
    bgra: &[u8],
    width: u32,
    height: u32,
    long_edge: u32,
) -> Result<RgbaImage> {
    let mut rgba = bgra.to_vec();
    bgra_to_rgba_in_place(&mut rgba);
    let image = RgbaImage::from_raw(width, height, rgba)
        .ok_or_else(|| LivefeedError::capture_failed("invalid RGBA frame buffer"))?;
    let (out_w, out_h, _) = scaled_dimensions(width, height, long_edge);
    if out_w == width && out_h == height {
        return Ok(image);
    }
    Ok(image::imageops::resize(
        &image,
        out_w,
        out_h,
        FilterType::Triangle,
    ))
}

pub(crate) fn encode_look_jpeg(
    bgra: &[u8],
    width: u32,
    height: u32,
    look_long_edge: u32,
    quality: u8,
) -> Result<Vec<u8>> {
    let image = downscaled_rgba(bgra, width, height, look_long_edge)?;
    encode_dynamic_jpeg(&DynamicImage::from(image), quality)
}

pub(crate) fn encode_crop_jpeg(raw: &RawBgraFrame, rect: Rect, quality: u8) -> Result<Vec<u8>> {
    validate_rect_in_frame(rect, raw.width, raw.height)?;
    let (crop_w, crop_h, rgba) =
        crop_bgra_to_rgba(&raw.bgra, raw.width, raw.height, raw.row_stride, rect)?;
    let image = RgbaImage::from_raw(crop_w, crop_h, rgba)
        .ok_or_else(|| LivefeedError::capture_failed("invalid crop image buffer"))?;
    encode_dynamic_jpeg(&DynamicImage::from(image), quality)
}

pub(crate) fn encode_dynamic_jpeg(image: &DynamicImage, quality: u8) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    JpegEncoder::new_with_quality(&mut bytes, quality)
        .encode_image(image)
        .map_err(|err| LivefeedError::capture_failed(format!("JPEG encode failed: {err}")))?;
    Ok(bytes)
}

pub(crate) fn extract_tight_bgra(frame: &CapturedFrame) -> Option<(Vec<u8>, usize)> {
    if frame.width == ZERO_U32
        || frame.height == ZERO_U32
        || frame.pixels.bytes_per_pixel != BYTES_PER_PIXEL_U8
    {
        return None;
    }
    let swap_rb = match frame.format {
        DxgiFormat::Bgra8 | DxgiFormat::Bgra8Srgb => false,
        DxgiFormat::Rgba8 | DxgiFormat::Rgba8Srgb => true,
        _ => return None,
    };
    let width_px = usize::try_from(frame.width).ok()?;
    let height_px = usize::try_from(frame.height).ok()?;
    let tight_stride = width_px.checked_mul(BYTES_PER_PIXEL)?;
    let src_stride = frame.pixels.row_stride_bytes;
    if src_stride < tight_stride {
        return None;
    }
    let mut out = Vec::with_capacity(tight_stride.checked_mul(height_px)?);
    for row in ZERO_USIZE..height_px {
        let start = row.checked_mul(src_stride)?;
        out.extend_from_slice(
            frame
                .pixels
                .bytes
                .get(start..start.checked_add(tight_stride)?)?,
        );
    }
    if swap_rb {
        bgra_to_rgba_in_place(&mut out);
    }
    Some((out, tight_stride))
}

pub(crate) fn current_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(ZERO_U64, |dur| {
            u64::try_from(dur.as_millis()).unwrap_or(u64::MAX)
        })
}
