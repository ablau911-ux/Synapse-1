//! #107: what counts as an observed EFFECT of a click.
//!
//! The click delta signature carries focus fields (focused element, the per-
//! element `focused` flag inside the element hash). Focus can move without the
//! click doing anything, so a focus-only change must never satisfy `verify_delta`
//! for a click. Real effects - element value/enabled/toggle-or-selection state,
//! structure, target-window pixels, CDP/web state - still do.

use super::{
    ActClickParams, ActClickPostcondition, ClickDeltaSignature, Duration, ErrorData, MouseButton,
    Point, SynapseService, Value, click_delta_readback, delivered_no_observed_delta_error,
    foreground_identity_changed, foreground_lost_delta_error, postcondition_observed_delta,
    signature_hash,
};
// Tests on Windows read the foreground through the probe mock in `m2_tools`.
#[cfg(all(test, windows))]
use super::synapse_a11y;

/// Neutralizes the focus-only parts of a signature: the focused-element fields
/// are dropped and the element hash is replaced by its focus-free variant.
fn without_focus(signature: &ClickDeltaSignature) -> ClickDeltaSignature {
    let mut effect = signature.clone();
    effect.focused_element_id = None;
    effect.focused_role = None;
    effect.focused_name_sha256 = None;
    effect.focused_value_sha256 = None;
    effect.focused_bbox = None;
    effect.elements_sha256 = signature.effect_elements_sha256.clone();
    effect
}

/// Pixel evidence is comparable only when both captures succeeded: a capture
/// that failed (or is missing) on either side is no evidence, never a change
/// (#191). Copies `before`'s pixel fields into `after` in that case.
fn neutralize_unusable_pixels(before: &ClickDeltaSignature, after: &mut ClickDeltaSignature) {
    if before.pixel.status != "ok" || after.pixel.status != "ok" {
        after.pixel = before.pixel.clone();
    }
    let usable =
        |pixel: Option<&super::ClickPixelSignature>| pixel.is_some_and(|p| p.status == "ok");
    if !(usable(before.point_pixel.as_ref()) && usable(after.point_pixel.as_ref())) {
        after.point_pixel.clone_from(&before.point_pixel);
    }
}

/// What a target-point pixel comparison proves (#191).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PointPixelEvidence {
    /// Both captures succeeded and the pixels differ.
    Changed,
    /// Both captures succeeded and the pixels are identical.
    Unchanged,
    /// A capture failed or is missing on either side, or the two pictures were
    /// taken at different places (the window moved between the reads): no evidence.
    Unavailable,
}

impl PointPixelEvidence {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Changed => "changed",
            Self::Unchanged => "unchanged",
            Self::Unavailable => "unavailable",
        }
    }
}

pub(super) fn point_pixel_evidence(
    before: Option<&super::ClickPixelSignature>,
    after: Option<&super::ClickPixelSignature>,
) -> PointPixelEvidence {
    match (before, after) {
        (Some(before), Some(after))
            if before.status == "ok" && after.status == "ok" && before.region == after.region =>
        {
            if before.bitmap_sha256 == after.bitmap_sha256 {
                PointPixelEvidence::Unchanged
            } else {
                PointPixelEvidence::Changed
            }
        }
        _ => PointPixelEvidence::Unavailable,
    }
}

/// What a background point click on a claimed window may claim (#2332).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PointClickVerdict {
    /// The pixels in the box around the click point changed.
    PixelVerified,
    /// The window title changed.
    TitleVerified,
    /// The keyboard focus moved into the clicked text control.
    FocusVerified,
    /// Nothing tied to the click was observed.
    NoObservedDelta {
        /// The window changed somewhere away from the click point.
        window_changed_elsewhere: bool,
    },
}

/// The facts read before and after a background point click.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct PointClickFacts {
    title_changed: bool,
    point_pixels: PointPixelEvidence,
    window_pixels_changed: bool,
    clicked_child: Option<i64>,
    clicked_is_text_control: bool,
    focus_before: Option<i64>,
    focus_after: Option<i64>,
}

impl PointClickFacts {
    /// Reads the facts from the before/after pictures, the control under the
    /// click point and the keyboard focus before/after delivery.
    fn new(
        before: &ClickDeltaSignature,
        after: &ClickDeltaSignature,
        (clicked_child, clicked_is_text_control): (Option<i64>, bool),
        focus_before: Option<i64>,
        focus_after: Option<i64>,
    ) -> Self {
        Self {
            title_changed: before.foreground_title_sha256 != after.foreground_title_sha256,
            point_pixels: point_pixel_evidence(
                before.point_pixel.as_ref(),
                after.point_pixel.as_ref(),
            ),
            window_pixels_changed: before.pixel.status == "ok"
                && after.pixel.status == "ok"
                && before.pixel.bitmap_sha256 != after.pixel.bitmap_sha256,
            clicked_child,
            clicked_is_text_control,
            focus_before,
            focus_after,
        }
    }

    /// #2332: the one rule for the verdict of a background point click.
    pub(super) fn verdict(&self) -> PointClickVerdict {
        if self.point_pixels == PointPixelEvidence::Changed {
            PointClickVerdict::PixelVerified
        } else if self.title_changed {
            PointClickVerdict::TitleVerified
        } else if self.clicked_is_text_control
            && let Some(child) = self.clicked_child
            && self.focus_after == Some(child)
            && self.focus_before != Some(child)
        {
            PointClickVerdict::FocusVerified
        } else {
            PointClickVerdict::NoObservedDelta {
                window_changed_elsewhere: self.window_pixels_changed,
            }
        }
    }
}

/// #2332: the check of one claimed background point click. `begin` runs
/// before delivery and keeps the before-picture (with the box around the
/// point) and the keyboard focus inside the target root; `finish` runs after
/// delivery, waits, reads again and returns the postcondition. Only the pixels
/// around the point, the window title, or the focus moving into the clicked
/// text control count; a change elsewhere in the window is reported, never
/// claimed. `focused_hwnd_in_window` never touches the foreground.
pub(super) struct PointClickCheck {
    root_hwnd: i64,
    point: Point,
    button: MouseButton,
    focus_before: Option<i64>,
    before: ClickDeltaSignature,
}

impl PointClickCheck {
    /// Takes the before-reads; `Ok(None)` when the click target is not a point
    /// (the PostMessage route refuses it before any input).
    pub(super) fn begin(
        service: &SynapseService,
        params: &ActClickParams,
        root_hwnd: i64,
    ) -> Result<Option<Self>, ErrorData> {
        let crate::m2::ActClickTarget::Point(target) = &params.target else {
            return Ok(None);
        };
        let point = Point {
            x: target.x,
            y: target.y,
        };
        let focus_before = synapse_a11y::focused_hwnd_in_window(root_hwnd)
            .ok()
            .flatten();
        let before =
            service.capture_raw_input_delta_signature(Some(root_hwnd), Some(point), false)?;
        Ok(Some(Self {
            root_hwnd,
            point,
            button: params.button,
            focus_before,
            before,
        }))
    }

    /// Waits `timeout_ms`, takes the after-reads and returns the verdict as a
    /// postcondition, or the no-observed-delta / foreground-lost error.
    pub(super) async fn finish(
        self,
        service: &SynapseService,
        timeout_ms: u32,
    ) -> Result<ActClickPostcondition, ErrorData> {
        tokio::time::sleep(Duration::from_millis(u64::from(timeout_ms))).await;
        let before = &self.before;
        let after = service.capture_raw_input_delta_signature(
            Some(before.foreground_hwnd),
            Some(self.point),
            false,
        )?;
        let before_hash = signature_hash(before)?;
        let after_hash = signature_hash(&after)?;
        if foreground_identity_changed(before, &after) {
            return Err(foreground_lost_delta_error(
                "act_click",
                "target_point_pixels",
                timeout_ms,
                &before_hash,
                &after_hash,
                before,
                &after,
            ));
        }
        let facts = PointClickFacts::new(
            before,
            &after,
            self.clicked_control(),
            self.focus_before,
            synapse_a11y::focused_hwnd_in_window(self.root_hwnd)
                .ok()
                .flatten(),
        );
        let (source_of_truth, detail) = match facts.verdict() {
            PointClickVerdict::PixelVerified => (
                "target_point_pixels",
                "pixels around the click point changed after delivery",
            ),
            PointClickVerdict::TitleVerified => (
                "target_window_title",
                "the window title changed after delivery",
            ),
            PointClickVerdict::FocusVerified => (
                "target_keyboard_focus",
                "the keyboard focus moved into the clicked text control",
            ),
            PointClickVerdict::NoObservedDelta {
                window_changed_elsewhere,
            } => {
                let os_foreground_hwnd = synapse_a11y::current_foreground_context()
                    .ok()
                    .map(|foreground| foreground.hwnd);
                let mut readback = click_delta_readback(before, &after, os_foreground_hwnd);
                if window_changed_elsewhere && let Value::Object(fields) = &mut readback {
                    fields.insert("window_changed_elsewhere".to_owned(), Value::Bool(true));
                }
                return Err(delivered_no_observed_delta_error(
                    "act_click",
                    "target_point_pixels",
                    timeout_ms,
                    before_hash,
                    after_hash,
                    readback,
                ));
            }
        };
        Ok(postcondition_observed_delta(
            "act_click",
            source_of_truth,
            before_hash,
            after_hash,
            detail,
        ))
    }

    /// The child control under the click point (the one PostMessage delivery
    /// resolves) and whether it is a text control; `(None, false)` when it
    /// cannot be read.
    #[cfg(windows)]
    fn clicked_control(&self) -> (Option<i64>, bool) {
        match crate::m2::hwnd_message_readback_for_root(self.root_hwnd, self.point, self.button) {
            Ok(readback) => (
                Some(readback.hwnd),
                crate::m2::press::is_text_control_class(&readback.class_name),
            ),
            Err(_) => (None, false),
        }
    }

    #[cfg(not(windows))]
    const fn clicked_control(&self) -> (Option<i64>, bool) {
        (None, false)
    }
}

#[cfg(test)]
#[path = "m2_click_effect_table_2332.rs"]
mod table_2332;
#[cfg(test)]
#[path = "m2_click_effect_table_2332c.rs"]
mod table_2332c;

fn focus_moved(before: &ClickDeltaSignature, after: &ClickDeltaSignature) -> bool {
    before.focused_element_id != after.focused_element_id
        || before.focused_bbox != after.focused_bbox
        || before.focused_name_sha256 != after.focused_name_sha256
        || before.focused_value_sha256 != after.focused_value_sha256
}

/// True when the observed state differs, counting pixel evidence only when
/// both captures succeeded (#191).
pub(super) fn observed_state_changed(
    before: &ClickDeltaSignature,
    after: &ClickDeltaSignature,
) -> bool {
    let mut comparable_after = after.clone();
    neutralize_unusable_pixels(before, &mut comparable_after);
    *before != comparable_after
}

/// True when `after` differs from `before` in something other than focus.
pub(super) fn click_effect_changed(
    before: &ClickDeltaSignature,
    after: &ClickDeltaSignature,
) -> bool {
    let effect_before = without_focus(before);
    let mut effect_after = without_focus(after);
    neutralize_unusable_pixels(&effect_before, &mut effect_after);
    // #218: when focus moved but no element changed state, a pixel change can be
    // just the focus ring, default-button border or caret, so pixels cannot
    // prove a click effect here.
    if focus_moved(before, after) && before.effect_elements_sha256 == after.effect_elements_sha256 {
        effect_after.pixel = effect_before.pixel.clone();
        effect_after
            .point_pixel
            .clone_from(&effect_before.point_pixel);
    }
    effect_before != effect_after
}
