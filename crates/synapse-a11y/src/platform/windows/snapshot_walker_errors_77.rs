//! #77: a UIA tree-walker error is only "no more children / siblings" when it is
//! the genuine end-of-list signal. Provider/COM failures must not be read as an
//! empty subtree: they leave coverage unknown and mark the snapshot truncated.

use uiautomation::errors::{ERR_NOTFOUND, ERR_TIMEOUT};

use super::{has_children_after_probe, walker_reached_end};

fn err(code: i32) -> uiautomation::Error {
    uiautomation::Error::new(code, "test")
}

const UIA_E_ELEMENTNOTAVAILABLE: i32 = i32::from_ne_bytes(0x8004_0201_u32.to_ne_bytes());
const E_FAIL: i32 = i32::from_ne_bytes(0x8000_4005_u32.to_ne_bytes());

#[test]
fn must_pass_not_found_and_the_null_result_are_the_true_end_of_the_list() {
    assert!(walker_reached_end(&err(ERR_NOTFOUND)));
    // windows-rs reports a NULL element out-param (no first child / no next
    // sibling) as an empty error whose HRESULT is 0.
    assert!(walker_reached_end(&err(0)));
}

#[test]
fn must_fail_provider_and_com_failures_are_not_the_end_of_the_list() {
    for code in [UIA_E_ELEMENTNOTAVAILABLE, E_FAIL, ERR_TIMEOUT] {
        assert!(!walker_reached_end(&err(code)), "code {code:#x}");
    }
}

#[test]
fn must_fail_child_probe_error_counts_as_has_children_unless_it_is_the_end_signal() {
    assert!(has_children_after_probe(Ok(())));
    assert!(!has_children_after_probe(Err(err(ERR_NOTFOUND))));
    assert!(!has_children_after_probe(Err(err(0))));
    assert!(has_children_after_probe(Err(err(
        UIA_E_ELEMENTNOTAVAILABLE
    ))));
    assert!(has_children_after_probe(Err(err(E_FAIL))));
}
