//! #177: raw-CDP captures must never activate the user's tab.

use chromiumoxide::cdp::browser_protocol::page::{CaptureScreenshotFormat, Viewport};

use super::{background_capture_params, decode_capture_data};

const CDP_CAPTURE_SRC: &str = include_str!("../cdp_capture.rs");

/// Source with `//` comments removed, so doc text may mention the banned names.
fn code_only(src: &str) -> String {
    src.lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn must_fail_capture_module_never_activates_target() {
    let capture = code_only(
        CDP_CAPTURE_SRC
            .split("#[cfg(test)]")
            .next()
            .unwrap_or_default(),
    );
    for banned in [
        ".screenshot(",
        ".activate(",
        "bring_to_front",
        "BringToFront",
        "ActivateTarget",
        "cdp_activate_target",
    ] {
        assert!(
            !capture.contains(banned),
            "cdp_capture.rs must not contain {banned}"
        );
    }
}

#[test]
fn must_pass_background_params_are_surface_png_without_activation_flags() {
    let params = background_capture_params(None, false);
    assert_eq!(params.format, Some(CaptureScreenshotFormat::Png));
    assert_eq!(params.from_surface, Some(true));
    assert_eq!(params.capture_beyond_viewport, None);
    assert!(params.clip.is_none());
}

#[test]
fn must_pass_background_params_carry_clip_and_beyond_viewport() {
    let clip = Viewport {
        x: 1.0,
        y: 2.0,
        width: 30.0,
        height: 40.0,
        scale: 1.0,
    };
    let params = background_capture_params(Some(clip.clone()), true);
    assert_eq!(params.capture_beyond_viewport, Some(true));
    assert_eq!(params.clip, Some(clip));
}

#[test]
fn must_fail_decode_capture_data_accepts_base64_and_rejects_garbage() {
    assert_eq!(decode_capture_data("aGk=").unwrap_or_default(), b"hi");
    // Documented: invalid base64 is `A11Y_CDP_AXTREE_FAILED`, naming the cause.
    let error = match decode_capture_data("not base64 !!") {
        Ok(bytes) => panic!("garbage must be refused, decoded {bytes:?}"),
        Err(error) => error,
    };
    assert_eq!(
        error.code(),
        synapse_core::error_codes::A11Y_CDP_AXTREE_FAILED
    );
    assert!(
        error.to_string().contains("invalid base64"),
        "detail must name the cause: {error}"
    );
    assert_eq!(
        decode_capture_data("").unwrap_or_default(),
        Vec::<u8>::new()
    );
}
