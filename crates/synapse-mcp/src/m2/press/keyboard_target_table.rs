//! #2328 frozen behaviour table (manager-owned, work method v3). Codex and Opus
//! never edit this file. One case per L row of the #2328 table: which control
//! receives a background PostMessage key press under a claimed root window.

use super::{KeyboardCandidate, KeyboardTargetPicker};

const ROOT: i64 = 0x10;

fn c(hwnd: i64, score: u8) -> KeyboardCandidate {
    KeyboardCandidate {
        hwnd,
        score,
        class_name: String::new(),
    }
}

#[test]
fn keyboard_target_table_2328() {
    // (row, candidates in enumeration order with the root last, thread focus, expected target)
    let rows: Vec<(&str, Vec<KeyboardCandidate>, Option<i64>, Option<i64>)> = vec![
        (
            "F1_focus_in_bench_input_wins_over_later_tied_scroll_box",
            vec![c(0x100, 100), c(0x200, 100), c(ROOT, 1)],
            Some(0x100),
            Some(0x100),
        ),
        (
            "F2_focus_in_scroll_box_follows_focus",
            vec![c(0x100, 100), c(0x200, 100), c(ROOT, 1)],
            Some(0x200),
            Some(0x200),
        ),
        (
            "F3_focused_child_button_gets_the_keys",
            vec![c(0x100, 100), c(0x300, 1), c(ROOT, 1)],
            Some(0x300),
            Some(0x300),
        ),
        (
            "F4_neighbour_no_focus_tie_keeps_old_pick_last_highest",
            vec![c(0x100, 100), c(0x200, 100), c(ROOT, 1)],
            None,
            Some(0x200),
        ),
        (
            "F5_neighbour_no_focus_single_edit",
            vec![c(0x100, 100), c(ROOT, 1)],
            None,
            Some(0x100),
        ),
        (
            "F6_neighbour_focus_on_root_keeps_old_pick",
            vec![c(0x100, 100), c(ROOT, 1)],
            Some(ROOT),
            Some(0x100),
        ),
        (
            "F7_neighbour_single_window_app_focus_on_root",
            vec![c(ROOT, 1)],
            Some(ROOT),
            Some(ROOT),
        ),
        (
            "F8_someone_elses_window_focus_is_ignored",
            vec![c(0x100, 100), c(ROOT, 1)],
            Some(0x999),
            Some(0x100),
        ),
        (
            "F9_hidden_or_disabled_focused_child_not_listed_is_ignored",
            vec![c(0x100, 100), c(ROOT, 1)],
            Some(0x150),
            Some(0x100),
        ),
        (
            "F10_hostile_focus_hwnd_zero_is_ignored",
            vec![c(0x100, 100), c(ROOT, 1)],
            Some(0),
            Some(0x100),
        ),
        ("F11_empty_no_candidates_no_focus", vec![], None, None),
        (
            "F12_empty_no_candidates_focus_set",
            vec![],
            Some(0x100),
            None,
        ),
        (
            "F13_boundary_focus_on_mid_score_child_among_five",
            vec![
                c(0x100, 100),
                c(0x200, 80),
                c(0x300, 1),
                c(0x400, 40),
                c(ROOT, 1),
            ],
            Some(0x400),
            Some(0x400),
        ),
        (
            "F14_hostile_negative_focus_hwnd_is_ignored",
            vec![c(0x100, 100), c(ROOT, 1)],
            Some(-1),
            Some(0x100),
        ),
    ];
    let mut failures = Vec::new();
    for (row, candidates, focus, expected) in rows {
        let picker = KeyboardTargetPicker::new(ROOT, candidates, focus);
        let got = picker.pick().map(KeyboardCandidate::hwnd);
        if got != expected {
            failures.push(format!("row {row}: target {got:?}, expected {expected:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "#2328 table rows failed:\n{}",
        failures.join("\n")
    );
}
