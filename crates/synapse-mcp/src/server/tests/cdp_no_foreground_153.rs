//! #153: CDP element click/type must not need an OS foreground window.
use super::action_requires_live_foreground;
use synapse_core::ElementId;

#[test]
fn must_pass_cdp_element_click_and_type_skip_foreground_requirement() {
    let cdp = synapse_a11y::cdp_element_id(0x1234, 42);
    assert!(!action_requires_live_foreground("act_click", Some(&cdp)));
    assert!(!action_requires_live_foreground("act_type", Some(&cdp)));
}

#[test]
fn must_fail_native_and_pointer_actions_still_require_foreground() {
    let cdp = synapse_a11y::cdp_element_id(0x1234, 42);
    let native = ElementId::parse("0x1000:0000002a00000001").expect("valid id");
    assert!(action_requires_live_foreground("act_click", Some(&native)));
    assert!(action_requires_live_foreground("act_click", None));
    assert!(action_requires_live_foreground("act_type", None));
    // Raw OS-input tools keep the gate even when handed a CDP id.
    assert!(action_requires_live_foreground("act_press", Some(&cdp)));
    assert!(action_requires_live_foreground("act_stroke", Some(&cdp)));
}
