//! #179: browser_screenshot must bound clip/output size before allocating.

use super::BrowserScreenshotLimits;

fn code_of(error: &rmcp::model::ErrorData) -> String {
    error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(|code| code.as_str())
        .unwrap_or_default()
        .to_owned()
}

#[test]
fn must_pass_defaults_are_16384_edge_and_100mp() {
    let limits = BrowserScreenshotLimits::default();
    assert_eq!(limits.max_edge, 16_384);
    assert_eq!(limits.max_pixels, 100_000_000);
}

#[test]
fn must_pass_clip_within_bounds_is_accepted() {
    let limits = BrowserScreenshotLimits::default();
    assert!(limits.check_clip(1, 1).is_ok());
    assert!(limits.check_clip(1920, 1080).is_ok());
    assert!(limits.check_clip(16_384, 6_103).is_ok());
}

#[test]
fn must_fail_huge_clip_is_rejected_before_any_allocation() {
    let limits = BrowserScreenshotLimits::default();
    let error = limits
        .check_clip(100_000, 100_000)
        .expect_err("100000x100000 clip must be rejected");
    assert_eq!(
        code_of(&error),
        synapse_core::error_codes::TOOL_PARAMS_INVALID
    );
}

/// The clip check's refusal code; panics when the clip was accepted.
fn clip_refusal(limits: BrowserScreenshotLimits, width: i32, height: i32) -> String {
    match limits.check_clip(width, height) {
        Ok(()) => panic!("clip {width}x{height} must be rejected"),
        Err(error) => code_of(&error),
    }
}

#[test]
fn must_fail_edge_boundary_is_inclusive_and_one_past_is_rejected() {
    let limits = BrowserScreenshotLimits::default();
    assert!(limits.check_clip(16_384, 1).is_ok());
    assert!(limits.check_clip(1, 16_384).is_ok());
    for (width, height) in [(16_385, 1), (1, 16_385)] {
        assert_eq!(
            clip_refusal(limits, width, height),
            synapse_core::error_codes::TOOL_PARAMS_INVALID,
            "{width}x{height}"
        );
    }
}

#[test]
fn must_fail_total_pixel_budget_applies_even_when_each_edge_is_legal() {
    let limits = BrowserScreenshotLimits::default();
    assert!(limits.check_clip(10_000, 10_000).is_ok());
    for (width, height) in [(10_000, 10_001), (16_384, 16_384)] {
        assert_eq!(
            clip_refusal(limits, width, height),
            synapse_core::error_codes::TOOL_PARAMS_INVALID,
            "{width}x{height}"
        );
    }
}

/// Negative dimensions must be rejected, never wrapped into a small or zero
/// extent. (Zero-sized clips are refused earlier, by
/// `m1_tools::validate_browser_screenshot_clip`, which is not reachable from
/// this module.)
#[test]
fn must_fail_non_positive_clip_dimensions_are_rejected_not_wrapped() {
    let limits = BrowserScreenshotLimits::default();
    for (width, height) in [(-1, 10), (10, -1), (-1, -1), (i32::MIN, 10), (10, i32::MIN)] {
        assert_eq!(
            clip_refusal(limits, width, height),
            synapse_core::error_codes::TOOL_PARAMS_INVALID,
            "{width}x{height}"
        );
    }
}

#[test]
fn must_fail_output_bitmap_check_uses_capture_error_code() {
    let limits = BrowserScreenshotLimits::default();
    assert!(limits.check_output(4096, 4096).is_ok());
    let error = limits
        .check_output(u32::MAX, u32::MAX)
        .expect_err("u32::MAX output must be rejected");
    assert_eq!(
        code_of(&error),
        synapse_core::error_codes::CAPTURE_TARGET_INVALID
    );
}

#[test]
fn must_pass_custom_limits_are_honoured() {
    let limits = BrowserScreenshotLimits {
        max_edge: 100,
        max_pixels: 5_000,
    };
    assert!(limits.check_clip(100, 50).is_ok());
    for (width, height) in [(100, 51), (101, 1)] {
        assert_eq!(
            clip_refusal(limits, width, height),
            synapse_core::error_codes::TOOL_PARAMS_INVALID,
            "{width}x{height}"
        );
    }
}

const ENV_VARS: [&str; 2] = [
    "SYNAPSE_BROWSER_SCREENSHOT_MAX_EDGE",
    "SYNAPSE_BROWSER_SCREENSHOT_MAX_PIXELS",
];

/// Serializes this file's env mutation. Only invalid values are ever set, and
/// those resolve to the defaults, so concurrent production readers elsewhere
/// observe the same limits.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn set_limit_env(value: Option<&str>) {
    for name in ENV_VARS {
        // SAFETY: callers hold ENV_LOCK, serializing this file's env mutation;
        // std documents set_var/remove_var as always safe on Windows.
        unsafe {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
    }
}

/// `from_env` is the only constructor production uses (clip and output
/// checks): unset, zero, negative or unparsable overrides keep the defaults.
#[test]
fn must_fail_invalid_env_limits_fall_back_to_defaults() {
    let _lock = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let saved: Vec<Option<std::ffi::OsString>> =
        ENV_VARS.iter().map(|name| std::env::var_os(name)).collect();
    for value in [
        None,
        Some("0"),
        Some("-5"),
        Some("garbage"),
        Some(""),
        Some("1e9"),
    ] {
        set_limit_env(value);
        assert_eq!(
            BrowserScreenshotLimits::from_env(),
            BrowserScreenshotLimits::default(),
            "env override {value:?} must fall back to the defaults"
        );
    }
    for (name, previous) in ENV_VARS.iter().zip(saved) {
        // SAFETY: ENV_LOCK is held; see set_limit_env.
        unsafe {
            match previous {
                Some(previous) => std::env::set_var(name, previous),
                None => std::env::remove_var(name),
            }
        }
    }
}
