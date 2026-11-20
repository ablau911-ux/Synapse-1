//! #191: window-scoped pixel evidence for `verify_delta`.
//!
//! The pixel signature of an action target is hashed from the TARGET window's
//! own capture (a whole-window WGC frame), never from a screen-DC region at the
//! window's bounds. A screen capture shows whatever window is on top, so a
//! background or occluded target was measured through its cover: a posted click
//! the target ignored looked like a delta when the covering window repainted,
//! and a real change under a cover looked like `ACTION_NO_OBSERVED_DELTA`.
//!
//! When the window capture fails there is deliberately NO screen fallback: the
//! pixel evidence is reported `unavailable` and the other signature fields
//! (focus, UI tree) carry the verification.

use sha2::{Digest as _, Sha256};
use synapse_capture::{CaptureError, CapturedBgraBitmap, CapturedWindowBgraBitmap};
use synapse_core::{Point, Rect};

use super::{ClickPixelSignature, hex_encode};

/// Upper bound for one whole-window WGC frame used as verification evidence.
const PIXEL_EVIDENCE_CAPTURE_TIMEOUT_MS: u64 = 1_500;

/// Pixel evidence for one signature: the whole target window, plus an optional
/// point-centred region cropped from that same frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct WindowPixelEvidence {
    pub(super) window: ClickPixelSignature,
    pub(super) point: Option<ClickPixelSignature>,
}

/// Captures the pixel evidence of `hwnd` through the window itself (WGC).
/// `unavailable_region` only labels the evidence when the capture fails.
pub(super) fn capture_window_pixel_evidence(
    hwnd: i64,
    unavailable_region: Rect,
    point: Option<Point>,
) -> WindowPixelEvidence {
    capture_window_pixel_evidence_with(
        hwnd,
        unavailable_region,
        point,
        false,
        || crate::server::m1_tools::capture_window_identity(hwnd),
        || {
            // The WGC window frame is the DWM extended frame (the visible window,
            // without the invisible resize border); its bitmap `region` is
            // window-local (0, 0). Its screen position is read before and after
            // the capture and kept only when it did not move (#2332).
            let bounds_before = synapse_capture::window_capture_bounds(hwnd);
            let captured = synapse_capture::window_full_frame_to_bgra_bitmap(
                hwnd,
                PIXEL_EVIDENCE_CAPTURE_TIMEOUT_MS,
            )
            .map_err(|error| crate::m1::mcp_error(error.code(), error.to_string()))?;
            let bounds_after = synapse_capture::window_capture_bounds(hwnd);
            let origin = bounds_before
                .ok()
                .zip(bounds_after.ok())
                .and_then(|(before, after)| {
                    crate::m1::stable_capture_origin(
                        before,
                        after,
                        captured.bitmap.width,
                        captured.bitmap.height,
                    )
                })
                .map(|frame| Point {
                    x: frame.x,
                    y: frame.y,
                });
            Ok((captured, origin))
        },
    )
}

fn capture_window_pixel_evidence_with(
    _hwnd: i64,
    unavailable_region: Rect,
    point: Option<Point>,
    reject_blank: bool,
    mut identity: impl FnMut() -> Result<(i64, u32), rmcp::ErrorData>,
    capture: impl FnOnce() -> Result<(CapturedWindowBgraBitmap, Option<Point>), rmcp::ErrorData>,
) -> WindowPixelEvidence {
    if unavailable_region.w <= 0 || unavailable_region.h <= 0 {
        return WindowPixelEvidence {
            window: unavailable(
                unavailable_region,
                "target window bounds are empty".to_owned(),
            ),
            point: point.map(|point| {
                PointBox::around(point).unavailable("target window bounds are empty".to_owned())
            }),
        };
    }
    let evidence = (|| {
        let expected = identity()?;
        let metadata = std::cell::RefCell::new(None);
        crate::server::m1_tools::capture_with_verified_identity_result(
            expected,
            reject_blank,
            identity,
            || {
                let captured = capture()?;
                *metadata.borrow_mut() = Some(captured);
                let mut stored = metadata.borrow_mut();
                let (captured, _origin) = stored.as_mut().ok_or_else(|| {
                    crate::m1::mcp_error(
                        synapse_core::error_codes::CAPTURE_FRAME_UNAVAILABLE,
                        "pixel capture missing",
                    )
                })?;
                Ok(std::mem::replace(
                    &mut captured.bitmap,
                    synapse_capture::CapturedBgraBitmap {
                        region: unavailable_region,
                        width: 0,
                        height: 0,
                        bytes: Vec::new(),
                    },
                ))
            },
            |bitmap| {
                let (mut captured, origin) = metadata.borrow_mut().take().ok_or_else(|| {
                    crate::m1::mcp_error(
                        synapse_core::error_codes::CAPTURE_FRAME_UNAVAILABLE,
                        "pixel capture metadata missing",
                    )
                })?;
                captured.bitmap = bitmap;
                Ok(window_pixel_evidence_from_capture(
                    Ok(captured),
                    unavailable_region,
                    point,
                    origin,
                ))
            },
        )
    })();
    evidence.unwrap_or_else(|error| WindowPixelEvidence {
        window: unavailable(unavailable_region, error.to_string()),
        point: point.map(|point| PointBox::around(point).unavailable(error.to_string())),
    })
}

/// Pure conversion of a window capture result into pixel evidence. It takes no
/// screen source, so a failed window capture can only become `unavailable`.
/// `capture_origin` is the screen position of the capture's pixel (0, 0); the
/// point box needs it (`None`: the point evidence is `unavailable`).
pub(super) fn window_pixel_evidence_from_capture(
    captured: Result<CapturedWindowBgraBitmap, CaptureError>,
    unavailable_region: Rect,
    point: Option<Point>,
    capture_origin: Option<Point>,
) -> WindowPixelEvidence {
    let captured = match captured {
        Ok(captured) => captured,
        Err(error) => {
            let detail = format!("target window capture failed: {error}");
            return WindowPixelEvidence {
                window: unavailable(unavailable_region, detail.clone()),
                point: point.map(|point| PointBox::around(point).unavailable(detail.clone())),
            };
        }
    };
    let bitmap = captured.bitmap;
    let window = ClickPixelSignature {
        status: "ok".to_owned(),
        region: bitmap.region,
        bitmap_sha256: Some(hash_bgra(bitmap.width, bitmap.height, &bitmap.bytes)),
        detail: Some(format!(
            "captured {}x{} BGRA bytes={} backend={}",
            bitmap.width,
            bitmap.height,
            bitmap.bytes.len(),
            captured.capture_backend
        )),
    };
    let point = point.map(|point| PointBox::around(point).signature(&bitmap, capture_origin));
    WindowPixelEvidence { window, point }
}

fn unavailable(region: Rect, detail: String) -> ClickPixelSignature {
    ClickPixelSignature {
        status: "unavailable".to_owned(),
        region,
        bitmap_sha256: None,
        detail: Some(detail),
    }
}

/// Plain SHA-256 on purpose (#174): before/after pixel signature of window
/// pixels (not text), used only to detect that the pixels changed.
fn hash_bgra(width: u32, height: u32, bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(width.to_le_bytes());
    hasher.update(height.to_le_bytes());
    hasher.update(bytes);
    hex_encode(&hasher.finalize())
}

#[cfg(test)]
#[path = "tests/native_pixel_transaction_246.rs"]
mod native_pixel_transaction_246_tests;

/// The pixels around a click point: a box in screen coordinates that is placed
/// on a window capture through the capture's own screen origin (#2332, real-app
/// finding of 2026-11-19: the box was cut from the window-local capture with
/// screen coordinates, so it only fitted a window at the top-left corner).
pub(super) struct PointBox {
    screen: Rect,
}

impl PointBox {
    /// The box around a click point given in screen coordinates: 512 px left
    /// and right, 192 px above and below. Wide list rows often leave blank
    /// padding under the pointer, so nearby content is included.
    pub(super) const fn around(point: Point) -> Self {
        const HALF_WIDTH: i32 = 512;
        const HALF_HEIGHT: i32 = 192;
        Self {
            screen: Rect {
                x: point.x.saturating_sub(HALF_WIDTH),
                y: point.y.saturating_sub(HALF_HEIGHT),
                w: HALF_WIDTH * 2,
                h: HALF_HEIGHT * 2,
            },
        }
    }

    /// The part of the box that lies inside a capture, in the capture's own
    /// pixel coordinates. `capture_origin` is the screen position of the
    /// capture's pixel (0, 0). `None` when the box does not overlap the capture
    /// (touching an edge is no overlap).
    pub(super) fn in_capture(
        &self,
        capture_origin: Point,
        capture_width: u32,
        capture_height: u32,
    ) -> Option<Rect> {
        let capture = Rect {
            x: capture_origin.x,
            y: capture_origin.y,
            w: i32::try_from(capture_width).ok()?,
            h: i32::try_from(capture_height).ok()?,
        };
        crate::m1::crop_element_to_window_frame(self.screen, capture).map(|crop| crop.window_region)
    }

    /// The pixel evidence of the box, cut from a window-local capture whose
    /// pixel (0, 0) sits at `capture_origin` on the screen. The reported region
    /// is in SCREEN coordinates, like the click point and the `unavailable`
    /// label (for a window at the screen corner this is what the reply always
    /// showed); the detail names the position inside the capture.
    fn signature(
        &self,
        bitmap: &CapturedBgraBitmap,
        capture_origin: Option<Point>,
    ) -> ClickPixelSignature {
        let Some(origin) = capture_origin else {
            return self.unavailable(
                "screen position of the target window capture is unknown (window bounds unreadable or changed during the capture)"
                    .to_owned(),
            );
        };
        let Some((inside, crop)) = self
            .in_capture(origin, bitmap.width, bitmap.height)
            .and_then(|inside| Some((inside, bitmap.crop_screen_rect(inside)?)))
        else {
            return self
                .unavailable("point region lies outside the target window capture".to_owned());
        };
        ClickPixelSignature {
            status: "ok".to_owned(),
            region: Rect {
                x: inside.x.saturating_add(origin.x),
                y: inside.y.saturating_add(origin.y),
                w: inside.w,
                h: inside.h,
            },
            bitmap_sha256: Some(hash_bgra(crop.width, crop.height, &crop.bytes)),
            detail: Some(format!(
                "cropped {}x{} BGRA bytes={} at capture ({}, {}) from the target window capture",
                crop.width,
                crop.height,
                crop.bytes.len(),
                inside.x,
                inside.y
            )),
        }
    }

    fn unavailable(&self, detail: String) -> ClickPixelSignature {
        unavailable(self.screen, detail)
    }
}

// The frozen table's row tuple trips `type_complexity` (deny via clippy::all);
// the table file itself may not be edited, so the allow sits on its module.
#[cfg(test)]
#[path = "m2_pixel_signature_table_2332b.rs"]
#[allow(clippy::type_complexity)]
mod table_2332b;
