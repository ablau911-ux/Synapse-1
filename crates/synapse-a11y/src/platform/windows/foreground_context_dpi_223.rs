//! Regression tests for #223: `foreground_context` must populate
//! `ForegroundContext.dpi_scale` from the window's DPI reader (`120 -> 1.25`,
//! `144 -> 1.5`, `0 -> 1.0`) rather than a hard-coded `1.0`, even when tested
//! on a 96-DPI monitor.

use super::{
    foreground_context, foreground_context_with_dpi_reader, window_dpi_scale_for_hwnd,
    window_owner_chain_tests_217::TestWindow,
};
use crate::A11yError;

#[test]
fn must_pass_foreground_context_propagates_scaled_dpi_from_reader() {
    let window = TestWindow::hidden(None);
    let raw = window.raw();

    for (dpi, expected_scale) in [(120_u32, 1.25_f32), (144, 1.5), (192, 2.0)] {
        let context = match foreground_context_with_dpi_reader(raw, |hwnd| {
            assert_eq!(hwnd.0 as isize as i64, raw);
            dpi
        }) {
            Ok(ctx) => ctx,
            Err(error) => panic!("live window context must succeed for {dpi} DPI: {error}"),
        };
        assert_eq!(context.hwnd, raw);
        assert_eq!(context.pid, std::process::id());
        assert!(
            (context.dpi_scale - expected_scale).abs() < f32::EPSILON,
            "foreground_context at {dpi} DPI must report dpi_scale={expected_scale}, got {}",
            context.dpi_scale
        );
    }

    let live_ctx = match foreground_context(raw) {
        Ok(ctx) => ctx,
        Err(error) => panic!("foreground_context on live window must succeed: {error}"),
    };
    let expected_live = window_dpi_scale_for_hwnd(raw);
    assert!(
        (live_ctx.dpi_scale - expected_live).abs() < f32::EPSILON,
        "foreground_context must match window_dpi_scale_for_hwnd ({expected_live}), got {}",
        live_ctx.dpi_scale
    );
}

#[test]
fn must_fail_foreground_context_rejects_invalid_hwnd_and_falls_back_on_zero_dpi() {
    let err = foreground_context_with_dpi_reader(0, |_| 120);
    assert!(
        matches!(err, Err(A11yError::NoForeground { .. })),
        "invalid hwnd=0 must fail with NoForeground, got {err:?}"
    );

    let window = TestWindow::hidden(None);
    let context = match foreground_context_with_dpi_reader(window.raw(), |_| 0) {
        Ok(ctx) => ctx,
        Err(error) => panic!("live window with 0 DPI readback must fall back to 1.0: {error}"),
    };
    assert!(
        (context.dpi_scale - 1.0).abs() < f32::EPSILON,
        "dpi=0 fallback must report 1.0, got {}",
        context.dpi_scale
    );
}
