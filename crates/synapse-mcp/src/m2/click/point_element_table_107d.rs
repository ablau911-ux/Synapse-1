//! FROZEN behaviour table of #107 part (d) (roadmap item #568), rows P1-P8 and
//! C1-C8 of the issue table of 2026-11-19 21:40. Written before the code.
//! Coders never edit this file; a wrong row is reported to the manager.

use synapse_core::{AccessibleNode, ElementId, MouseButton, Point, Rect, UiaPattern, element_id};

use super::{PointClickPlan, PointElement, PointElementPicker};

const HWND: i64 = 0x1234;

// Runtime ids (hex) of the nodes.
const WINDOW: &str = "f0";
const GROUP: &str = "f1";
const PANE: &str = "fa";
const BUTTON: &str = "b0";
const BIG: &str = "b1";
const SMALL: &str = "b2";
const LEFT: &str = "ba";
const RIGHT_NEIGHBOUR: &str = "bb";
const BELOW: &str = "bc";
const LIST_ITEM: &str = "d0";
const TEXT: &str = "c0";
const OFFSCREEN: &str = "e0";
const ZERO_WIDE: &str = "e1";

const ENABLED: bool = true;
const DISABLED: bool = false;
const NATIVE_BUTTON: bool = true;
const NO_NATIVE_BUTTON: bool = false;

const NONE: &[UiaPattern] = &[];
const INVOKE: &[UiaPattern] = &[UiaPattern::Invoke];
const SELECTION_ITEM: &[UiaPattern] = &[UiaPattern::SelectionItem];

fn id(runtime_id_hex: &str) -> ElementId {
    element_id(HWND, runtime_id_hex)
}

/// One node of the claimed window's accessibility tree.
fn node(
    runtime_id_hex: &str,
    parent: Option<&str>,
    depth: u32,
    (x, y, w, h): (i32, i32, i32, i32),
    enabled: bool,
    patterns: &[UiaPattern],
) -> AccessibleNode {
    AccessibleNode {
        element_id: id(runtime_id_hex),
        parent: parent.map(id),
        name: runtime_id_hex.to_owned(),
        role: String::new(),
        automation_id: None,
        value: None,
        bbox: Rect { x, y, w, h },
        enabled,
        focused: false,
        patterns: patterns.to_vec(),
        children_count: 0,
        depth,
        control_state: None,
    }
}

/// The window node every tree starts with (depth 0, no click pattern).
fn window() -> AccessibleNode {
    node(WINDOW, None, 0, (0, 0, 400, 300), ENABLED, NONE)
}

/// The control at the point, as `PointClickPlan::choose` receives it.
fn element(framework: &str, pattern: UiaPattern) -> PointElement {
    PointElement::new(id(BUTTON), framework.to_owned(), vec![pattern])
}

const fn at(x: i32, y: i32) -> Point {
    Point { x, y }
}

#[test]
fn point_element_table_107d() {
    use MouseButton::{Left, Right};
    use PointClickPlan::{BmClick, PostedMessages, UiaPattern as Uia};
    use UiaPattern::{ExpandCollapse, Invoke, SelectionItem, Toggle};

    let pick_rows: Vec<(&str, Vec<AccessibleNode>, Point, Option<&str>)> = vec![
        (
            "P1_button_inside_its_pane",
            vec![
                window(),
                node(GROUP, Some(WINDOW), 1, (0, 0, 400, 300), ENABLED, NONE),
                node(PANE, Some(GROUP), 2, (50, 50, 300, 200), ENABLED, NONE),
                node(BUTTON, Some(PANE), 3, (100, 100, 80, 30), ENABLED, INVOKE),
            ],
            at(120, 110),
            Some(BUTTON),
        ),
        (
            "P2_same_depth_smaller_box_listed_second",
            vec![
                window(),
                node(PANE, Some(WINDOW), 1, (0, 0, 400, 300), ENABLED, NONE),
                node(BIG, Some(PANE), 2, (100, 100, 200, 100), ENABLED, INVOKE),
                node(SMALL, Some(PANE), 2, (150, 120, 40, 20), ENABLED, INVOKE),
            ],
            at(160, 125),
            Some(SMALL),
        ),
        (
            "P2_same_depth_smaller_box_listed_first",
            vec![
                window(),
                node(PANE, Some(WINDOW), 1, (0, 0, 400, 300), ENABLED, NONE),
                node(SMALL, Some(PANE), 2, (150, 120, 40, 20), ENABLED, INVOKE),
                node(BIG, Some(PANE), 2, (100, 100, 200, 100), ENABLED, INVOKE),
            ],
            at(160, 125),
            Some(SMALL),
        ),
        (
            "P3_text_inside_a_list_item_gives_the_list_item",
            vec![
                window(),
                node(PANE, Some(WINDOW), 1, (0, 0, 400, 300), ENABLED, NONE),
                node(
                    LIST_ITEM,
                    Some(PANE),
                    2,
                    (100, 100, 200, 20),
                    ENABLED,
                    SELECTION_ITEM,
                ),
                node(TEXT, Some(LIST_ITEM), 3, (104, 102, 60, 16), ENABLED, NONE),
            ],
            at(110, 108),
            Some(LIST_ITEM),
        ),
        (
            "P4_disabled_button_inside_a_clickable_pane",
            vec![
                window(),
                node(PANE, Some(WINDOW), 1, (0, 0, 400, 300), ENABLED, INVOKE),
                node(BUTTON, Some(PANE), 2, (100, 100, 80, 30), DISABLED, INVOKE),
            ],
            at(120, 110),
            None,
        ),
        (
            "P5_offscreen_node_with_a_zero_size_box_is_skipped",
            vec![
                window(),
                node(PANE, Some(WINDOW), 1, (0, 0, 400, 300), ENABLED, NONE),
                node(BUTTON, Some(PANE), 2, (100, 100, 80, 30), ENABLED, INVOKE),
                node(
                    OFFSCREEN,
                    Some(BUTTON),
                    3,
                    (100, 100, 0, 0),
                    ENABLED,
                    INVOKE,
                ),
            ],
            at(100, 100),
            Some(BUTTON),
        ),
        (
            "P5_node_with_zero_width_is_skipped",
            vec![
                window(),
                node(PANE, Some(WINDOW), 1, (0, 0, 400, 300), ENABLED, NONE),
                node(BUTTON, Some(PANE), 2, (100, 100, 80, 30), ENABLED, INVOKE),
                node(
                    ZERO_WIDE,
                    Some(BUTTON),
                    3,
                    (110, 100, 0, 30),
                    ENABLED,
                    INVOKE,
                ),
            ],
            at(110, 110),
            Some(BUTTON),
        ),
        (
            "P6_point_outside_every_box",
            vec![
                window(),
                node(BUTTON, Some(WINDOW), 1, (100, 100, 80, 30), ENABLED, INVOKE),
            ],
            at(500, 500),
            None,
        ),
        ("P6_empty_node_list", Vec::new(), at(120, 110), None),
        (
            "P7_right_edge_belongs_to_the_neighbour",
            vec![
                window(),
                node(LEFT, Some(WINDOW), 1, (100, 100, 50, 30), ENABLED, INVOKE),
                node(
                    RIGHT_NEIGHBOUR,
                    Some(WINDOW),
                    1,
                    (150, 100, 50, 30),
                    ENABLED,
                    INVOKE,
                ),
            ],
            at(150, 110),
            Some(RIGHT_NEIGHBOUR),
        ),
        (
            "P7_bottom_edge_belongs_to_the_node_below",
            vec![
                window(),
                node(LEFT, Some(WINDOW), 1, (100, 100, 50, 30), ENABLED, INVOKE),
                node(BELOW, Some(WINDOW), 1, (100, 130, 50, 30), ENABLED, INVOKE),
            ],
            at(110, 130),
            Some(BELOW),
        ),
        (
            "P7_last_pixel_inside_is_that_node",
            vec![
                window(),
                node(LEFT, Some(WINDOW), 1, (100, 100, 50, 30), ENABLED, INVOKE),
                node(
                    RIGHT_NEIGHBOUR,
                    Some(WINDOW),
                    1,
                    (150, 100, 50, 30),
                    ENABLED,
                    INVOKE,
                ),
                node(BELOW, Some(WINDOW), 1, (100, 130, 50, 30), ENABLED, INVOKE),
            ],
            at(149, 129),
            Some(LEFT),
        ),
        (
            "P8_only_the_window_node_holds_the_point",
            vec![window()],
            at(10, 10),
            None,
        ),
    ];

    let plan_rows: Vec<(
        &str,
        MouseButton,
        u8,
        bool,
        Option<PointElement>,
        PointClickPlan,
    )> = vec![
        (
            "C1_classic_button_with_a_wpf_element",
            Left,
            1,
            NATIVE_BUTTON,
            Some(element("WPF", Invoke)),
            BmClick,
        ),
        (
            "C1_classic_button_without_element",
            Left,
            1,
            NATIVE_BUTTON,
            None,
            BmClick,
        ),
        (
            "C2_wpf_invoke",
            Left,
            1,
            NO_NATIVE_BUTTON,
            Some(element("WPF", Invoke)),
            Uia(element("WPF", Invoke)),
        ),
        (
            "C3_xaml_selection_item",
            Left,
            1,
            NO_NATIVE_BUTTON,
            Some(element("XAML", SelectionItem)),
            Uia(element("XAML", SelectionItem)),
        ),
        (
            "C3_xaml_toggle",
            Left,
            1,
            NO_NATIVE_BUTTON,
            Some(element("XAML", Toggle)),
            Uia(element("XAML", Toggle)),
        ),
        (
            "C3_xaml_expand_collapse",
            Left,
            1,
            NO_NATIVE_BUTTON,
            Some(element("XAML", ExpandCollapse)),
            Uia(element("XAML", ExpandCollapse)),
        ),
        (
            "C4_winforms_invoke_unchanged",
            Left,
            1,
            NO_NATIVE_BUTTON,
            Some(element("WinForm", Invoke)),
            PostedMessages,
        ),
        (
            "C4_win32_invoke_unchanged",
            Left,
            1,
            NO_NATIVE_BUTTON,
            Some(element("Win32", Invoke)),
            PostedMessages,
        ),
        (
            "C5_no_element_at_the_point",
            Left,
            1,
            NO_NATIVE_BUTTON,
            None,
            PostedMessages,
        ),
        (
            "C6_double_click_on_wpf_unchanged",
            Left,
            2,
            NO_NATIVE_BUTTON,
            Some(element("WPF", Invoke)),
            PostedMessages,
        ),
        (
            "C6_right_click_on_wpf_unchanged",
            Right,
            1,
            NO_NATIVE_BUTTON,
            Some(element("WPF", Invoke)),
            PostedMessages,
        ),
        (
            "C7_qt_invoke_unchanged",
            Left,
            1,
            NO_NATIVE_BUTTON,
            Some(element("Qt", Invoke)),
            PostedMessages,
        ),
        (
            "C7_empty_framework_invoke_unchanged",
            Left,
            1,
            NO_NATIVE_BUTTON,
            Some(element("", Invoke)),
            PostedMessages,
        ),
        (
            "C8_lower_case_wpf",
            Left,
            1,
            NO_NATIVE_BUTTON,
            Some(element("wpf", Invoke)),
            Uia(element("wpf", Invoke)),
        ),
    ];

    let total = pick_rows.len() + plan_rows.len();
    let mut failures = Vec::new();
    for (row, nodes, point, expected) in pick_rows {
        let picker = PointElementPicker::new(nodes);
        let observed = picker.pick(point).map(|picked| picked.element_id.clone());
        let expected = expected.map(id);
        if observed != expected {
            failures.push(format!(
                "{row}: expected {expected:?}, observed {observed:?}"
            ));
        }
    }
    for (row, button, clicks, hit_is_native_button, element_at_point, expected) in plan_rows {
        let observed =
            PointClickPlan::choose(button, clicks, hit_is_native_button, || element_at_point);
        if observed != expected {
            failures.push(format!(
                "{row}: expected {expected:?}, observed {observed:?}"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} rows failed:\n{}",
        failures.len(),
        total,
        failures.join("\n")
    );
}
