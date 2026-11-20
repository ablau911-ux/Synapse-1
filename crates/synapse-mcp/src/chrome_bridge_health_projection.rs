//! #82: public bridge health uses typed facts, never raw diagnostic text.
use super::{ChromeBridgeHealthRecord, SubsystemHealth};

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct InstallFacts {
    pub scanned: bool,
    pub installed: Option<bool>,
    pub active_profile_installed: Option<bool>,
}

pub(super) fn lock_error(detailed: bool) -> SubsystemHealth {
    let mut detail = "chrome_bridge_state_lock_poisoned tab_control_available=false".to_owned();
    if detailed {
        detail.push_str(&format!(" expected_extension_id={}", super::EXTENSION_ID));
    }
    SubsystemHealth {
        status: "error".to_owned(),
        detail: Some(detail),
        ..SubsystemHealth::default()
    }
}

fn bool_name(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "true",
        Some(false) => "false",
        None => "unknown",
    }
}

fn public_causes(host: &ChromeBridgeHealthRecord) -> String {
    let expected =
        super::ExpectedHostIdentity::for_kind(super::bridge().registry, &host.browser_kind);
    let worker_verified = matches!(
        (&host.extension_service_worker_sha256, &host.expected_service_worker_sha256),
        (Some(actual), Some(expected)) if actual.trim().eq_ignore_ascii_case(expected.trim())
            && host.extension_service_worker_sha256_status.as_deref() == Some("ok")
    );
    let missing = expected.missing_capabilities(&host.extension_capabilities);
    let mut causes = Vec::new();
    for (present, cause) in [
        (
            host.extension_id.as_deref() != Some(expected.extension_id),
            "bridge_identity_mismatch",
        ),
        (
            super::browser_kind_stale_reason(
                &host.browser_kind,
                host.extension_browser_kind.as_deref(),
            )
            .is_some(),
            "browser_kind_mismatch",
        ),
        (
            host.extension_build_id.as_deref() != Some(super::EXPECTED_EXTENSION_BUILD_ID),
            "loaded_worker_outdated",
        ),
        (!worker_verified, "worker_integrity_unverified"),
        (
            super::code_stamp::code_stamp_stale_reason(
                host.extension_code_stamp.as_deref(),
                host.expected_code_stamp.as_deref(),
            )
            .is_some(),
            "worker_code_outdated",
        ),
        (
            expected.debugger_api_expected && host.extension_debugger_api_available != Some(true),
            "debugger_api_unavailable",
        ),
        (
            missing.iter().any(|name| name == "alarmReconnect"),
            "missing_capabilities=alarmReconnect",
        ),
        (
            missing.iter().any(|name| name != "alarmReconnect"),
            "required_capabilities_missing",
        ),
    ] {
        if present {
            causes.push(cause);
        }
    }
    causes.join("|")
}

pub(super) fn project(
    mut health: SubsystemHealth,
    host: Option<&ChromeBridgeHealthRecord>,
    install: InstallFacts,
    counts: (usize, usize, usize),
    repair_deferred: bool,
    reload_recorded: bool,
) -> SubsystemHealth {
    let mut detail = format!(
        "tab_control_available={} host_count={} queued_count={} pending_count={} repair_deferred={repair_deferred} stale_code_reload_recorded={reload_recorded}",
        health.status == "ok",
        counts.0,
        counts.1,
        counts.2
    );
    if let Some(host) = host {
        let causes = public_causes(host);
        let capabilities = super::REQUIRED_DIRECT_HTTP_CAPABILITIES
            .iter()
            .copied()
            .filter(|name| host.extension_capabilities.contains(*name))
            .collect::<Vec<_>>()
            .join(",");
        detail.push_str(&format!(
            " extension_stale={} extension_capabilities={capabilities}",
            !causes.is_empty()
        ));
        if !causes.is_empty() {
            detail.push_str(&format!(
                " extension_stale_reasons={causes}; install the bundled Synapse Chrome extension"
            ));
        }
        if let Some(disconnect) = &host.last_disconnect_detail {
            let cause = if disconnect == "websocket_closed" {
                "websocket_closed"
            } else {
                "bridge_disconnected"
            };
            detail.push_str(&format!(
                " last_disconnect_detail={cause}; wait for bridge registration and re-read health"
            ));
        }
        if health.status == "unsafe_profile" {
            detail.push_str(" reason=unsafe_browser_environment");
        }
    } else {
        detail.push_str(&format!(
            " reason=no_active_chrome_bridge_host synapse_chrome_bridge_profile_installation scanned={} installed={} active_profile_installed={}",
            install.scanned, bool_name(install.installed), bool_name(install.active_profile_installed)
        ));
        if install.installed == Some(false) || install.active_profile_installed == Some(false) {
            detail.push_str("; repair_guidance=install the bundled Synapse Chrome extension");
        } else {
            detail.push_str("; repair_guidance=use the already-open authenticated Chrome profile only; wait for the installed bridge worker alarmReconnect registration and re-read health");
        }
    }
    health.detail = Some(detail);
    health
}

#[cfg(test)]
thread_local! {
    static HEALTH_CASE: std::cell::RefCell<Option<&'static str>> = const { std::cell::RefCell::new(None) };
}

/// Scoped override, restored even when the public-health assertion unwinds.
#[cfg(test)]
pub(crate) fn with_health_case_for_test<T>(case: &str, f: impl FnOnce(SubsystemHealth) -> T) -> T {
    let case = match case {
        "ready" => "ready",
        "absent" => "absent",
        "disconnected" => "disconnected",
        "stale" => "stale",
        _ => "error",
    };
    struct Restore(Option<&'static str>);
    impl Drop for Restore {
        fn drop(&mut self) {
            HEALTH_CASE.with(|slot| *slot.borrow_mut() = self.0.take());
        }
    }
    let _restore = Restore(HEALTH_CASE.with(|slot| slot.replace(Some(case))));
    f(case_health(case, true))
}

#[cfg(test)]
pub(super) fn fixture_health(detailed: bool) -> Option<SubsystemHealth> {
    HEALTH_CASE.with(|slot| slot.borrow().map(|case| case_health(case, detailed)))
}

#[cfg(test)]
fn case_health(case: &str, detailed: bool) -> SubsystemHealth {
    if case == "error" {
        return lock_error(detailed);
    }
    let mut host = fixture_host();
    if case == "stale" {
        host.extension_capabilities.remove("alarmReconnect");
    }
    if case == "disconnected" {
        host.last_disconnect_detail = Some("synthetic-disconnected-82".to_owned());
    }
    let active_host = (case != "absent").then_some(&host);
    let install = super::SynapseChromeProfileInstallState::not_scanned("fixture");
    let shield = super::SynapseChromeSelfPolicyShieldStatus {
        present: true,
        detail: String::new(),
    };
    let health = super::chrome_bridge_health_from_snapshot_with_self_policy(
        active_host,
        usize::from(active_host.is_some()),
        0,
        0,
        &[],
        &[],
        &[],
        &shield,
        &install,
    );
    if detailed {
        health
    } else {
        project(
            health,
            active_host,
            InstallFacts::default(),
            (usize::from(active_host.is_some()), 0, 0),
            false,
            false,
        )
    }
}

#[cfg(test)]
fn fixture_host() -> ChromeBridgeHealthRecord {
    ChromeBridgeHealthRecord {
        host_id: "synthetic-82".to_owned(),
        browser_kind: super::BrowserKindId::legacy_default(),
        extension_browser_kind: None,
        origin: format!("chrome-extension://{}/", super::EXTENSION_ID),
        extension_id: Some(super::EXTENSION_ID.to_owned()),
        extension_version: None,
        extension_protocol_version: Some(1),
        extension_build_id: Some(super::EXPECTED_EXTENSION_BUILD_ID.to_owned()),
        extension_build_sha256: None,
        extension_declared_build_sha256: None,
        extension_service_worker_sha256: Some("11".repeat(32)),
        extension_service_worker_sha256_status: Some("ok".to_owned()),
        extension_service_worker_sha256_source: None,
        extension_service_worker_byte_length: None,
        extension_service_worker_sha256_error: None,
        expected_service_worker_sha256: Some("11".repeat(32)),
        expected_service_worker_path: None,
        extension_code_stamp: None,
        expected_code_stamp: None,
        extension_capabilities: super::REQUIRED_DIRECT_HTTP_CAPABILITIES
            .iter()
            .map(|name| (*name).to_owned())
            .collect(),
        extension_user_agent: None,
        extension_debugger_api_available: Some(true),
        extension_popup_risk_suppression: None,
        pid: 0,
        parent_window: None,
        transport: Some("direct_http".to_owned()),
        registered_unix_ms: 0,
        last_seen_unix_ms: 0,
        last_disconnect_detail: None,
        last_detach_reason: None,
    }
}
