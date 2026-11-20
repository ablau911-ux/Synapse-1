//! #371 frozen behaviour table (manager-owned, work method v3). Codex and Opus
//! never edit this file. One case per L row of the #371 table (background
//! PostMessage key delivery to a claimed target without a child edit control).

use super::{PostKeyPlan, postmessage_key_delivery_plan};

const CHAR_ONLY: PostKeyPlan = PostKeyPlan::CharOnly;
const KEYS_NO_CHAR: PostKeyPlan = PostKeyPlan::KeyDownUp { emit_char: false };
const KEYS_WITH_CHAR: PostKeyPlan = PostKeyPlan::KeyDownUp { emit_char: true };

#[test]
fn postmessage_key_delivery_table_371() {
    // (row, resolved target class, single printable key, hold_ms, expected plan)
    let rows: [(&str, &str, bool, u32, PostKeyPlan); 13] = [
        (
            "371a_winit_window_class_letter_hold_50_posts_keydown_keyup",
            "Window Class",
            true,
            50,
            KEYS_NO_CHAR,
        ),
        (
            "371a_winit_window_class_letter_no_hold_posts_keydown_keyup",
            "Window Class",
            true,
            0,
            KEYS_NO_CHAR,
        ),
        (
            "371a_game_canvas_class_digit_posts_keydown_keyup",
            "SDL_app",
            true,
            0,
            KEYS_NO_CHAR,
        ),
        (
            "371a_empty_class_name_posts_keydown_keyup",
            "",
            true,
            0,
            KEYS_NO_CHAR,
        ),
        (
            "371b_neighbour_edit_letter_no_hold_stays_char_only",
            "Edit",
            true,
            0,
            CHAR_ONLY,
        ),
        (
            "371b_neighbour_richedit_letter_no_hold_stays_char_only",
            "RichEditD2DPT",
            true,
            0,
            CHAR_ONLY,
        ),
        (
            "371b_neighbour_scintilla_letter_no_hold_stays_char_only",
            "Scintilla",
            true,
            0,
            CHAR_ONLY,
        ),
        (
            "371b_neighbour_chrome_render_widget_letter_no_hold_stays_char_only",
            "Chrome_RenderWidgetHostHWND",
            true,
            0,
            CHAR_ONLY,
        ),
        (
            "371b_hostile_upper_case_edit_class_stays_char_only",
            "EDIT",
            true,
            0,
            CHAR_ONLY,
        ),
        (
            "371c_edit_letter_hold_100_posts_keydown_keyup_without_char",
            "Edit",
            true,
            100,
            KEYS_NO_CHAR,
        ),
        (
            "371c_boundary_edit_letter_hold_1_posts_keydown_keyup_without_char",
            "Edit",
            true,
            1,
            KEYS_NO_CHAR,
        ),
        (
            "371d_neighbour_winit_named_key_keeps_keydown_char_keyup",
            "Window Class",
            false,
            0,
            KEYS_WITH_CHAR,
        ),
        (
            "371d_neighbour_edit_named_key_with_hold_keeps_keydown_char_keyup",
            "Edit",
            false,
            250,
            KEYS_WITH_CHAR,
        ),
    ];
    let mut failures = Vec::new();
    for (row, class_name, plain_printable, hold_ms, expected) in rows {
        let got = postmessage_key_delivery_plan(class_name, plain_printable, hold_ms);
        if got != expected {
            failures.push(format!("row {row}: plan {got:?}, expected {expected:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "#371 table rows failed:\n{}",
        failures.join("\n")
    );
}
