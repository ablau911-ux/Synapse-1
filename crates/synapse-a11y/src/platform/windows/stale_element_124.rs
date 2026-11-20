//! #124: a UIA provider reporting that an element is gone (subtree root that
//! disappears between resolution and the cache refresh, a disconnected
//! provider) must classify as a stale element on every UIA call that goes
//! through `map_uia_error`, never as a generic internal failure.

use uiautomation::Error;

use super::super::common::map_uia_error;
use crate::A11yError;

const UIA_E_ELEMENTNOTAVAILABLE: i32 = i32::from_ne_bytes(0x8004_0201_u32.to_ne_bytes());
const RPC_E_DISCONNECTED: i32 = i32::from_ne_bytes(0x8001_0108_u32.to_ne_bytes());
const E_FAIL: i32 = i32::from_ne_bytes(0x8000_4005_u32.to_ne_bytes());

#[test]
fn must_pass_element_not_available_hresult_is_stale_whatever_the_message() {
    let mapped = map_uia_error(Error::new(UIA_E_ELEMENTNOTAVAILABLE, "unrelated text"));
    assert!(
        matches!(mapped, A11yError::ElementStale { .. }),
        "{mapped:?}"
    );
    assert_eq!(mapped.code(), synapse_core::error_codes::A11Y_ELEMENT_STALE);
}

#[test]
fn must_pass_subscriber_failure_text_is_stale_even_without_the_hresult() {
    let mapped = map_uia_error(Error::new(
        -1,
        "An event was unable to invoke any of the subscribers",
    ));
    assert!(
        matches!(mapped, A11yError::ElementStale { .. }),
        "{mapped:?}"
    );
}

#[test]
fn must_pass_disconnected_provider_is_stale() {
    let mapped = map_uia_error(Error::new(
        RPC_E_DISCONNECTED,
        "The object invoked has disconnected from its clients",
    ));
    assert!(
        matches!(mapped, A11yError::ElementStale { .. }),
        "{mapped:?}"
    );
}

#[test]
fn must_fail_unrelated_provider_failure_stays_internal() {
    let mapped = map_uia_error(Error::new(E_FAIL, "Unspecified error"));
    assert!(matches!(mapped, A11yError::Internal { .. }), "{mapped:?}");
}
