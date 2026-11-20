//! FROZEN behaviour table of Gitea #2340 part B (roadmap item #569), rows
//! K01-K19. Written by the manager from the issue table before the code.
//! Coders never edit this file; a wrong row is reported to the manager.

use super::{KeyChord, KeyLane, KeyRoute};

const EDIT: &str = "Edit";
const RICH: &str = "RichEdit20W";
const SCINTILLA: &str = "Scintilla";
const CHROME: &str = "Chrome_RenderWidgetHostHWND";
const FIREFOX: &str = "MozillaWindowClass";
const GAME: &str = "UnityWndClass";

const FRONT: bool = true;
const BACK: bool = false;

#[test]
fn key_route_table_569b() {
    use KeyLane::{
        ClaimedKeyPress, EditMessage, RefusedNeedsFront, RefusedSystemChord, WindowMessages,
    };
    let rows: Vec<(&str, &[&str], &str, bool, KeyLane)> = vec![
        ("K01_plain_letter", &["a"], EDIT, BACK, WindowMessages),
        ("K02_enter", &["enter"], CHROME, BACK, WindowMessages),
        ("K03_function_key", &["f5"], FIREFOX, BACK, WindowMessages),
        (
            "K03_plain_key_on_a_game_window",
            &["w"],
            GAME,
            BACK,
            WindowMessages,
        ),
        (
            "K04_shift_letter_unchanged",
            &["shift", "a"],
            EDIT,
            BACK,
            WindowMessages,
        ),
        (
            "K05_select_all_on_an_edit_box",
            &["ctrl", "a"],
            EDIT,
            BACK,
            EditMessage,
        ),
        (
            "K06_copy_on_a_rich_edit_box",
            &["ctrl", "c"],
            RICH,
            BACK,
            EditMessage,
        ),
        (
            "K06_cut_on_a_rich_edit_box",
            &["ctrl", "x"],
            RICH,
            BACK,
            EditMessage,
        ),
        (
            "K06_paste_on_a_rich_edit_box",
            &["ctrl", "v"],
            RICH,
            BACK,
            EditMessage,
        ),
        (
            "K06_undo_on_a_rich_edit_box",
            &["ctrl", "z"],
            RICH,
            BACK,
            EditMessage,
        ),
        (
            "K07_paste_on_scintilla",
            &["ctrl", "v"],
            SCINTILLA,
            BACK,
            EditMessage,
        ),
        (
            "K08_redo_ctrl_y_on_a_rich_edit_box",
            &["ctrl", "y"],
            RICH,
            BACK,
            EditMessage,
        ),
        (
            "K08_redo_ctrl_shift_z_on_a_rich_edit_box",
            &["ctrl", "shift", "z"],
            RICH,
            BACK,
            EditMessage,
        ),
        (
            "K09_redo_on_a_plain_edit_box_in_the_back",
            &["ctrl", "y"],
            EDIT,
            BACK,
            RefusedNeedsFront,
        ),
        (
            "K09_redo_on_a_plain_edit_box_in_front",
            &["ctrl", "y"],
            EDIT,
            FRONT,
            ClaimedKeyPress,
        ),
        (
            "K10_vs03_save_in_front",
            &["ctrl", "s"],
            CHROME,
            FRONT,
            ClaimedKeyPress,
        ),
        (
            "K11_save_in_the_back",
            &["ctrl", "s"],
            CHROME,
            BACK,
            RefusedNeedsFront,
        ),
        (
            "K12_vs05_palette_in_front",
            &["ctrl", "shift", "p"],
            CHROME,
            FRONT,
            ClaimedKeyPress,
        ),
        (
            "K13_vs12_alt_f4_in_front",
            &["alt", "f4"],
            CHROME,
            FRONT,
            ClaimedKeyPress,
        ),
        (
            "K13_alt_f4_in_the_back",
            &["alt", "f4"],
            CHROME,
            BACK,
            RefusedNeedsFront,
        ),
        (
            "K14_ff11_ctrl_tab_in_the_back",
            &["ctrl", "tab"],
            FIREFOX,
            BACK,
            RefusedNeedsFront,
        ),
        (
            "K14_ctrl_tab_in_front",
            &["ctrl", "tab"],
            FIREFOX,
            FRONT,
            ClaimedKeyPress,
        ),
        (
            "K15_paste_on_a_chrome_window_in_the_back",
            &["ctrl", "v"],
            CHROME,
            BACK,
            RefusedNeedsFront,
        ),
        (
            "K15_paste_on_a_chrome_window_in_front",
            &["ctrl", "v"],
            CHROME,
            FRONT,
            ClaimedKeyPress,
        ),
        (
            "K16_save_on_an_edit_box_in_the_back",
            &["ctrl", "s"],
            EDIT,
            BACK,
            RefusedNeedsFront,
        ),
        (
            "K17_win_key_alone",
            &["super"],
            EDIT,
            FRONT,
            RefusedSystemChord,
        ),
        (
            "K17_win_d_in_front",
            &["super", "d"],
            CHROME,
            FRONT,
            RefusedSystemChord,
        ),
        (
            "K17_win_l_in_the_back",
            &["super", "l"],
            CHROME,
            BACK,
            RefusedSystemChord,
        ),
        (
            "K17_alt_tab",
            &["alt", "tab"],
            EDIT,
            FRONT,
            RefusedSystemChord,
        ),
        (
            "K17_alt_esc",
            &["alt", "esc"],
            EDIT,
            FRONT,
            RefusedSystemChord,
        ),
        (
            "K17_ctrl_esc",
            &["ctrl", "esc"],
            EDIT,
            FRONT,
            RefusedSystemChord,
        ),
        (
            "K17_ctrl_alt_delete",
            &["ctrl", "alt", "delete"],
            EDIT,
            FRONT,
            RefusedSystemChord,
        ),
        (
            "K17_ctrl_shift_esc_in_the_back",
            &["ctrl", "shift", "esc"],
            CHROME,
            BACK,
            RefusedSystemChord,
        ),
        (
            "K18_alt_shift_tab",
            &["alt", "shift", "tab"],
            EDIT,
            FRONT,
            RefusedSystemChord,
        ),
        (
            "K19_two_held_modifiers_in_front",
            &["ctrl", "alt", "s"],
            EDIT,
            FRONT,
            ClaimedKeyPress,
        ),
        (
            "K19_two_held_modifiers_in_the_back",
            &["ctrl", "alt", "s"],
            EDIT,
            BACK,
            RefusedNeedsFront,
        ),
    ];

    let mut failures = Vec::new();
    for (row, labels, class, claimed_and_in_front, expected) in &rows {
        let labels: Vec<String> = labels.iter().map(|label| (*label).to_owned()).collect();
        let observed =
            KeyRoute::for_window(&KeyChord::of(&labels), class, *claimed_and_in_front).lane();
        if observed != *expected {
            failures.push(format!(
                "{row}: expected {expected:?}, observed {observed:?}"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} rows failed:\n{}",
        failures.len(),
        rows.len(),
        failures.join("\n")
    );
}
