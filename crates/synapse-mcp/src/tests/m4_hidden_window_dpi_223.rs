//! Regression tests for #223 in `m4`: `hidden_desktop_window_context` must
//! populate `ForegroundContext.dpi_scale` from the per-window DPI helper
//! (`1.25`, `1.5`) instead of a hard-coded `1.0`, and return `None` for an
//! invalid HWND.

use anyhow::{Context, ensure};
use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, WS_EX_NOACTIVATE, WS_POPUP};
use windows::core::w;

use super::{hidden_desktop_window_context, hidden_desktop_window_context_with_dpi_scale};
use crate::test_support::{PumpedWindows, from_hwnd};

/// Never-shown test window on a message-pumping UI thread, so a concurrent
/// UIA query that reaches it is answered; destroyed on drop.
struct HiddenTestWindow {
    ui: PumpedWindows,
}

impl HiddenTestWindow {
    fn open() -> anyhow::Result<Self> {
        let ui = PumpedWindows::spawn(|| {
            let hwnd = unsafe {
                CreateWindowExW(
                    WS_EX_NOACTIVATE,
                    w!("Static"),
                    w!("synapse-223-m4-window"),
                    WS_POPUP,
                    20,
                    30,
                    160,
                    90,
                    None,
                    None,
                    None,
                    None,
                )
            }
            .context("CreateWindowExW")?;
            Ok(vec![from_hwnd(hwnd)])
        })?;
        Ok(Self { ui })
    }

    fn raw(&self) -> i64 {
        self.ui.hwnd(0)
    }
}

#[test]
fn must_pass_m4_hidden_desktop_window_context_propagates_per_window_dpi_scale() -> anyhow::Result<()>
{
    let window = HiddenTestWindow::open()?;
    let raw = window.raw();

    for expected_scale in [1.25_f32, 1.5, 2.0] {
        let ctx = hidden_desktop_window_context_with_dpi_scale(raw, |passed_hwnd| {
            assert_eq!(passed_hwnd, raw);
            expected_scale
        })
        .context("live window must produce ForegroundContext")?;
        ensure!(
            ctx.hwnd == raw
                && ctx.pid == std::process::id()
                && (ctx.dpi_scale - expected_scale).abs() < f32::EPSILON,
            "#223: m4 hidden_desktop_window_context must propagate dpi_scale={expected_scale}, got {ctx:?}"
        );
    }

    let live = hidden_desktop_window_context(raw).context("live window context")?;
    let expected_live = synapse_a11y::window_dpi_scale_for_hwnd(raw);
    ensure!(
        (live.dpi_scale - expected_live).abs() < f32::EPSILON,
        "hidden_desktop_window_context must match window_dpi_scale_for_hwnd ({expected_live}), got {}",
        live.dpi_scale
    );
    Ok(())
}

#[test]
fn must_fail_m4_hidden_desktop_window_context_returns_none_for_invalid_hwnd() -> anyhow::Result<()>
{
    let mut called = false;
    let result = hidden_desktop_window_context_with_dpi_scale(0, |_| {
        called = true;
        1.25
    });
    ensure!(
        result.is_none() && !called,
        "invalid hwnd=0 must return None before querying DPI, got {result:?}"
    );
    Ok(())
}
