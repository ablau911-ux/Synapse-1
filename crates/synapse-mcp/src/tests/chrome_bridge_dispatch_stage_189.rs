//! #189 B2/B8: the bridge carries the dispatch stage its producer proved.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Mutex, atomic::AtomicU64};
use std::time::Duration;

use serde_json::json;
use synapse_browsers::{BrowserKindId, BrowserRegistry};
use synapse_core::error_codes;
use tokio::sync::Notify;

use super::{
    BridgeDispatchStage, BridgeInner, ChromeDebuggerBridge, ChromeDebuggerBridgeError,
    ChromeResponseError,
};

fn poisoned_bridge() -> ChromeDebuggerBridge {
    let bridge = ChromeDebuggerBridge {
        inner: Mutex::new(BridgeInner::default()),
        notify: Notify::new(),
        command_seq: AtomicU64::new(1),
        stale_code_reload: super::code_stamp::StaleCodeReloadGate::default(),
        registry: BrowserRegistry::global(),
    };
    std::thread::scope(|scope| {
        let joined = scope
            .spawn(|| {
                let _guard = bridge.inner.lock().unwrap();
                panic!("#189 test: poison the bridge lock while held");
            })
            .join();
        assert!(joined.is_err());
    });
    assert!(bridge.inner.is_poisoned());
    bridge
}

/// B2: a poisoned lock fails before the command is queued: proven undelivered.
#[tokio::test]
async fn must_fail_lock_poisoned_enqueue_is_typed_not_started() {
    let bridge = poisoned_bridge();
    let error = bridge
        .enqueue_and_await(
            &BrowserKindId::new("chrome").unwrap(),
            "domAction",
            json!({}),
            Duration::from_millis(50),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code(), error_codes::A11Y_CDP_ATTACH_FAILED);
    assert_eq!(error.dispatch_stage(), BridgeDispatchStage::NotStarted);
}

pub(crate) fn response_error(value: serde_json::Value) -> ChromeDebuggerBridgeError {
    let payload: ChromeResponseError = serde_json::from_value(value).unwrap();
    ChromeDebuggerBridgeError::from_response(Some(&payload), "domAction")
}

/// B8: the extension's reported stage survives the bridge conversion.
#[test]
fn must_fail_extension_dispatch_stage_is_never_dropped() {
    for (stage, expected) in [
        (json!("after_dispatch"), BridgeDispatchStage::AfterDispatch),
        (json!("not_started"), BridgeDispatchStage::NotStarted),
        (json!("bogus"), BridgeDispatchStage::Unreported),
        (serde_json::Value::Null, BridgeDispatchStage::Unreported),
    ] {
        let error = response_error(json!({
            "code": error_codes::CHROME_DOM_ACTION_UNSUPPORTED,
            "detail": "restoreWindowOpen threw after click",
            "dispatch_stage": stage,
        }));
        assert_eq!(error.code(), error_codes::CHROME_DOM_ACTION_UNSUPPORTED);
        assert_eq!(error.dispatch_stage(), expected, "{stage}");
    }
}

// The end-to-end wire -> click-status test lives in
// m2/click/tests/dispatch_delivery_189.rs: this file is also compiled into the
// native-host binary, which has no `crate::m2` / `crate::server`.

#[path = "browser_password_bridge_fixture_244.rs"]
pub(crate) mod password_fixture_244;
