//! #237: a running bridge worker whose code differs from the installed
//! `service_worker.js` must be reported stale, so the daemon can reload it
//! instead of silently routing commands to pre-deploy code.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::code_stamp::{
    StaleCodeHeal, StaleCodeReloadGate, code_stamp_stale_reason, heal_stale_code,
    is_code_stamp_reason, parse_code_stamp, remembered_after_reload, should_reload,
};
use super::{
    EXPECTED_EXTENSION_BUILD_ID, EXTENSION_ID, HostRecord, REQUIRED_DIRECT_HTTP_CAPABILITIES,
    SynapseChromeProfileInstallState, bridge_command_stale_reason_with_profile_state,
};

const WORKER_SHA: &str = "abababababababababababababababababababababababababababababababab";

/// A healthy direct-HTTP host whose only variable is the running code stamp.
fn host_running(stamp: Option<&str>) -> HostRecord {
    let mut capabilities: BTreeSet<String> = REQUIRED_DIRECT_HTTP_CAPABILITIES
        .iter()
        .map(|capability| (*capability).to_owned())
        .collect();
    capabilities.extend(["reloadSelf".to_owned(), "listTabs".to_owned()]);
    HostRecord {
        browser_kind: synapse_browsers::BrowserKindId::legacy_default(),
        extension_browser_kind: None,
        origin: format!("chrome-extension://{EXTENSION_ID}/"),
        extension_id: Some(EXTENSION_ID.to_owned()),
        extension_version: Some("0.1.1".to_owned()),
        extension_protocol_version: Some(1),
        extension_build_id: Some(EXPECTED_EXTENSION_BUILD_ID.to_owned()),
        extension_build_sha256: None,
        extension_declared_build_sha256: None,
        extension_service_worker_sha256: Some(WORKER_SHA.to_owned()),
        extension_service_worker_sha256_status: Some("ok".to_owned()),
        extension_service_worker_sha256_source: None,
        extension_service_worker_byte_length: None,
        extension_service_worker_sha256_error: None,
        extension_code_stamp: stamp.map(ToOwned::to_owned),
        extension_capabilities: capabilities,
        extension_user_agent: None,
        extension_debugger_api_available: Some(true),
        extension_popup_risk_suppression: None,
        pid: 0,
        parent_window: None,
        transport: Some("direct_http".to_owned()),
        bridge_token_digest: [0; 32],
        registered_unix_ms: 0,
        last_seen_unix_ms: 0,
        last_disconnect_detail: None,
        last_detach_reason: None,
    }
}

fn installed(stamp: Option<&str>) -> SynapseChromeProfileInstallState {
    SynapseChromeProfileInstallState {
        detail: String::new(),
        active_profile_extension_path: None,
        active_profile_service_worker_sha256: Some(WORKER_SHA.to_owned()),
        active_profile_service_worker_error: None,
        active_profile_code_stamp: stamp.map(ToOwned::to_owned),
    }
}

#[test]
fn must_pass_command_on_a_worker_running_the_installed_code_is_admitted() {
    let reason = bridge_command_stale_reason_with_profile_state(
        &host_running(Some(STAMP_NEW)),
        "listTabs",
        &installed(Some(STAMP_NEW)),
    );
    assert_eq!(reason, None);
}

#[test]
fn must_fail_command_on_a_worker_running_older_code_is_refused_as_code_stamp_stale() {
    for (running, running_label) in [(Some(STAMP_OLD), STAMP_OLD), (None, "not_seen_yet")] {
        let reason = bridge_command_stale_reason_with_profile_state(
            &host_running(running),
            "listTabs",
            &installed(Some(STAMP_NEW)),
        );
        let reason = reason.unwrap_or_default();
        assert!(is_code_stamp_reason(&reason), "{running:?}: {reason}");
        assert!(
            reason.contains(&format!("code_stamp={running_label}")),
            "{running:?}: reason must name the running stamp: {reason}"
        );
        assert!(
            reason.contains(&format!("expected={STAMP_NEW}")),
            "{running:?}: reason must name the installed stamp: {reason}"
        );
    }
}

#[test]
fn must_pass_reload_self_is_still_admitted_on_a_code_stamp_stale_worker() {
    let reason = bridge_command_stale_reason_with_profile_state(
        &host_running(Some(STAMP_OLD)),
        "reloadSelf",
        &installed(Some(STAMP_NEW)),
    );
    assert_eq!(reason, None);
}

#[test]
fn must_pass_a_reload_is_tried_once_per_installed_stamp() {
    assert!(should_reload(None, Some(STAMP_NEW)));
    assert!(should_reload(Some(STAMP_OLD), Some(STAMP_NEW)));
}

#[test]
fn must_fail_a_second_reload_for_the_same_installed_stamp_or_an_unstamped_install() {
    assert!(!should_reload(Some(STAMP_NEW), Some(STAMP_NEW)));
    assert!(!should_reload(None, None));
    assert!(!should_reload(Some(STAMP_OLD), None));
}

const STAMP_NEW: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const STAMP_OLD: &str = "2222222222222222222222222222222222222222222222222222222222222222";

fn worker_source(stamp: &str) -> String {
    format!(
        "const PROTOCOL_VERSION = 1;\nconst BRIDGE_CODE_STAMP = \"{stamp}\";\nconst DEBUGGER_COMMAND_TIMEOUT_MS = 5000;\n"
    )
}

#[test]
fn must_pass_installed_worker_stamp_is_parsed() {
    assert_eq!(
        parse_code_stamp(&worker_source(STAMP_NEW)).as_deref(),
        Some(STAMP_NEW)
    );
}

#[test]
fn must_pass_repo_worker_source_carries_the_installer_placeholder() {
    let worker = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../extensions/synapse-chrome-debugger/service_worker.js");
    let source = std::fs::read_to_string(&worker).unwrap_or_default();
    assert_eq!(parse_code_stamp(&source).as_deref(), Some("unstamped"));
}

#[test]
fn must_pass_running_stamp_equal_to_installed_stamp_is_fresh() {
    assert_eq!(
        code_stamp_stale_reason(Some(STAMP_NEW), Some(STAMP_NEW)),
        None
    );
}

#[test]
fn must_pass_no_installed_stamp_cannot_mark_the_worker_stale() {
    assert_eq!(code_stamp_stale_reason(Some(STAMP_OLD), None), None);
    assert_eq!(code_stamp_stale_reason(None, None), None);
}

#[test]
fn must_fail_running_pre_deploy_code_is_stale_and_names_both_stamps() {
    let reason = code_stamp_stale_reason(Some(STAMP_OLD), Some(STAMP_NEW));
    let reason = reason.as_deref().unwrap_or_default();
    assert!(
        reason.contains(&format!("code_stamp={STAMP_OLD}")),
        "{reason}"
    );
    assert!(
        reason.contains(&format!("expected={STAMP_NEW}")),
        "{reason}"
    );
    assert!(is_code_stamp_reason(reason));
}

#[test]
fn must_fail_worker_without_a_stamp_is_stale_when_the_install_is_stamped() {
    let reason = code_stamp_stale_reason(None, Some(STAMP_NEW));
    let reason = reason.as_deref().unwrap_or_default();
    assert!(reason.contains("code_stamp=not_seen_yet"), "{reason}");
    assert!(is_code_stamp_reason(reason));
}

#[test]
fn must_fail_source_without_a_well_formed_stamp_line_parses_to_none() {
    assert_eq!(parse_code_stamp("const PROTOCOL_VERSION = 1;\n"), None);
    assert_eq!(
        parse_code_stamp("const BRIDGE_CODE_STAMP = unquoted;\n"),
        None
    );
    assert_eq!(parse_code_stamp("const BRIDGE_CODE_STAMP = \"\";\n"), None);
    assert!(!is_code_stamp_reason("build_id=x expected=y"));
}

#[test]
fn must_pass_a_healed_or_failed_reload_leaves_the_gate_open_for_the_next_worker() {
    // Healed: another profile's cached worker with the same installed stamp
    // must still be healable.
    let after_heal = remembered_after_reload(false, Some(STAMP_NEW));
    assert_eq!(after_heal, None);
    assert!(should_reload(after_heal.as_deref(), Some(STAMP_NEW)));
    // Timed out / Chrome busy: proves nothing, so the next command may retry.
    assert_eq!(remembered_after_reload(false, None), None);
}

#[test]
fn must_fail_a_reload_that_reconnects_still_stale_closes_the_gate_for_that_stamp() {
    let remembered = remembered_after_reload(true, Some(STAMP_NEW));
    assert_eq!(remembered.as_deref(), Some(STAMP_NEW));
    assert!(!should_reload(remembered.as_deref(), Some(STAMP_NEW)));
    // A new install (new stamp) reopens it.
    assert!(should_reload(remembered.as_deref(), Some(STAMP_OLD)));
}

/// Drives `heal_stale_code` with a fake reload: `outcome` is what the reload
/// returns (`Err(true)` = reconnected still stale, `Err(false)` = timed out).
async fn heal(
    gate: &StaleCodeReloadGate,
    still_stale: bool,
    outcome: Result<(), bool>,
    reloads: &AtomicUsize,
    records: &AtomicUsize,
) -> Result<StaleCodeHeal, bool> {
    heal_stale_code(
        gate,
        || still_stale,
        Some(STAMP_NEW.to_owned()),
        || {
            reloads.fetch_add(1, Ordering::SeqCst);
            std::future::ready(outcome)
        },
        |reconnected_still_stale: &bool| *reconnected_still_stale,
        |_: &Result<(), bool>| {
            records.fetch_add(1, Ordering::SeqCst);
        },
    )
    .await
}

#[tokio::test]
async fn must_pass_stale_code_is_reloaded_once_recorded_and_the_command_retried() {
    let gate = StaleCodeReloadGate::default();
    let (reloads, records) = (AtomicUsize::new(0), AtomicUsize::new(0));
    let healed = heal(&gate, true, Ok(()), &reloads, &records).await;
    assert_eq!(healed, Ok(StaleCodeHeal::Retry));
    assert_eq!(reloads.load(Ordering::SeqCst), 1);
    assert_eq!(records.load(Ordering::SeqCst), 1);
    // Healed: the gate stays open, so another cached worker is healable too.
    assert_eq!(*gate.lock().await, None);
    assert_eq!(
        heal(&gate, true, Ok(()), &reloads, &records).await,
        Ok(StaleCodeHeal::Retry)
    );
    assert_eq!(reloads.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn must_pass_a_worker_already_healed_by_another_command_is_retried_without_reload() {
    let gate = StaleCodeReloadGate::default();
    let (reloads, records) = (AtomicUsize::new(0), AtomicUsize::new(0));
    let outcome = heal(&gate, false, Ok(()), &reloads, &records).await;
    assert_eq!(outcome, Ok(StaleCodeHeal::Retry));
    assert_eq!(reloads.load(Ordering::SeqCst), 0);
    assert_eq!(records.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn must_fail_an_unhealable_stamp_is_refused_without_reloading_again() {
    let gate = StaleCodeReloadGate::default();
    let (reloads, records) = (AtomicUsize::new(0), AtomicUsize::new(0));
    // Reconnected yet still stale: the reload error surfaces and the stamp is kept.
    assert_eq!(
        heal(&gate, true, Err(true), &reloads, &records).await,
        Err(true)
    );
    assert_eq!(gate.lock().await.as_deref(), Some(STAMP_NEW));
    // The next command keeps its stale refusal and triggers no reload.
    assert_eq!(
        heal(&gate, true, Ok(()), &reloads, &records).await,
        Ok(StaleCodeHeal::Refuse)
    );
    assert_eq!(reloads.load(Ordering::SeqCst), 1);
    assert_eq!(records.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn must_fail_a_timed_out_reload_surfaces_its_error_and_leaves_the_gate_open() {
    let gate = StaleCodeReloadGate::default();
    let (reloads, records) = (AtomicUsize::new(0), AtomicUsize::new(0));
    assert_eq!(
        heal(&gate, true, Err(false), &reloads, &records).await,
        Err(false)
    );
    assert_eq!(*gate.lock().await, None);
    assert_eq!(
        heal(&gate, true, Ok(()), &reloads, &records).await,
        Ok(StaleCodeHeal::Retry)
    );
    assert_eq!(reloads.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn must_pass_concurrent_stale_commands_share_one_reload() {
    let gate = StaleCodeReloadGate::default();
    let stale_flag = AtomicBool::new(true);
    let stale = &stale_flag;
    let reloads = AtomicUsize::new(0);
    let reloads = &reloads;
    let command = || {
        heal_stale_code(
            &gate,
            || stale.load(Ordering::SeqCst),
            Some(STAMP_NEW.to_owned()),
            || {
                reloads.fetch_add(1, Ordering::SeqCst);
                // The reload suspends, so the other in-flight commands are
                // polled while it runs; only a gate held across the whole
                // reload keeps them from starting a second reload.
                async move {
                    for _ in 0..8 {
                        tokio::task::yield_now().await;
                    }
                    stale.store(false, Ordering::SeqCst);
                    Ok::<(), bool>(())
                }
            },
            |_: &bool| false,
            |_: &Result<(), bool>| {},
        )
    };
    let (first, second, third) = tokio::join!(command(), command(), command());
    assert_eq!(
        [first, second, third],
        [
            Ok(StaleCodeHeal::Retry),
            Ok(StaleCodeHeal::Retry),
            Ok(StaleCodeHeal::Retry)
        ]
    );
    assert_eq!(reloads.load(Ordering::SeqCst), 1);
}
