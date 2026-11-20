//! MY TDD contract for the multi-browser bridge foundation (daemon side):
//! per-kind active hosts, token-bound kinds, per-kind unavailability,
//! per-command unsupported capabilities, fail-closed routing and health rows.
//! `testium` is a test-only fixture kind (`synapse-browsers/tests/fixtures`).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Mutex, OnceLock, atomic::AtomicU64};
use std::time::Duration;

use serde_json::json;
use synapse_browsers::{BridgeLane, BrowserKindId, BrowserRegistry};
use synapse_core::error_codes;
use tokio::sync::{Notify, oneshot};

use super::routing::{WindowProcess, WindowProcessProbe, resolve_command_route};
use super::{
    BridgeInner, ChromeDebuggerBridge, EXTENSION_ID, NativeMessageRequest, NativeRegisterRequest,
    PendingResponse, bridge_command_stale_reason, chrome_debugger_health_endpoint,
    health_rows::browser_row_health, registration_token_kind,
};

const CHROME_ORIGIN: &str = "chrome-extension://leoocgnkjnplbfdbklajepahofecgfbk/";

fn two_kind_registry() -> &'static BrowserRegistry {
    static REGISTRY: OnceLock<BrowserRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        BrowserRegistry::from_json_sources(&[
            (
                "chrome.json",
                include_str!("../../../synapse-browsers/browsers/chrome.json"),
            ),
            (
                "testium.json",
                include_str!("../../../synapse-browsers/tests/fixtures/testium.json"),
            ),
        ])
        .unwrap()
    })
}

fn kind(id: &str) -> BrowserKindId {
    BrowserKindId::new(id).unwrap()
}

fn test_bridge() -> ChromeDebuggerBridge {
    ChromeDebuggerBridge {
        inner: Mutex::new(BridgeInner::default()),
        notify: Notify::new(),
        command_seq: AtomicU64::new(1),
        stale_code_reload: super::code_stamp::StaleCodeReloadGate::default(),
        registry: two_kind_registry(),
    }
}

fn register(bridge: &ChromeDebuggerBridge, browser: &str, transport: &str) -> String {
    bridge
        .register(
            NativeRegisterRequest {
                origin: CHROME_ORIGIN.to_owned(),
                pid: 0,
                parent_window: None,
                bridge_protocol_version: super::BRIDGE_PROTOCOL_VERSION,
                transport: Some(transport.to_owned()),
            },
            kind(browser),
        )
        .unwrap()
        .host_id
}

fn park_pending(
    bridge: &ChromeDebuggerBridge,
    host_id: &str,
    command_id: &str,
) -> oneshot::Receiver<super::ChromeResponse> {
    let (sender, receiver) = oneshot::channel();
    bridge.inner.lock().unwrap().pending.insert(
        command_id.to_owned(),
        PendingResponse {
            host_id: host_id.to_owned(),
            kind: "listTabs".to_owned(),
            sender,
        },
    );
    receiver
}

#[test]
fn must_pass_chrome_row_preserves_legacy_wire_ids() {
    let registry = BrowserRegistry::global();
    let chrome = registry.legacy_default().unwrap();
    assert_eq!(chrome.extension.id, EXTENSION_ID);
    assert_eq!(
        chrome.extension_origin(),
        "chrome-extension://leoocgnkjnplbfdbklajepahofecgfbk"
    );
    assert_eq!(chrome.registration_token_file, "chrome-bridge-token.txt");
    assert_eq!(chrome.health_key, "chrome_bridge");
    assert_eq!(chrome.tab_target_prefix, "chrome-tab:");
    assert_eq!(
        chrome.endpoint(BridgeLane::Tabs),
        chrome_debugger_health_endpoint(EXTENSION_ID)
    );
    assert_eq!(
        chrome.policy.as_ref().unwrap().root,
        "Software\\Policies\\Google\\Chrome"
    );
}

#[test]
fn must_pass_two_kinds_hosts_coexist() {
    let bridge = test_bridge();
    let chrome_host = register(&bridge, "chrome", "direct_http");
    let mut chrome_pending = park_pending(&bridge, &chrome_host, "c-1");

    let testium_host = register(&bridge, "testium", "direct_http");
    {
        let inner = bridge.inner.lock().unwrap();
        assert_eq!(
            inner.active_hosts.active(&kind("chrome")),
            Some(chrome_host.as_str())
        );
        assert_eq!(
            inner.active_hosts.active(&kind("testium")),
            Some(testium_host.as_str())
        );
        assert!(inner.pending.contains_key("c-1"));
    }
    assert!(
        matches!(
            chrome_pending.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ),
        "a testium registration must not fail chrome's pending command"
    );

    // Same-kind reconnect still replaces (and fails) only that kind's host.
    let mut testium_pending = park_pending(&bridge, &testium_host, "t-1");
    let chrome_again = register(&bridge, "chrome", "direct_http");
    let failed = chrome_pending.try_recv().unwrap();
    assert!(!failed.ok);
    assert!(matches!(
        testium_pending.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    let inner = bridge.inner.lock().unwrap();
    assert_eq!(
        inner.active_hosts.active(&kind("chrome")),
        Some(chrome_again.as_str())
    );
    assert_eq!(
        inner.active_hosts.active(&kind("testium")),
        Some(testium_host.as_str())
    );
    assert!(chrome_again.starts_with("chrome-native-"));
    assert!(testium_host.starts_with("testium-native-"));
}

#[test]
fn must_fail_token_of_kind_a_registers_as_b() {
    let registry = two_kind_registry();
    let dir = tempfile::tempdir().unwrap();
    let chrome_token = "a".repeat(64);
    let testium_token = "b".repeat(64);
    std::fs::write(dir.path().join("chrome-bridge-token.txt"), &chrome_token).unwrap();
    std::fs::write(
        dir.path().join("browser-bridge-token-testium.txt"),
        &testium_token,
    )
    .unwrap();
    assert_eq!(
        registration_token_kind(registry, dir.path(), &testium_token),
        Some(kind("testium"))
    );
    assert_eq!(
        registration_token_kind(registry, dir.path(), &chrome_token),
        Some(kind("chrome"))
    );
    assert_eq!(registration_token_kind(registry, dir.path(), "wrong"), None);

    // The testium-token host claims chrome in hello: every command is stale.
    let bridge = test_bridge();
    let host_id = register(&bridge, "testium", "direct_http");
    bridge
        .post_message(NativeMessageRequest {
            host_id: host_id.clone(),
            message: json!({
                "type": "hello",
                "browserKind": "chrome",
                "extensionId": EXTENSION_ID,
            }),
        })
        .unwrap();
    let inner = bridge.inner.lock().unwrap();
    let host = inner.hosts.get(&host_id).unwrap();
    let reason = bridge_command_stale_reason(registry, host, "listTabs").unwrap();
    assert!(
        reason.contains("browser_kind=chrome expected=testium"),
        "{reason}"
    );
    assert_eq!(inner.active_hosts.active(&kind("chrome")), None);
}

#[test]
fn must_fail_duplicate_token_content_resolves_no_kind() {
    let registry = two_kind_registry();
    let dir = tempfile::tempdir().unwrap();
    let shared = "c".repeat(64);
    std::fs::write(dir.path().join("chrome-bridge-token.txt"), &shared).unwrap();
    std::fs::write(dir.path().join("browser-bridge-token-testium.txt"), &shared).unwrap();
    assert_eq!(registration_token_kind(registry, dir.path(), &shared), None);
}

#[tokio::test]
async fn must_fail_command_without_host_for_kind() {
    let bridge = test_bridge();
    register(&bridge, "chrome", "direct_http");
    let error = bridge
        .enqueue_and_await(
            &kind("testium"),
            "listTabs",
            json!({}),
            Duration::from_millis(50),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code(), error_codes::A11Y_CDP_EXTENSION_UNAVAILABLE);
    assert!(
        error.detail().contains("kind=testium"),
        "{}",
        error.detail()
    );
    assert!(bridge.inner.lock().unwrap().commands.is_empty());
}

#[tokio::test]
async fn must_fail_unsupported_capability_per_command() {
    let bridge = test_bridge();
    // native_messaging transport: identity checks are skipped, so only the
    // per-row capability gate decides.
    register(&bridge, "testium", "native_messaging");
    let error = bridge
        .enqueue_and_await(
            &kind("testium"),
            "evaluateScript",
            json!({}),
            Duration::from_millis(50),
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.code(),
        error_codes::BROWSER_BRIDGE_CAPABILITY_UNSUPPORTED
    );
    assert!(
        error.detail().contains("evaluateScript"),
        "{}",
        error.detail()
    );
    assert!(bridge.inner.lock().unwrap().commands.is_empty());

    // Another command passes the gate: it is queued and only times out.
    let error = bridge
        .enqueue_and_await(
            &kind("testium"),
            "listTabs",
            json!({}),
            Duration::from_millis(30),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code(), error_codes::A11Y_CDP_EXTENSION_TIMEOUT);
}

struct FakeProbe(Option<(&'static str, Option<&'static str>)>);

impl WindowProcessProbe for FakeProbe {
    fn process_of(&self, _hwnd: i64) -> Option<WindowProcess> {
        self.0.map(|(name, path)| WindowProcess {
            name: name.to_owned(),
            path: path.map(str::to_owned),
        })
    }
}

fn route(
    probe: &FakeProbe,
    active: &[BrowserKindId],
    params: serde_json::Value,
) -> Result<super::routing::BrowserRoute, synapse_browsers::BrowserBridgeError> {
    resolve_command_route(two_kind_registry(), probe, active, &params)
}

#[test]
fn must_pass_route_resolution_rules() {
    let both = [kind("chrome"), kind("testium")];
    let testium_window = FakeProbe(Some(("TESTIUM.EXE", None)));
    let notepad = FakeProbe(Some(("notepad.exe", None)));
    let unreadable = FakeProbe(None);

    assert_eq!(
        route(&testium_window, &both, json!({"hwnd": 7}))
            .unwrap()
            .kind,
        kind("testium")
    );
    assert_eq!(
        route(
            &notepad,
            &both,
            json!({"hwnd": 7, "targetIdHint": "testium-tab:3"})
        )
        .unwrap()
        .kind,
        kind("testium")
    );
    assert_eq!(
        route(&notepad, &[kind("chrome")], json!({"hwnd": 7}))
            .unwrap()
            .kind,
        kind("chrome"),
        "single active kind keeps the legacy non-browser-window behaviour"
    );
    assert_eq!(
        route(&unreadable, &both, json!({"hwnd": 7})).unwrap().kind,
        kind("chrome")
    );
    assert_eq!(
        route(&notepad, &[], json!({})).unwrap().kind,
        kind("chrome")
    );
}

#[test]
fn must_fail_route_resolution_never_guesses() {
    let both = [kind("chrome"), kind("testium")];
    let chrome_window = FakeProbe(Some(("chrome.exe", None)));
    let notepad = FakeProbe(Some(("notepad.exe", None)));
    for (probe, params) in [
        (
            &chrome_window,
            json!({"hwnd": 7, "targetIdHint": "testium-tab:3"}),
        ),
        (&notepad, json!({"hwnd": 7})),
        (
            &chrome_window,
            json!({"hwnd": 7, "targetIdHint": "foo-tab:1"}),
        ),
        (
            &notepad,
            json!({"hwnd": 7, "targetIdHint": "chrome-tab:1", "elementId": "testium-tab:2:frame:0:path:0"}),
        ),
    ] {
        let error = route(probe, &both, params.clone()).unwrap_err();
        assert_eq!(
            error.code,
            error_codes::BROWSER_BRIDGE_KIND_UNRESOLVED,
            "{params}"
        );
    }
}

#[test]
fn must_pass_missing_browser_health_is_not_installed_and_never_error() {
    let bridge = test_bridge();
    let testium = two_kind_registry().by_id("testium").unwrap();
    let health = browser_row_health(&bridge, testium, None);
    assert_eq!(health.status, "not_installed");
    assert_ne!(health.status, "error");
    let detail = health.detail.unwrap();
    assert!(detail.contains("browser_kind=testium"), "{detail}");
}

#[test]
fn must_pass_edge_not_installed_health_never_degrades() {
    // The shipped edge row (browsers/edge.json), not a fixture: a host without
    // Edge reports `not_installed` with the row's winget remediation and never
    // a degrading status.
    let registry = BrowserRegistry::from_json_sources(&[
        (
            "chrome.json",
            include_str!("../../../synapse-browsers/browsers/chrome.json"),
        ),
        (
            "edge.json",
            include_str!("../../../synapse-browsers/browsers/edge.json"),
        ),
    ])
    .unwrap();
    let edge = registry.by_id("edge").unwrap();
    let bridge = test_bridge();
    let health = browser_row_health(&bridge, edge, None);
    assert_eq!(health.status, "not_installed");
    for degrading in ["error", "degraded", "unavailable", "stale"] {
        assert_ne!(health.status, degrading);
    }
    let detail = health.detail.unwrap();
    assert!(detail.contains("browser_kind=edge"), "{detail}");
    assert!(detail.contains("winget install Microsoft.Edge"), "{detail}");
    assert!(detail.contains("tab_control_available=false"), "{detail}");
}
