use std::collections::VecDeque;

pub use synapse_core::Rect;

use crate::{
    config::{
        BLACK_CHANNEL_MAX, BLACK_SAMPLE_STEP_PX, BLUE_CHANNEL_INDEX, BYTES_PER_PIXEL, ONE_U32,
        ONE_USIZE, RED_CHANNEL_INDEX, ZERO_I32, ZERO_U8, ZERO_U32, ZERO_USIZE,
    },
    error::{LivefeedError, Result},
};

#[must_use]
pub fn diff(
    prev: &[u8],
    next: &[u8],
    width: u32,
    height: u32,
    row_stride: usize,
    tile_px: u32,
) -> Vec<Rect> {
    if width == ZERO_U32 || height == ZERO_U32 {
        if prev.is_empty() && next.is_empty() {
            return Vec::new();
        }
        return vec![full_frame_rect(width, height)];
    }
    if !valid_frame_buffers(prev.len(), next.len(), width, height, row_stride, tile_px) {
        return vec![full_frame_rect(width, height)];
    }
    if prev == next {
        return Vec::new();
    }
    let cols = usize::try_from(width.div_ceil(tile_px)).unwrap_or(ZERO_USIZE);
    let rows = usize::try_from(height.div_ceil(tile_px)).unwrap_or(ZERO_USIZE);
    let tile_size = usize::try_from(tile_px).unwrap_or(ONE_USIZE);
    let mut changed =
        build_changed_grid(prev, next, width, height, row_stride, tile_size, cols, rows);
    let mut rects = merge_connected_tiles(&mut changed, cols, rows, tile_size, width, height);
    rects.sort_by_key(|rect| (rect.y, rect.x));
    rects
}

fn valid_frame_buffers(
    prev_len: usize,
    next_len: usize,
    width: u32,
    height: u32,
    row_stride: usize,
    tile_px: u32,
) -> bool {
    if tile_px == ZERO_U32 || prev_len != next_len {
        return false;
    }
    let Ok(width_px) = usize::try_from(width) else {
        return false;
    };
    let Ok(height_px) = usize::try_from(height) else {
        return false;
    };
    let Some(row_bytes) = width_px.checked_mul(BYTES_PER_PIXEL) else {
        return false;
    };
    if row_stride < row_bytes {
        return false;
    }
    let Some(expected_len) = row_stride.checked_mul(height_px) else {
        return false;
    };
    prev_len == expected_len
}

#[allow(clippy::too_many_arguments)]
fn build_changed_grid(
    prev: &[u8],
    next: &[u8],
    width: u32,
    height: u32,
    row_stride: usize,
    tile_size: usize,
    cols: usize,
    rows: usize,
) -> Vec<bool> {
    let width_px = usize::try_from(width).unwrap_or(ZERO_USIZE);
    let height_px = usize::try_from(height).unwrap_or(ZERO_USIZE);
    let mut grid = vec![false; cols.saturating_mul(rows)];
    for ty in ZERO_USIZE..rows {
        for tx in ZERO_USIZE..cols {
            let idx = ty.saturating_mul(cols).saturating_add(tx);
            if let Some(cell) = grid.get_mut(idx) {
                *cell = tile_changed(
                    prev, next, row_stride, tile_size, width_px, height_px, tx, ty,
                );
            }
        }
    }
    grid
}

#[allow(clippy::too_many_arguments)]
fn tile_changed(
    prev: &[u8],
    next: &[u8],
    row_stride: usize,
    tile_size: usize,
    width_px: usize,
    height_px: usize,
    tx: usize,
    ty: usize,
) -> bool {
    let x_start = tx.saturating_mul(tile_size).min(width_px);
    let x_end = tx
        .saturating_add(ONE_USIZE)
        .saturating_mul(tile_size)
        .min(width_px);
    let y_start = ty.saturating_mul(tile_size).min(height_px);
    let y_end = ty
        .saturating_add(ONE_USIZE)
        .saturating_mul(tile_size)
        .min(height_px);
    let byte_start = x_start.saturating_mul(BYTES_PER_PIXEL);
    let byte_end = x_end.saturating_mul(BYTES_PER_PIXEL);
    for row in y_start..y_end {
        let row_offset = row.saturating_mul(row_stride);
        let start = row_offset.saturating_add(byte_start);
        let end = row_offset.saturating_add(byte_end);
        if prev.get(start..end) != next.get(start..end) {
            return true;
        }
    }
    false
}

fn merge_connected_tiles(
    changed: &mut [bool],
    cols: usize,
    rows: usize,
    tile_size: usize,
    width: u32,
    height: u32,
) -> Vec<Rect> {
    let mut rects = Vec::new();
    let mut queue = VecDeque::new();
    for ty in ZERO_USIZE..rows {
        for tx in ZERO_USIZE..cols {
            let start_idx = ty.saturating_mul(cols).saturating_add(tx);
            if !changed.get(start_idx).copied().unwrap_or(false) {
                continue;
            }
            if let Some(cell) = changed.get_mut(start_idx) {
                *cell = false;
            }
            let bounds = flood_fill_component(changed, &mut queue, cols, rows, tx, ty);
            rects.push(bounds_to_rect(bounds, tile_size, width, height));
        }
    }
    rects
}

fn flood_fill_component(
    changed: &mut [bool],
    queue: &mut VecDeque<(usize, usize)>,
    cols: usize,
    rows: usize,
    start_tx: usize,
    start_ty: usize,
) -> (usize, usize, usize, usize) {
    let mut min_tx = start_tx;
    let mut max_tx = start_tx;
    let mut min_ty = start_ty;
    let mut max_ty = start_ty;
    queue.clear();
    queue.push_back((start_tx, start_ty));
    while let Some((cx, cy)) = queue.pop_front() {
        min_tx = min_tx.min(cx);
        max_tx = max_tx.max(cx);
        min_ty = min_ty.min(cy);
        max_ty = max_ty.max(cy);
        visit_neighbors(changed, queue, cols, rows, cx, cy);
    }
    (min_tx, min_ty, max_tx, max_ty)
}

fn visit_neighbors(
    changed: &mut [bool],
    queue: &mut VecDeque<(usize, usize)>,
    cols: usize,
    rows: usize,
    cx: usize,
    cy: usize,
) {
    if cx > ZERO_USIZE {
        try_enqueue(changed, queue, cols, cx - ONE_USIZE, cy);
    }
    if cx.saturating_add(ONE_USIZE) < cols {
        try_enqueue(changed, queue, cols, cx + ONE_USIZE, cy);
    }
    if cy > ZERO_USIZE {
        try_enqueue(changed, queue, cols, cx, cy - ONE_USIZE);
    }
    if cy.saturating_add(ONE_USIZE) < rows {
        try_enqueue(changed, queue, cols, cx, cy + ONE_USIZE);
    }
}

fn try_enqueue(
    changed: &mut [bool],
    queue: &mut VecDeque<(usize, usize)>,
    cols: usize,
    nx: usize,
    ny: usize,
) {
    let idx = ny.saturating_mul(cols).saturating_add(nx);
    if let Some(cell) = changed.get_mut(idx)
        && *cell
    {
        *cell = false;
        queue.push_back((nx, ny));
    }
}

fn bounds_to_rect(
    bounds: (usize, usize, usize, usize),
    tile_size: usize,
    width: u32,
    height: u32,
) -> Rect {
    let (min_tx, min_ty, max_tx, max_ty) = bounds;
    let width_px = usize::try_from(width).unwrap_or(ZERO_USIZE);
    let height_px = usize::try_from(height).unwrap_or(ZERO_USIZE);
    let left = min_tx.saturating_mul(tile_size).min(width_px);
    let top = min_ty.saturating_mul(tile_size).min(height_px);
    let right = max_tx
        .saturating_add(ONE_USIZE)
        .saturating_mul(tile_size)
        .min(width_px);
    let bottom = max_ty
        .saturating_add(ONE_USIZE)
        .saturating_mul(tile_size)
        .min(height_px);
    Rect {
        x: i32::try_from(left).unwrap_or(i32::MAX),
        y: i32::try_from(top).unwrap_or(i32::MAX),
        w: i32::try_from(right.saturating_sub(left)).unwrap_or(i32::MAX),
        h: i32::try_from(bottom.saturating_sub(top)).unwrap_or(i32::MAX),
    }
}

#[must_use]
pub(crate) fn full_frame_rect(width: u32, height: u32) -> Rect {
    Rect {
        x: ZERO_I32,
        y: ZERO_I32,
        w: i32::try_from(width).unwrap_or(i32::MAX),
        h: i32::try_from(height).unwrap_or(i32::MAX),
    }
}

#[must_use]
pub(crate) fn merge_record_rects(
    rects: &[Rect],
    width: u32,
    height: u32,
    tile_px: u32,
) -> Vec<Rect> {
    if width == ZERO_U32 || height == ZERO_U32 || rects.is_empty() {
        return Vec::new();
    }
    let effective_tile_px = tile_px.max(ONE_U32);
    let cols = usize::try_from(width.div_ceil(effective_tile_px)).unwrap_or(ZERO_USIZE);
    let rows = usize::try_from(height.div_ceil(effective_tile_px)).unwrap_or(ZERO_USIZE);
    let tile_size = usize::try_from(effective_tile_px).unwrap_or(ONE_USIZE);
    let mut changed = vec![false; cols.saturating_mul(rows)];
    for &rect in rects {
        if let Some(clamped) = clamp_rect_to_frame(rect, width, height) {
            mark_rect_tiles(&mut changed, clamped, cols, rows, tile_size);
        }
    }
    let mut merged = merge_connected_tiles(&mut changed, cols, rows, tile_size, width, height);
    merged.sort_by_key(|rect| (rect.y, rect.x));
    merged
}

fn mark_rect_tiles(
    changed: &mut [bool],
    clamped: Rect,
    cols: usize,
    rows: usize,
    tile_size: usize,
) {
    let left = usize::try_from(clamped.x).unwrap_or(ZERO_USIZE);
    let top = usize::try_from(clamped.y).unwrap_or(ZERO_USIZE);
    let w = usize::try_from(clamped.w).unwrap_or(ZERO_USIZE);
    let h = usize::try_from(clamped.h).unwrap_or(ZERO_USIZE);
    if w == ZERO_USIZE || h == ZERO_USIZE {
        return;
    }
    let right = left.saturating_add(w);
    let bottom = top.saturating_add(h);
    let tx_start = (left / tile_size).min(cols);
    let tx_end = ((right.saturating_sub(ONE_USIZE)) / tile_size)
        .saturating_add(ONE_USIZE)
        .min(cols);
    let ty_start = (top / tile_size).min(rows);
    let ty_end = ((bottom.saturating_sub(ONE_USIZE)) / tile_size)
        .saturating_add(ONE_USIZE)
        .min(rows);
    for ty in ty_start..ty_end {
        for tx in tx_start..tx_end {
            let idx = ty.saturating_mul(cols).saturating_add(tx);
            if let Some(cell) = changed.get_mut(idx) {
                *cell = true;
            }
        }
    }
}

fn clamp_rect_to_frame(rect: Rect, width: u32, height: u32) -> Option<Rect> {
    let max_x = i64::from(width);
    let max_y = i64::from(height);
    let left = i64::from(rect.x).clamp(i64::from(ZERO_I32), max_x);
    let top = i64::from(rect.y).clamp(i64::from(ZERO_I32), max_y);
    let right = (i64::from(rect.x) + i64::from(rect.w)).clamp(left, max_x);
    let bottom = (i64::from(rect.y) + i64::from(rect.h)).clamp(top, max_y);
    let w = i32::try_from(right - left).unwrap_or(ZERO_I32);
    let h = i32::try_from(bottom - top).unwrap_or(ZERO_I32);
    if w <= ZERO_I32 || h <= ZERO_I32 {
        return None;
    }
    Some(Rect {
        x: i32::try_from(left).unwrap_or(ZERO_I32),
        y: i32::try_from(top).unwrap_or(ZERO_I32),
        w,
        h,
    })
}

#[must_use]
pub(crate) const fn offset_rect(rect: Rect, origin_x: i32, origin_y: i32) -> Rect {
    Rect {
        x: rect.x.saturating_add(origin_x),
        y: rect.y.saturating_add(origin_y),
        w: rect.w,
        h: rect.h,
    }
}

pub(crate) fn translate_to_frame_rect(
    virtual_rect: Rect,
    origin_x: i32,
    origin_y: i32,
) -> Result<Rect> {
    let fx = i64::from(virtual_rect.x) - i64::from(origin_x);
    let fy = i64::from(virtual_rect.y) - i64::from(origin_y);
    let x = i32::try_from(fx).map_err(|_e| LivefeedError::bad_region("rect.x out of range"))?;
    let y = i32::try_from(fy).map_err(|_e| LivefeedError::bad_region("rect.y out of range"))?;
    Ok(Rect {
        x,
        y,
        w: virtual_rect.w,
        h: virtual_rect.h,
    })
}

pub(crate) fn is_black_bgra(bgra: &[u8]) -> bool {
    bgra.is_empty()
        || !bgra
            .chunks_exact(BYTES_PER_PIXEL)
            .step_by(BLACK_SAMPLE_STEP_PX)
            .any(|px| {
                let b = px.get(BLUE_CHANNEL_INDEX).copied().unwrap_or(ZERO_U8);
                let g = px.get(ONE_USIZE).copied().unwrap_or(ZERO_U8);
                let r = px.get(RED_CHANNEL_INDEX).copied().unwrap_or(ZERO_U8);
                b > BLACK_CHANNEL_MAX || g > BLACK_CHANNEL_MAX || r > BLACK_CHANNEL_MAX
            })
}

pub(crate) fn bgra_to_rgba_in_place(bytes: &mut [u8]) {
    for px in bytes.chunks_exact_mut(BYTES_PER_PIXEL) {
        px.swap(BLUE_CHANNEL_INDEX, RED_CHANNEL_INDEX);
    }
}

pub(crate) fn validate_rect_in_frame(rect: Rect, width: u32, height: u32) -> Result<()> {
    if rect.x < ZERO_I32 || rect.y < ZERO_I32 || rect.w <= ZERO_I32 || rect.h <= ZERO_I32 {
        return Err(LivefeedError::bad_region(format!(
            "region {rect:?} must have non-negative origin and positive size"
        )));
    }
    let right = i64::from(rect.x) + i64::from(rect.w);
    let bottom = i64::from(rect.y) + i64::from(rect.h);
    if right > i64::from(width) || bottom > i64::from(height) {
        return Err(LivefeedError::bad_region(format!(
            "region {rect:?} exceeds frame bounds {width}x{height}"
        )));
    }
    Ok(())
}

pub(crate) fn crop_bgra_to_rgba(
    bgra: &[u8],
    width: u32,
    height: u32,
    row_stride: usize,
    rect: Rect,
) -> Result<(u32, u32, Vec<u8>)> {
    validate_rect_in_frame(rect, width, height)?;
    let x0 = usize::try_from(rect.x).map_err(|_e| LivefeedError::bad_region("invalid rect.x"))?;
    let y0 = usize::try_from(rect.y).map_err(|_e| LivefeedError::bad_region("invalid rect.y"))?;
    let crop_w = u32::try_from(rect.w).map_err(|_e| LivefeedError::bad_region("invalid rect.w"))?;
    let crop_h = u32::try_from(rect.h).map_err(|_e| LivefeedError::bad_region("invalid rect.h"))?;
    let row_bytes = usize::try_from(crop_w)
        .unwrap_or(ZERO_USIZE)
        .saturating_mul(BYTES_PER_PIXEL);
    let rows = usize::try_from(crop_h).unwrap_or(ZERO_USIZE);
    let mut rgba = Vec::with_capacity(row_bytes.saturating_mul(rows));
    for row in y0..y0.saturating_add(rows) {
        let start = row
            .saturating_mul(row_stride)
            .saturating_add(x0.saturating_mul(BYTES_PER_PIXEL));
        let slice = bgra
            .get(start..start.saturating_add(row_bytes))
            .ok_or_else(|| LivefeedError::bad_region("crop slice out of bounds"))?;
        rgba.extend_from_slice(slice);
    }
    bgra_to_rgba_in_place(&mut rgba);
    Ok((crop_w, crop_h, rgba))
}
