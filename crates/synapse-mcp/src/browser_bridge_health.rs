//! Health rows for every declared non-legacy browser (multi-browser bridge).
//!
//! `health.subsystems.<row.health_key>` per row. The legacy chrome row keeps
//! its unchanged `chrome_bridge` subsystem (read by `synapse-setup.ps1`). A
//! browser that is not installed reports `not_installed`; no status produced
//! here is ever `error`, so a missing optional browser never degrades overall
//! health.

use std::path::Path;

use synapse_browsers::{BrowserExecutableLocator, BrowserRow, LocatedExecutable};
use synapse_core::SubsystemHealth;

use super::registry_reader::WindowsRegistryReader;
use super::{
    ChromeBridgeHealthRecord, ChromeDebuggerBridge, bridge, bridge_identity_stale_reasons,
    format_capabilities, host_record_to_health_record_with_profile_state,
    profile_install_state_for,
};

/// `(health_key, health)` for every declared row except the legacy default.
pub(crate) fn browser_bridge_health_subsystems() -> Vec<(String, SubsystemHealth)> {
    let bridge = bridge();
    bridge
        .registry
        .rows()
        .iter()
        .filter(|row| !row.id.is_legacy_default())
        .map(|row| {
            let located = locate_executable(row);
            (
                row.health_key.clone(),
                browser_row_health(bridge, row, located.as_ref()),
            )
        })
        .collect()
}

fn locate_executable(row: &BrowserRow) -> Option<LocatedExecutable> {
    let is_file = |path: &Path| path.is_file();
    BrowserExecutableLocator::new(
        &WindowsRegistryReader,
        &synapse_browsers::process_env_var,
        &is_file,
    )
    .locate(row)
}

/// Health of one row given its located executable (`None` = not installed).
pub(super) fn browser_row_health(
    bridge: &ChromeDebuggerBridge,
    row: &BrowserRow,
    located: Option<&LocatedExecutable>,
) -> SubsystemHealth {
    let Some(located) = located else {
        return SubsystemHealth {
            status: "not_installed".to_owned(),
            detail: Some(format!(
                "browser_kind={} label={:?} tab_control_available=false reason=browser_executable_not_found source_of_truth=browsers/{}.json (app_paths_name, exe_candidates) remediation=install the browser{} then run scripts\\install-synapse-browser-bridge.ps1 -Browser {}",
                row.id,
                row.label,
                row.id,
                row.winget_id
                    .as_deref()
                    .map_or_else(String::new, |id| format!(" (winget install {id})")),
                row.id
            )),
            ..SubsystemHealth::default()
        };
    };
    let install = profile_install_state_for(Some(row));
    let active = bridge.inner.lock().ok().and_then(|inner| {
        let host_id = inner.active_hosts.active(&row.id)?;
        inner
            .hosts
            .get(host_id)
            .map(|host| host_record_to_health_record_with_profile_state(host_id, host, &install))
    });
    let prefix = format!(
        "browser_kind={} family={} executable={:?} executable_source_of_truth={:?} unsupported_capabilities={}",
        row.id,
        row.family.as_str(),
        located.path.display().to_string(),
        located.source_of_truth,
        if row.unsupported_capabilities.is_empty() {
            "<none>".to_owned()
        } else {
            row.unsupported_capabilities.join(",")
        }
    );
    let Some(host) = active else {
        return SubsystemHealth {
            status: "unavailable".to_owned(),
            detail: Some(format!(
                "{prefix} tab_control_available=false reason=no_active_bridge_host_for_kind {}",
                install.detail
            )),
            ..SubsystemHealth::default()
        };
    };
    active_host_health(&prefix, &host, &install.detail)
}

fn active_host_health(
    prefix: &str,
    host: &ChromeBridgeHealthRecord,
    install_detail: &str,
) -> SubsystemHealth {
    let stale_reasons = bridge_identity_stale_reasons(host);
    let status = if !stale_reasons.is_empty() {
        "stale"
    } else if host.last_disconnect_detail.is_some() {
        "unavailable"
    } else {
        "ok"
    };
    SubsystemHealth {
        status: status.to_owned(),
        detail: Some(format!(
            "{prefix} tab_control_available={} bridge_host_id={} transport={} origin={} extension_id={} extension_browser_kind={} extension_build_id={} extension_code_stamp={} extension_capabilities={} extension_stale_reasons={} last_disconnect_detail={} {install_detail}",
            status == "ok",
            host.host_id,
            host.transport.as_deref().unwrap_or("native_messaging"),
            host.origin,
            host.extension_id.as_deref().unwrap_or("not_seen_yet"),
            host.extension_browser_kind
                .as_deref()
                .unwrap_or("not_seen_yet"),
            host.extension_build_id.as_deref().unwrap_or("not_seen_yet"),
            host.extension_code_stamp
                .as_deref()
                .unwrap_or("not_seen_yet"),
            format_capabilities(&host.extension_capabilities),
            if stale_reasons.is_empty() {
                "none".to_owned()
            } else {
                stale_reasons.join("|")
            },
            host.last_disconnect_detail.as_deref().unwrap_or("")
        )),
        ..SubsystemHealth::default()
    }
}
