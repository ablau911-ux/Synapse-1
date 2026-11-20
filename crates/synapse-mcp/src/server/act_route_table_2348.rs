//! FROZEN behaviour table of Gitea #2348, rows V0-V12 (`ActRoute`).
//! Written from the issue table before the code. Coders never edit this
//! file; a wrong row is reported to the manager.
//! Place: crates/synapse-mcp/src/server/act_route_table_2348.rs (registered
//! by the `#[cfg(test)] #[path]` lines at the end of act_route.rs).
//! One test per row, named after the row. V11 has a CONTROL case: its own
//! cases expect the Chrome bridge, which a stub that always answers the
//! bridge would pass.

use synapse_core::error_codes;

use super::{ActBackend, ActLocator, ActRoute, ActTarget, ActVerb};

const VERBS: [ActVerb; 12] = [
    ActVerb::Click,
    ActVerb::Dblclick,
    ActVerb::Press,
    ActVerb::Check,
    ActVerb::Uncheck,
    ActVerb::Select,
    ActVerb::Focus,
    ActVerb::Blur,
    ActVerb::Clear,
    ActVerb::SelectText,
    ActVerb::Submit,
    ActVerb::DispatchEvent,
];

const LOCATORS: [ActLocator; 8] = [
    ActLocator::NativeElementId,
    ActLocator::CdpElementId,
    ActLocator::PageElementId,
    ActLocator::NameAndRole,
    ActLocator::NameAlone,
    ActLocator::Selector,
    ActLocator::Point,
    ActLocator::None,
];

/// Every target kind except a Chrome bridge tab.
const NOT_BRIDGE: [ActTarget; 4] = [
    ActTarget::None,
    ActTarget::NativeWindow,
    ActTarget::NativeWindowWithRawCdp,
    ActTarget::RawCdpPage,
];

/// Locators only a page (or a native lookup by name) resolves: what the
/// human's front tab would answer if it were ever used.
const PAGE_LOCATORS: [ActLocator; 4] = [
    ActLocator::PageElementId,
    ActLocator::Selector,
    ActLocator::NameAndRole,
    ActLocator::NameAlone,
];

fn backend(verb: ActVerb, target: ActTarget, locator: ActLocator, option: bool) -> ActBackend {
    ActRoute::new(verb, target, locator, option).backend()
}

/// The refusal (code, text), or a failure naming what came instead.
fn refusal(
    verb: ActVerb,
    target: ActTarget,
    locator: ActLocator,
    option: bool,
) -> Result<(&'static str, String), String> {
    match backend(verb, target, locator, option) {
        ActBackend::Refused { code, text } => Ok((code, text)),
        other => Err(format!(
            "{verb:?} on {target:?} with {locator:?} (option {option}): expected a refusal, got {other:?}"
        )),
    }
}

#[test]
fn v0_no_target_but_a_bridge_tab_ever_reaches_the_chrome_bridge() {
    for target in NOT_BRIDGE {
        for verb in VERBS {
            for locator in LOCATORS {
                for option in [false, true] {
                    assert_ne!(
                        backend(verb, target, locator, option),
                        ActBackend::ChromeBridge,
                        "{verb:?} on {target:?} with {locator:?} (option {option})"
                    );
                }
            }
        }
    }
}

#[test]
fn v1_native_window_click_by_name_alone_is_native() {
    assert_eq!(
        backend(
            ActVerb::Click,
            ActTarget::NativeWindow,
            ActLocator::NameAlone,
            false
        ),
        ActBackend::NativeAccessibility
    );
}

#[test]
fn v2_native_window_click_by_name_and_role_is_native() {
    assert_eq!(
        backend(
            ActVerb::Click,
            ActTarget::NativeWindow,
            ActLocator::NameAndRole,
            false
        ),
        ActBackend::NativeAccessibility
    );
}

#[test]
fn v3_native_window_select_with_an_option_is_native() {
    for locator in [
        ActLocator::NameAlone,
        ActLocator::NameAndRole,
        ActLocator::NativeElementId,
    ] {
        assert_eq!(
            backend(ActVerb::Select, ActTarget::NativeWindow, locator, true),
            ActBackend::NativeAccessibility,
            "{locator:?}"
        );
    }
}

#[test]
fn v4_native_window_select_without_an_option_is_refused() -> Result<(), String> {
    for locator in [ActLocator::NameAlone, ActLocator::NativeElementId] {
        let (code, text) = refusal(ActVerb::Select, ActTarget::NativeWindow, locator, false)?;
        assert!(text.contains("option is needed"), "{locator:?}: {text}");
        assert_ne!(code, error_codes::TARGET_NOT_SET, "{locator:?}: {text}");
    }
    Ok(())
}

#[test]
fn v5_native_window_check_and_uncheck_by_name_alone_are_native() {
    for verb in [ActVerb::Check, ActVerb::Uncheck] {
        assert_eq!(
            backend(verb, ActTarget::NativeWindow, ActLocator::NameAlone, false),
            ActBackend::NativeAccessibility,
            "{verb:?}"
        );
    }
}

#[test]
fn v6_native_window_click_by_selector_is_refused_naming_what_it_takes() -> Result<(), String> {
    let (code, text) = refusal(
        ActVerb::Click,
        ActTarget::NativeWindow,
        ActLocator::Selector,
        false,
    )?;
    assert_eq!(code, error_codes::ACTION_TARGET_INVALID, "{text}");
    for word in ["native window", "name + role", "name,", "element_id", "x/y"] {
        assert!(text.contains(word), "missing {word:?}: {text}");
    }
    Ok(())
}

#[test]
fn v7_native_window_dblclick_and_focus_by_name_are_refused() -> Result<(), String> {
    let (code, text) = refusal(
        ActVerb::Dblclick,
        ActTarget::NativeWindow,
        ActLocator::NameAlone,
        false,
    )?;
    assert_eq!(code, error_codes::ACTION_TARGET_INVALID, "{text}");
    for word in ["dblclick", "native window", "element_id", "x/y"] {
        assert!(text.contains(word), "dblclick, missing {word:?}: {text}");
    }
    let (code, text) = refusal(
        ActVerb::Focus,
        ActTarget::NativeWindow,
        ActLocator::NameAlone,
        false,
    )?;
    assert_eq!(code, error_codes::ACTION_TARGET_INVALID, "{text}");
    assert!(
        text.contains("not available on a native window"),
        "focus: {text}"
    );
    Ok(())
}

#[test]
fn v8_raw_cdp_page_select_with_an_option_is_raw_cdp() {
    for locator in [
        ActLocator::CdpElementId,
        ActLocator::PageElementId,
        ActLocator::Selector,
        ActLocator::NameAndRole,
        ActLocator::NameAlone,
    ] {
        assert_eq!(
            backend(ActVerb::Select, ActTarget::RawCdpPage, locator, true),
            ActBackend::RawCdp,
            "{locator:?}"
        );
    }
}

#[test]
fn v9_raw_cdp_page_focus_by_selector_and_check_by_name_are_refused() -> Result<(), String> {
    for (verb, locator) in [
        (ActVerb::Focus, ActLocator::Selector),
        (ActVerb::Check, ActLocator::NameAlone),
        (ActVerb::Uncheck, ActLocator::NameAlone),
    ] {
        let (code, text) = refusal(verb, ActTarget::RawCdpPage, locator, false)?;
        assert_eq!(code, error_codes::ACTION_TARGET_INVALID, "{verb:?}: {text}");
        assert!(text.contains("raw-CDP page"), "{verb:?}: {text}");
        assert!(text.contains("element_id from find"), "{verb:?}: {text}");
        assert!(!text.contains("extension"), "{verb:?}: {text}");
    }
    Ok(())
}

#[test]
fn v10_native_window_hosting_a_raw_cdp_page_select_by_web_element_id_is_raw_cdp() {
    assert_eq!(
        backend(
            ActVerb::Select,
            ActTarget::NativeWindowWithRawCdp,
            ActLocator::CdpElementId,
            true
        ),
        ActBackend::RawCdp
    );
}

#[test]
fn v11_chrome_bridge_tab_keeps_the_chrome_bridge() {
    for (verb, locator, option) in [
        (ActVerb::Select, ActLocator::Selector, true),
        (ActVerb::Select, ActLocator::NameAlone, true),
        (ActVerb::Click, ActLocator::Selector, false),
        (ActVerb::Check, ActLocator::NameAlone, false),
    ] {
        assert_eq!(
            backend(verb, ActTarget::BridgeTab, locator, option),
            ActBackend::ChromeBridge,
            "{verb:?} with {locator:?}"
        );
    }
    // CONTROL: the same select on a native window is not the bridge.
    assert_ne!(
        backend(
            ActVerb::Select,
            ActTarget::NativeWindow,
            ActLocator::NameAlone,
            true
        ),
        ActBackend::ChromeBridge,
        "control: native window"
    );
}

#[test]
fn v12_no_target_refuses_every_page_locator_with_target_not_set() -> Result<(), String> {
    for verb in VERBS {
        for locator in PAGE_LOCATORS {
            let (code, text) = refusal(verb, ActTarget::None, locator, true)?;
            assert_eq!(
                code,
                error_codes::TARGET_NOT_SET,
                "{verb:?} with {locator:?}: {text}"
            );
        }
    }
    Ok(())
}
