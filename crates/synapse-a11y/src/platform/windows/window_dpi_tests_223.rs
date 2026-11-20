//! Regression tests for #223: `foreground_context` must report the real
//! per-window DPI scale, not a hard-coded 1.0.

use super::dpi_scale_from_dpi;

#[test]
fn must_pass_a_120_dpi_window_reports_a_1_25_scale() {
    assert!((dpi_scale_from_dpi(120) - 1.25).abs() < f32::EPSILON);
}

#[test]
fn must_pass_standard_and_high_dpi_values_map_to_dpi_over_96() {
    assert!((dpi_scale_from_dpi(96) - 1.0).abs() < f32::EPSILON);
    assert!((dpi_scale_from_dpi(144) - 1.5).abs() < f32::EPSILON);
    assert!((dpi_scale_from_dpi(192) - 2.0).abs() < f32::EPSILON);
}

#[test]
fn must_fail_an_invalid_window_dpi_of_zero_falls_back_to_unscaled() {
    assert!((dpi_scale_from_dpi(0) - 1.0).abs() < f32::EPSILON);
}

#[test]
fn must_fail_the_public_hwnd_helper_reports_unscaled_for_an_invalid_window() {
    // Used by the hidden-desktop launch context (m4) instead of a hard-coded 1.0.
    assert!((super::window_dpi_scale_for_hwnd(0) - 1.0).abs() < f32::EPSILON);
}

/// The public helper must read the REAL window DPI: a live window created under
/// per-monitor-v2 awareness reports `GetDpiForWindow / 96`. (On a 96-DPI
/// monitor this equals 1.0 and cannot tell a hard-coded 1.0 apart; the check
/// bites on any scaled monitor.)
#[test]
fn must_pass_the_public_hwnd_helper_reads_the_live_window_dpi() {
    use windows::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetThreadDpiAwarenessContext,
    };

    use super::window_owner_chain_tests_217::TestWindow;

    // SAFETY: switches only this test thread's awareness; restored below.
    let previous =
        unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let window = TestWindow::hidden(None);
    // SAFETY: reads DPI of the live window owned by `window`.
    let dpi = unsafe { GetDpiForWindow(window.hwnd) };
    let scale = super::window_dpi_scale_for_hwnd(window.raw());
    drop(window);
    // SAFETY: restores the context this thread had before the test (a null
    // previous context is rejected by the API without side effects).
    let _restored = unsafe { SetThreadDpiAwarenessContext(previous) };

    assert!(dpi > 0, "a live window has a DPI");
    #[allow(clippy::cast_precision_loss)]
    let expected = dpi as f32 / 96.0;
    assert!(
        (scale - expected).abs() < f32::EPSILON,
        "window at {dpi} DPI must report {expected}, got {scale}"
    );
}
