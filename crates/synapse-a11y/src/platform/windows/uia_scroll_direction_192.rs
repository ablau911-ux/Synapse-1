//! #192: UIA ScrollPattern horizontal direction must match the Win32/CDP convention
//! used by every other act_scroll route: dx > 0 scrolls RIGHT, dy > 0 scrolls UP.

use super::{ScrollAmount, uia_scroll_steps};

#[test]
fn must_pass_positive_dx_scrolls_right_via_small_increment() {
    let steps = uia_scroll_steps(0, 2).expect("dx=2 must plan UIA steps");
    assert_eq!(steps.len(), 2);
    assert!(
        steps
            .iter()
            .all(|s| s.horizontal == ScrollAmount::SmallIncrement)
    );
    assert!(steps.iter().all(|s| s.vertical == ScrollAmount::NoAmount));
}

#[test]
fn must_pass_negative_dx_scrolls_left_via_small_decrement() {
    let steps = uia_scroll_steps(0, -1).expect("dx=-1 must plan UIA steps");
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].horizontal, ScrollAmount::SmallDecrement);
}

#[test]
fn must_pass_vertical_convention_is_unchanged_positive_dy_scrolls_up() {
    let up = uia_scroll_steps(1, 0).expect("dy=1 must plan UIA steps");
    assert_eq!(up[0].vertical, ScrollAmount::SmallDecrement);
    assert_eq!(up[0].horizontal, ScrollAmount::NoAmount);
    let down = uia_scroll_steps(-1, 0).expect("dy=-1 must plan UIA steps");
    assert_eq!(down[0].vertical, ScrollAmount::SmallIncrement);
}

#[test]
fn must_fail_zero_deltas_plan_no_steps() {
    assert!(uia_scroll_steps(0, 0).expect("zero is valid").is_empty());
}
