//! #2332 frozen behaviour table (manager-owned, work method v3). Codex and Opus
//! never edit this file. One case per L row: what a background point click on a
//! claimed window may claim (roadmap Phase 3, #107; prover rows 107p and 107q).

use super::{PointClickFacts, PointClickVerdict, PointPixelEvidence};

const EDIT: i64 = 0x100;
const OTHER: i64 = 0x200;
const BUTTON: i64 = 0x300;

const QUIET: PointClickFacts = PointClickFacts {
    title_changed: false,
    point_pixels: PointPixelEvidence::Unchanged,
    window_pixels_changed: false,
    clicked_child: Some(EDIT),
    clicked_is_text_control: true,
    focus_before: None,
    focus_after: None,
};

const NOTHING: PointClickVerdict = PointClickVerdict::NoObservedDelta {
    window_changed_elsewhere: false,
};
const ELSEWHERE: PointClickVerdict = PointClickVerdict::NoObservedDelta {
    window_changed_elsewhere: true,
};

#[test]
fn point_click_verdict_table_2332() {
    let rows: [(&str, PointClickFacts, PointClickVerdict); 14] = [
        (
            "V1_normal_pixels_at_the_point_changed",
            PointClickFacts {
                point_pixels: PointPixelEvidence::Changed,
                window_pixels_changed: true,
                ..QUIET
            },
            PointClickVerdict::PixelVerified,
        ),
        (
            "V2_107p_dead_area_window_changed_elsewhere_is_not_verified",
            PointClickFacts {
                window_pixels_changed: true,
                clicked_child: None,
                clicked_is_text_control: false,
                ..QUIET
            },
            ELSEWHERE,
        ),
        (
            "V3_107q_focus_moved_into_the_clicked_text_control",
            PointClickFacts {
                focus_after: Some(EDIT),
                ..QUIET
            },
            PointClickVerdict::FocusVerified,
        ),
        (
            "V3_focus_moved_from_another_control_into_the_clicked_text_control",
            PointClickFacts {
                focus_before: Some(OTHER),
                focus_after: Some(EDIT),
                ..QUIET
            },
            PointClickVerdict::FocusVerified,
        ),
        (
            "V4_focus_moved_to_a_control_that_was_not_clicked",
            PointClickFacts {
                focus_after: Some(OTHER),
                ..QUIET
            },
            NOTHING,
        ),
        (
            "V5_neighbour_focus_alone_on_a_clicked_button_is_not_an_effect",
            PointClickFacts {
                clicked_child: Some(BUTTON),
                clicked_is_text_control: false,
                focus_after: Some(BUTTON),
                ..QUIET
            },
            NOTHING,
        ),
        (
            "V6_boundary_focus_was_already_in_the_clicked_text_control",
            PointClickFacts {
                focus_before: Some(EDIT),
                focus_after: Some(EDIT),
                ..QUIET
            },
            NOTHING,
        ),
        (
            "V7_title_changed_without_pixels",
            PointClickFacts {
                title_changed: true,
                ..QUIET
            },
            PointClickVerdict::TitleVerified,
        ),
        (
            "V8_point_capture_failed_window_change_is_no_proof",
            PointClickFacts {
                point_pixels: PointPixelEvidence::Unavailable,
                window_pixels_changed: true,
                ..QUIET
            },
            ELSEWHERE,
        ),
        (
            "V8_point_capture_failed_focus_into_clicked_text_control_still_counts",
            PointClickFacts {
                point_pixels: PointPixelEvidence::Unavailable,
                focus_after: Some(EDIT),
                ..QUIET
            },
            PointClickVerdict::FocusVerified,
        ),
        ("V9_empty_nothing_changed", QUIET, NOTHING),
        (
            "V10_hostile_clicked_child_unknown_focus_moved_somewhere",
            PointClickFacts {
                clicked_child: None,
                focus_after: Some(EDIT),
                ..QUIET
            },
            NOTHING,
        ),
        (
            "V11_pixels_at_the_point_win_over_focus",
            PointClickFacts {
                point_pixels: PointPixelEvidence::Changed,
                focus_after: Some(EDIT),
                ..QUIET
            },
            PointClickVerdict::PixelVerified,
        ),
        (
            "V12_someone_elses_window_focus_outside_the_target_reads_as_none",
            PointClickFacts {
                focus_before: Some(EDIT),
                focus_after: None,
                window_pixels_changed: true,
                ..QUIET
            },
            ELSEWHERE,
        ),
    ];
    let mut failures = Vec::new();
    for (row, facts, expected) in rows {
        let got = facts.verdict();
        if got != expected {
            failures.push(format!("row {row}: verdict {got:?}, expected {expected:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "#2332 verdict rows failed:\n{}",
        failures.join("\n")
    );
}
