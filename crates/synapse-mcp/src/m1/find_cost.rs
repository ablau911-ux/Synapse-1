//! #302: what a perception call must pay for. Pure decisions, pinned by
//! `find_cost_table_302.rs`; the OS work (window capture, process scan) stays in
//! `m1/sources.rs` and `synapse-a11y`.

/// Why the perception input is being built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PerceptionPurpose {
    /// `find` / locator resolution: the reply carries elements only.
    Find,
    /// `observe`: the reply carries capture health and pixel-based facts.
    Observe,
}

/// Whether building the input must capture the target window up front.
/// `pixels_needed_for_detection` is true when the requested mode itself needs
/// a frame (hybrid/pixel detection); `a11y_only` when the mode forbids pixels.
pub(crate) const fn capture_baseline_required(
    purpose: PerceptionPurpose,
    a11y_only: bool,
    pixels_needed_for_detection: bool,
) -> bool {
    !a11y_only && (matches!(purpose, PerceptionPurpose::Observe) || pixels_needed_for_detection)
}

/// Whether the WebView2 debug-port probe (an all-process scan) is needed for a
/// target window.
pub(crate) const fn webview2_probe_required(
    process_is_chromium_family: bool,
    has_chromium_child_window: bool,
) -> bool {
    !process_is_chromium_family && has_chromium_child_window
}

#[cfg(test)]
#[path = "find_cost_table_302.rs"]
mod find_cost_table_302;
