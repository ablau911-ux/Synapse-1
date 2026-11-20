//! Building `changes` results: union of change records, cut detection, crops.

use synapse_core::Rect;

use super::{ChangeRegion, Changes, FrameMeta, ScreenState};
use crate::{
    config::{CameraConfig, PERCENT_MAX, ZERO_U64},
    error::Result,
    history::{ChangeRecord, RawBgraFrame, encode_crop_jpeg},
    tiles::{full_frame_rect, merge_record_rects, offset_rect},
};

pub(super) fn collect_merged_rects(
    since_dims: Option<(u32, u32)>,
    records: &[ChangeRecord],
    width: u32,
    height: u32,
    tile_px: u32,
) -> Vec<Rect> {
    if since_dims.is_some_and(|(w, h)| w != width || h != height)
        || records
            .iter()
            .any(|rec| rec.width != width || rec.height != height)
    {
        return vec![full_frame_rect(width, height)];
    }
    let mut union = Vec::new();
    for rec in records {
        union.extend_from_slice(&rec.rects);
    }
    merge_record_rects(&union, width, height, tile_px)
}

/// Changed area at or above `ratio_percent` of the frame counts as a scene cut.
fn is_cut(rects: &[Rect], width: u32, height: u32, ratio_percent: u64) -> bool {
    let frame_area = u64::from(width) * u64::from(height);
    if frame_area == ZERO_U64 {
        return false;
    }
    let changed: u64 = rects
        .iter()
        .map(|r| u64::try_from(r.w).unwrap_or(ZERO_U64) * u64::try_from(r.h).unwrap_or(ZERO_U64))
        .sum();
    changed.saturating_mul(PERCENT_MAX) >= frame_area.saturating_mul(ratio_percent)
}

pub(super) fn build_changes_result(
    since: u64,
    meta: FrameMeta,
    frame_rects: Vec<Rect>,
    latest_raw: &RawBgraFrame,
    cfg: &CameraConfig,
    settled: bool,
) -> Result<Changes> {
    let too_many = frame_rects.len() > cfg.max_change_crops;
    let cut = is_cut(
        &frame_rects,
        latest_raw.width,
        latest_raw.height,
        cfg.cut_ratio_percent,
    );
    let mut regions = Vec::with_capacity(frame_rects.len());
    for frame_rect in frame_rects {
        let crop_jpeg = if too_many {
            Vec::new()
        } else {
            encode_crop_jpeg(latest_raw, frame_rect, cfg.jpeg_quality)?
        };
        regions.push(ChangeRegion {
            rect: offset_rect(frame_rect, meta.origin_x, meta.origin_y),
            crop_jpeg,
        });
    }
    Ok(Changes {
        since,
        seq: meta.seq,
        ts_ms: meta.ts_ms,
        regions,
        too_many,
        since_evicted: false,
        cut,
        settled,
        screen_state: ScreenState::Live,
        monitor: meta.monitor,
        origin_x: meta.origin_x,
        origin_y: meta.origin_y,
    })
}
