//! Background-safe `Page.captureScreenshot` over raw CDP (#177).
//!
//! `chromiumoxide::Page::screenshot` sends `Target.activateTarget` before every
//! capture, which switches the human's active Chrome tab and can raise Chrome.
//! Captures here go through `Page::execute(CaptureScreenshotParams)` instead,
//! which only sends `Page.captureScreenshot` to the target session. Nothing in
//! this module may call `activate`, `bring_to_front`, `Target.activateTarget`
//! or `Page.bringToFront`.

#![cfg(windows)]

use std::time::Duration;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use chromiumoxide::Page;
use chromiumoxide::cdp::browser_protocol::page::{
    CaptureScreenshotFormat, CaptureScreenshotParams, Viewport,
};

use crate::{A11yError, A11yResult};

/// A hidden tab may never produce a frame; refuse instead of hanging or
/// switching the user's tab to force one.
pub(crate) const BACKGROUND_CAPTURE_TIMEOUT: Duration = Duration::from_secs(15);

pub(crate) const MAX_CAPTURE_EDGE: u64 = 16_384;
pub(crate) const MAX_CAPTURE_PIXELS: u64 = 100_000_000;

/// Bounds page-controlled extents before capture or output allocation (#179).
pub(crate) fn check_capture_extent(width: u64, height: u64, max_pixels: u64) -> A11yResult<()> {
    if width > MAX_CAPTURE_EDGE
        || height > MAX_CAPTURE_EDGE
        || width.saturating_mul(height) > max_pixels
    {
        return Err(A11yError::CdpAxtreeFailed {
            detail: format!(
                "screenshot {width}x{height} exceeds the limit of {MAX_CAPTURE_EDGE} px per edge and {max_pixels} px total"
            ),
        });
    }
    Ok(())
}

/// Builds PNG `Page.captureScreenshot` params that capture from the surface
/// and never require the tab to be active.
pub(crate) fn background_capture_params(
    clip: Option<Viewport>,
    capture_beyond_viewport: bool,
) -> CaptureScreenshotParams {
    CaptureScreenshotParams {
        format: Some(CaptureScreenshotFormat::Png),
        clip,
        from_surface: Some(true),
        capture_beyond_viewport: capture_beyond_viewport.then_some(true),
        ..Default::default()
    }
}

/// Decodes the base64 `data` field of a `Page.captureScreenshot` response.
pub(crate) fn decode_capture_data(data: &str) -> A11yResult<Vec<u8>> {
    BASE64_STANDARD
        .decode(data)
        .map_err(|err| A11yError::CdpAxtreeFailed {
            detail: format!("Page.captureScreenshot returned invalid base64: {err}"),
        })
}

/// Captures `params` from `page` without activating the target tab.
///
/// # Errors
///
/// `A11Y_CDP_AXTREE_FAILED` if the command fails, the response is not valid
/// base64, or the tab produces no frame within [`BACKGROUND_CAPTURE_TIMEOUT`]
/// (the tab is left untouched; it is never activated to force a frame).
pub(crate) async fn capture_png_without_activation(
    page: &Page,
    params: CaptureScreenshotParams,
) -> A11yResult<Vec<u8>> {
    let response = tokio::time::timeout(BACKGROUND_CAPTURE_TIMEOUT, page.execute(params))
        .await
        .map_err(|_| A11yError::CdpAxtreeFailed {
            detail: format!(
                "Page.captureScreenshot produced no frame within {}s: the target tab is in the \
                 background and cannot be captured without activating it. Synapse will not switch \
                 the user's active tab; capture a visible tab or use a hidden-desktop seat.",
                BACKGROUND_CAPTURE_TIMEOUT.as_secs()
            ),
        })?
        .map_err(|err| A11yError::CdpAxtreeFailed {
            detail: format!("Page.captureScreenshot: {err}"),
        })?;
    decode_capture_data(AsRef::<str>::as_ref(&response.result.data))
}

#[cfg(test)]
#[path = "tests/cdp_capture_177.rs"]
mod tests_177;
