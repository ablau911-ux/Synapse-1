//! Size bounds for `browser_screenshot` (#179).
//!
//! A clip, an element, a full-page extent or a device-pixel-ratio scale can each
//! ask for an output bitmap of tens of GB. These limits are enforced before any
//! CDP call or bitmap allocation. Defaults are overridable by env:
//! `SYNAPSE_BROWSER_SCREENSHOT_MAX_EDGE` (px per side) and
//! `SYNAPSE_BROWSER_SCREENSHOT_MAX_PIXELS` (total px).

use super::ErrorData;
use crate::m1::mcp_error;
use synapse_core::error_codes;

const MAX_EDGE_ENV: &str = "SYNAPSE_BROWSER_SCREENSHOT_MAX_EDGE";
const MAX_PIXELS_ENV: &str = "SYNAPSE_BROWSER_SCREENSHOT_MAX_PIXELS";
const DEFAULT_MAX_EDGE: u64 = 16_384;
const DEFAULT_MAX_PIXELS: u64 = 100_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct BrowserScreenshotLimits {
    pub max_edge: u64,
    pub max_pixels: u64,
}

impl Default for BrowserScreenshotLimits {
    fn default() -> Self {
        Self {
            max_edge: DEFAULT_MAX_EDGE,
            max_pixels: DEFAULT_MAX_PIXELS,
        }
    }
}

impl BrowserScreenshotLimits {
    /// Reads the env overrides; unset, unparsable or zero values keep defaults.
    pub(super) fn from_env() -> Self {
        let read = |name: &str, default: u64| {
            std::env::var(name)
                .ok()
                .and_then(|raw| raw.trim().parse::<u64>().ok())
                .filter(|value| *value > 0)
                .unwrap_or(default)
        };
        Self {
            max_edge: read(MAX_EDGE_ENV, DEFAULT_MAX_EDGE),
            max_pixels: read(MAX_PIXELS_ENV, DEFAULT_MAX_PIXELS),
        }
    }

    /// Rejects a `width` x `height` extent that exceeds the per-edge or total
    /// pixel budget. `what` names the extent (e.g. "clip", "output bitmap").
    pub(super) fn check_extent(
        &self,
        what: &str,
        width: u64,
        height: u64,
        code: &'static str,
    ) -> Result<(), ErrorData> {
        let pixels = width.saturating_mul(height);
        if width > self.max_edge || height > self.max_edge || pixels > self.max_pixels {
            return Err(mcp_error(
                code,
                format!(
                    "browser_screenshot {what} {width}x{height} ({pixels} px) exceeds the limit \
                     of {} px per edge and {} px total; request a smaller clip/element \
                     (limits: {MAX_EDGE_ENV}, {MAX_PIXELS_ENV})",
                    self.max_edge, self.max_pixels
                ),
            ));
        }
        Ok(())
    }

    /// Request-time check for the caller-supplied clip (CSS px).
    pub(super) fn check_clip(&self, width: i32, height: i32) -> Result<(), ErrorData> {
        self.check_extent(
            "clip",
            u64::try_from(width).unwrap_or(u64::MAX),
            u64::try_from(height).unwrap_or(u64::MAX),
            error_codes::TOOL_PARAMS_INVALID,
        )
    }

    /// Check on the final output bitmap, before it is allocated.
    pub(super) fn check_output(&self, width: u32, height: u32) -> Result<(), ErrorData> {
        self.check_extent(
            "output bitmap",
            u64::from(width),
            u64::from(height),
            error_codes::CAPTURE_TARGET_INVALID,
        )
    }
}

#[cfg(test)]
#[path = "tests/browser_screenshot_limits_179.rs"]
mod tests_179;
