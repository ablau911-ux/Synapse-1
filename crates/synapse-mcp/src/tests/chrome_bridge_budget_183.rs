//! #183: bridge command budget outlives the in-extension wait chain, the
//! timeout detail names the real budget, and a command the daemon gave up on
//! is removed from the delivery queue and carries an absolute deadline.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    sync::{Mutex, OnceLock, atomic::AtomicU64},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde_json::json;
use synapse_browsers::{BrowserKindId, BrowserRegistry};
use synapse_core::error_codes;
use tokio::sync::{Notify, oneshot};

use super::{
    BridgeInner, ChromeCommand, ChromeDebuggerBridge, ChromeDebuggerBridgeError,
    NativeRegisterRequest, PendingResponse, QueuedCommand,
    budget::{COMMAND_BUDGET_MARGIN, DEFAULT_COMMAND_TIMEOUT, command_budget},
};

const CHROME_ORIGIN: &str = "chrome-extension://leoocgnkjnplbfdbklajepahofecgfbk/";

/// Chrome plus the test-only `testium` kind: a `native_messaging` testium host
/// skips identity/code-stamp checks, so the real `enqueue_and_await` path is
/// driven without depending on the host's installed Chrome profile.
fn fixture_registry() -> &'static BrowserRegistry {
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

fn testium() -> BrowserKindId {
    BrowserKindId::new("testium").unwrap()
}

fn bridge_with(registry: &'static BrowserRegistry) -> ChromeDebuggerBridge {
    ChromeDebuggerBridge {
        inner: Mutex::new(BridgeInner::default()),
        notify: Notify::new(),
        command_seq: AtomicU64::new(1),
        stale_code_reload: super::code_stamp::StaleCodeReloadGate::default(),
        registry,
    }
}

/// A bridge with one active `testium` host; returns the host id.
fn bridge_with_active_host() -> (ChromeDebuggerBridge, String) {
    let bridge = bridge_with(fixture_registry());
    let host_id = bridge
        .register(
            NativeRegisterRequest {
                origin: CHROME_ORIGIN.to_owned(),
                pid: 0,
                parent_window: None,
                bridge_protocol_version: super::BRIDGE_PROTOCOL_VERSION,
                transport: Some("native_messaging".to_owned()),
            },
            testium(),
        )
        .unwrap()
        .host_id;
    (bridge, host_id)
}

fn now_ms() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}

#[test]
fn must_pass_no_wait_keeps_the_default_budget() {
    assert_eq!(
        command_budget("listTabs", &json!({})),
        DEFAULT_COMMAND_TIMEOUT
    );
    assert_eq!(
        command_budget("pageContent", &json!({"waitTimeoutMs": 1000})),
        DEFAULT_COMMAND_TIMEOUT
    );
}

#[test]
fn must_pass_max_caller_wait_gets_the_margin() {
    let budget = command_budget("waitForSelector", &json!({"timeoutMs": 30_000}));
    assert_eq!(budget, Duration::from_secs(30) + COMMAND_BUDGET_MARGIN);
    assert!(budget > Duration::from_secs(30));
}

#[test]
fn must_pass_chained_pdf_wait_is_covered() {
    let budget = command_budget("pagePdf", &json!({"waitTimeoutMs": 60_000}));
    assert_eq!(budget, Duration::from_secs(60) + COMMAND_BUDGET_MARGIN);
}

#[test]
fn must_pass_dom_action_counts_default_and_explicit_auto_wait() {
    let default_auto = command_budget("domAction", &json!({"waitTimeoutMs": 30_000}));
    assert_eq!(
        default_auto,
        Duration::from_secs(32) + COMMAND_BUDGET_MARGIN
    );
    let explicit = command_budget(
        "cdpInput",
        &json!({"waitTimeoutMs": 30_000, "autoWaitTimeoutMs": 30_000}),
    );
    assert_eq!(explicit, Duration::from_secs(60) + COMMAND_BUDGET_MARGIN);
}

#[test]
fn must_fail_hostile_or_malformed_waits_are_bounded() {
    let huge = command_budget("downloads", &json!({"waitTimeoutMs": u64::MAX}));
    assert_eq!(huge, Duration::from_secs(300) + COMMAND_BUDGET_MARGIN);
    let garbage = command_budget("domAction", &json!({"waitTimeoutMs": "x", "timeoutMs": -5}));
    // Non-numeric / negative waits are ignored: default auto-wait (2 s) + margin
    // is below the 30 s floor, so the budget is exactly the default.
    assert_eq!(garbage, Duration::from_secs(30));
    assert_eq!(garbage, DEFAULT_COMMAND_TIMEOUT);
}

#[test]
fn must_pass_snake_case_wait_fields_extend_the_budget() {
    assert_eq!(
        command_budget("downloads", &json!({"wait_timeout_ms": 60_000})),
        Duration::from_secs(65)
    );
    assert_eq!(
        command_budget("downloads", &json!({"timeout_ms": 45_000})),
        Duration::from_secs(50)
    );
}

#[test]
fn must_pass_timeout_detail_names_the_actual_budget() {
    let error = ChromeDebuggerBridgeError::timeout("domAction", Duration::from_secs(37));
    assert_eq!(
        error.detail(),
        "Chrome debugger extension command \"domAction\" timed out after 37s"
    );
    assert_eq!(error.code(), error_codes::A11Y_CDP_EXTENSION_TIMEOUT);
}

#[test]
fn must_pass_command_serializes_absolute_deadline_only_when_set() {
    let with = serde_json::to_value(ChromeCommand {
        id: "c1".to_owned(),
        kind: "domAction".to_owned(),
        params: json!({}),
        deadline_unix_ms: Some(1_700_000_000_000),
    })
    .unwrap();
    assert_eq!(with["deadlineUnixMs"], json!(1_700_000_000_000_u64));
    let without = serde_json::to_value(ChromeCommand {
        id: "c2".to_owned(),
        kind: "domAction".to_owned(),
        params: json!({}),
        deadline_unix_ms: None,
    })
    .unwrap();
    assert!(without.get("deadlineUnixMs").is_none());
}

#[tokio::test]
async fn must_pass_enqueue_stamps_the_absolute_deadline_now_plus_budget() {
    let (bridge, _host_id) = bridge_with_active_host();
    let budget = Duration::from_secs(37);
    let before = now_ms();
    let testium_kind = testium();
    let command = bridge.enqueue_and_await(&testium_kind, "listTabs", json!({}), budget);
    tokio::pin!(command);
    // Poll once: the command is queued synchronously before the response wait.
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut command)
            .await
            .is_err(),
        "no extension answers, so the command must still be pending"
    );
    let after = now_ms();
    let inner = bridge.inner.lock().unwrap();
    assert_eq!(inner.commands.len(), 1);
    let deadline = inner.commands[0]
        .command
        .deadline_unix_ms
        .expect("enqueue_and_await must stamp an absolute deadline");
    let budget_ms = 37_000;
    assert!(
        deadline >= before + budget_ms && deadline <= after + budget_ms,
        "deadline {deadline} outside [{}, {}]",
        before + budget_ms,
        after + budget_ms
    );
}

#[tokio::test]
async fn must_fail_dropping_a_timed_out_command_removes_it_from_the_delivery_queue() {
    let (bridge, host_id) = bridge_with_active_host();
    {
        // A neighbour command on the same host must survive the timeout cleanup.
        let mut inner = bridge.inner.lock().unwrap();
        let (sender, _receiver) = oneshot::channel();
        inner.pending.insert(
            "other".to_owned(),
            PendingResponse {
                host_id: host_id.clone(),
                kind: "listTabs".to_owned(),
                sender,
            },
        );
        inner.commands.push_back(QueuedCommand {
            host_id: host_id.clone(),
            command: ChromeCommand {
                id: "other".to_owned(),
                kind: "listTabs".to_owned(),
                params: json!({}),
                deadline_unix_ms: None,
            },
        });
    }
    let error = bridge
        .enqueue_and_await(&testium(), "listTabs", json!({}), Duration::from_millis(1))
        .await
        .unwrap_err();
    assert_eq!(error.code(), error_codes::A11Y_CDP_EXTENSION_TIMEOUT);
    let inner = bridge.inner.lock().unwrap();
    assert_eq!(
        inner.commands.len(),
        1,
        "timed-out command must leave the queue"
    );
    assert_eq!(inner.commands[0].command.id, "other");
    assert_eq!(
        inner.pending.len(),
        1,
        "timed-out command must leave pending"
    );
    assert!(inner.pending.contains_key("other"));
}
