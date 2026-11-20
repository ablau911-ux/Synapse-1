//! #80 (UIA path): the date-segment parent used for `act_type` verification
//! must come from the LIVE UIA tree walker, because the depth-limited
//! observation snapshot omits the spin buttons' parent (live VBV on build
//! c1de0aa5 failed exactly there). These tests drive the real lookup through
//! its injection point and feed its output into the real verifier.
//!
//! NEEDS-SEAM (fixer adds, behaviour unchanged): in `act_type_date_parent.rs`
//!   pub(super) fn focused_date_parent_readback_with(
//!       focused_element_id: &str,
//!       focused_role: &str,
//!       elements: &[AccessibleNode],
//!       live_parent: impl FnOnce()
//!           -> synapse_a11y::A11yResult<Option<synapse_a11y::FocusedParentReadback>>,
//!   ) -> Option<ActTypeDateParentReadback>
//! and `focused_date_parent_readback` delegates to it with
//! `synapse_a11y::focused_spin_button_parent`.

use std::cell::Cell;

use ::synapse_a11y::{A11yError, A11yResult, FocusedParentReadback};
use synapse_core::{AccessibleNode, ElementId};

use super::act_type_date_parent::{ActTypeDateParentReadback, focused_date_parent_readback_with};
use super::*;

const HWND: i64 = 0x1000;
const PARENT: u32 = 100;

fn id(runtime: u32) -> String {
    format!("0x{HWND:x}:{runtime:08x}")
}

fn node(runtime: u32, parent: Option<u32>, role: &str, value: Option<&str>) -> AccessibleNode {
    AccessibleNode {
        element_id: ElementId::parse(&id(runtime)).expect("synthetic id"),
        parent: parent.map(|parent| ElementId::parse(&id(parent)).expect("synthetic parent")),
        name: String::new(),
        role: role.to_owned(),
        automation_id: None,
        value: value.map(str::to_owned),
        bbox: Rect {
            x: 10,
            y: 10,
            w: 20,
            h: 20,
        },
        enabled: true,
        focused: false,
        patterns: Vec::new(),
        children_count: 0,
        depth: 6,
        control_state: None,
    }
}

/// The snapshot the live VBV saw: the focused spin button, no parent row.
fn parentless_snapshot(focused: u32) -> Vec<AccessibleNode> {
    vec![node(focused, None, "spin button", Some("1"))]
}

fn live(value: Option<&str>, segments: &[&str]) -> FocusedParentReadback {
    FocusedParentReadback {
        parent_id: id(PARENT),
        parent_value: value.map(str::to_owned),
        child_spin_values: segments.iter().map(|s| (*s).to_owned()).collect(),
    }
}

fn lookup(
    focused: u32,
    role: &str,
    elements: &[AccessibleNode],
    result: A11yResult<Option<FocusedParentReadback>>,
    calls: &Cell<u32>,
) -> Option<ActTypeDateParentReadback> {
    focused_date_parent_readback_with(&id(focused), role, elements, || {
        calls.set(calls.get() + 1);
        result
    })
}

#[test]
fn must_pass_live_parent_is_used_when_the_snapshot_has_no_parent() {
    let calls = Cell::new(0);
    let parent = lookup(
        3,
        "spin button",
        &parentless_snapshot(3),
        Ok(Some(live(Some("09/12/2026"), &["09", "12", "2026"]))),
        &calls,
    );
    assert_eq!(calls.get(), 1, "the live tree walker is queried once");
    assert_eq!(
        parent,
        Some(ActTypeDateParentReadback {
            parent_id: id(PARENT),
            parent_value: Some("09/12/2026".to_owned()),
            segment_values: vec!["09".to_owned(), "12".to_owned(), "2026".to_owned()],
        })
    );
}

fn text_readback(
    focused: u32,
    date_parent: Option<ActTypeDateParentReadback>,
) -> ActTypeTextReadback {
    ActTypeTextReadback {
        signature: ActTypeTextSignature {
            foreground_hwnd: HWND,
            foreground_pid: 77,
            foreground_process: "webview-test.exe".to_owned(),
            focused_element_id: Some(id(focused)),
            focused_role: Some("spin button".to_owned()),
            readback_source: Some(ACT_TYPE_TEXT_SOURCE_UIA_VALUE.to_owned()),
            has_text_readback: true,
            ..Default::default()
        },
        value: Some("1".to_owned()),
        browser_url: None,
        date_parent,
    }
}

fn response() -> ActTypeResponse {
    ActTypeResponse {
        ok: true,
        chars_typed: 8,
        elapsed_ms: 10,
        backend_tier_used: "foreground".to_owned(),
        required_foreground: true,
        target_text_integrity: "dispatch_only_requires_target_readback".to_owned(),
        target_readback_required: true,
        minimum_linear_ms_per_char: 20,
        postcondition: crate::m2::postcondition::postcondition_not_requested(
            "act_type",
            "foreground_focused_ui_or_pixels",
        ),
    }
}

/// The original false failure: focus moved segment 1 -> segment 3, the
/// snapshot had no parent. With the live parent the type verifies.
#[test]
fn must_pass_live_parent_readbacks_verify_the_typed_date() {
    let calls = Cell::new(0);
    let before_parent = lookup(
        1,
        "spin button",
        &parentless_snapshot(1),
        Ok(Some(live(Some("mm/dd/yyyy"), &["mm", "dd", "yyyy"]))),
        &calls,
    );
    let after_parent = lookup(
        3,
        "spin button",
        &parentless_snapshot(3),
        Ok(Some(live(Some("09/12/2026"), &["09", "12", "2026"]))),
        &calls,
    );
    let verified = verify_act_type_text_response(
        response(),
        text_readback(1, before_parent),
        text_readback(3, after_parent),
        "before".to_owned(),
        "after".to_owned(),
        250,
        "09122026",
    )
    .expect("same live parent with the typed date must verify");
    assert_eq!(verified.postcondition.status, "observed_delta");
    assert!(!verified.target_readback_required);
    assert_eq!(
        verified.target_text_integrity,
        "verify_delta_text_readback:uia_date_parent"
    );
}

#[test]
fn must_fail_non_spin_focus_never_queries_the_live_tree() {
    let calls = Cell::new(0);
    let elements = vec![node(5, Some(PARENT), "edit", Some("x"))];
    let parent = lookup(5, "edit", &elements, Ok(Some(live(None, &[]))), &calls);
    assert_eq!(parent, None);
    assert_eq!(calls.get(), 0, "no UIA walk for a non-date focus");
}

#[test]
fn must_fail_live_error_without_snapshot_parent_yields_no_parent() {
    let calls = Cell::new(0);
    let error = A11yError::NotAvailable {
        detail: "synthetic UIA failure".to_owned(),
    };
    let parent = lookup(
        3,
        "spin button",
        &parentless_snapshot(3),
        Err(error),
        &calls,
    );
    assert_eq!(calls.get(), 1);
    assert_eq!(parent, None, "no parent may be invented on a UIA error");
}

#[test]
fn must_pass_live_none_falls_back_to_the_snapshot_parent() {
    let calls = Cell::new(0);
    let elements = vec![
        node(PARENT, None, "group", Some("09/12/2026")),
        node(1, Some(PARENT), "spin button", Some("09")),
        node(2, Some(PARENT), "spin button", Some("12")),
        node(3, Some(PARENT), "spin button", Some("2026")),
    ];
    let parent = lookup(3, "spin button", &elements, Ok(None), &calls);
    assert_eq!(calls.get(), 1);
    assert_eq!(
        parent,
        Some(ActTypeDateParentReadback {
            parent_id: id(PARENT),
            parent_value: Some("09/12/2026".to_owned()),
            segment_values: vec!["09".to_owned(), "12".to_owned(), "2026".to_owned()],
        })
    );
}

#[test]
fn must_fail_blank_live_parent_value_is_not_a_date_source() {
    let calls = Cell::new(0);
    let parent = lookup(
        3,
        "spin button",
        &parentless_snapshot(3),
        Ok(Some(live(Some("   "), &[]))),
        &calls,
    )
    .expect("live parent exists");
    assert_eq!(parent.parent_value, None);
    assert!(parent.segment_values.is_empty());
}

/// Wiring: the production lookup delegates to the seam with the real UIA tree
/// walker, and the act_type text capture calls the production lookup.
#[test]
fn must_pass_capture_uses_the_live_tree_walker_lookup() {
    // Checkouts with core.autocrlf=true embed CRLF; normalize so the function-end
    // probe below works on any line ending.
    let module = include_str!("../act_type_date_parent.rs").replace("\r\n", "\n");
    let start = module
        .find("pub(super) fn focused_date_parent_readback(")
        .expect("production lookup present");
    let body = &module[start..];
    let body = &body[..body.find("\n}\n").expect("function end")];
    assert!(
        body.contains("focused_date_parent_readback_with("),
        "{body}"
    );
    assert!(
        body.contains("synapse_a11y::focused_spin_button_parent"),
        "{body}"
    );
    let tools = include_str!("../m2_tools.rs");
    assert!(tools.contains("act_type_date_parent::focused_date_parent_readback("));
}
