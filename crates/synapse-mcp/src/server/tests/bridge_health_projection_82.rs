//! #82 owner contract 2026-10-15. Wire under tool_profiles for private fixture writer.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use crate::server::SynapseService;
use synapse_core::SubsystemHealth;

// Independent public vocabulary: fixture/producer changes must not redefine the oracle.
const CAPABILITIES: &str = "alarmReconnect activateTab closeTab coordinateClick cookies downloads domAction externalPopupRiskSuppression frameLocators frames listTabs navigateTab openTab pageVitals pageContent pageScreenshot pagePdf setContent storageState ariaSnapshot assertPoll locateElements inspectElement scrollIntoView waitForText waitForFunction waitForLoadState waitForUrl waitForRequest waitForResponse waitForSelector clock pageEvents evaluateScript initScript exposeBinding handleDialog fileUpload cdpInput viewportEmulation deviceEmulation geolocationEmulation localeEmulation mediaEmulation networkConditions maintenancePauseReconnect reloadSelf targetInfo targetInfoPageText typeActiveElement setFieldValue";

const CASES: [(&str, &str, &str); 5] = [
    ("ready", "ok", "tab_control_available=true"),
    (
        "absent",
        "unavailable",
        "reason=no_active_chrome_bridge_host",
    ),
    (
        "disconnected",
        "unavailable",
        "last_disconnect_detail=bridge_disconnected",
    ),
    ("stale", "stale", "missing_capabilities=alarmReconnect"),
    ("error", "error", "chrome_bridge_state_lock_poisoned"),
];

fn fixture() -> (tempfile::TempDir, SynapseService) {
    let home = tempfile::tempdir().expect("home");
    let service = SynapseService::try_new_isolated_for_tests(home.path()).expect("service");
    for (session, profile) in [
        ("normal-82", super::ToolProfileKind::NormalAgent),
        ("debugger-82", super::ToolProfileKind::BrowserDebugger),
    ] {
        service
            .write_tool_profile_assignment(session, profile, "test-82", None, None)
            .expect("profile fixture");
        assert_eq!(
            service
                .tool_profile_snapshot(Some(session))
                .expect("readback")
                .profile,
            profile
        );
    }
    (home, service)
}

fn row(service: &SynapseService, session: Option<&str>) -> SubsystemHealth {
    service.health_payload_for_session(session).subsystems["chrome_bridge"].clone()
}

// Structural grammar: optional facts and ordering may vary; unknown keys,
// unbounded/free-form values, private diagnostics and escalation instructions fail.
fn assert_allowlist(row: &SubsystemHealth) {
    let value = serde_json::to_value(row).expect("row");
    for key in value.as_object().expect("object").keys() {
        assert!(
            matches!(key.as_str(), "status" | "detail"),
            "unknown field {key}"
        );
    }
    let detail = row.detail.as_deref().expect("cause required");
    assert!(detail.len() <= 1024, "bounded repair advice");
    let mut facts = detail.to_owned();
    for advice in [
        "install the bundled Synapse Chrome extension",
        "wait for bridge registration and re-read health",
        "use the already-open authenticated Chrome profile only; wait for the installed bridge worker alarmReconnect registration and re-read health",
    ] {
        facts = facts.replace(advice, "");
    }
    for token in facts
        .split(|c: char| c.is_whitespace() || c == ';')
        .filter(|s| !s.is_empty())
    {
        let literal = matches!(
            token,
            "chrome_bridge_state_lock_poisoned"
                | "synapse_chrome_bridge_profile_installation"
                | "repair_guidance="
                | "reason=no_active_chrome_bridge_host"
                | "reason=unsafe_browser_environment"
                | "last_disconnect_detail=bridge_disconnected"
                | "last_disconnect_detail=websocket_closed"
                | "extension_stale=true"
        );
        let typed = token.split_once('=').is_some_and(|(key, value)| match key {
            "host_count" | "queued_count" | "pending_count" => value.parse::<usize>().is_ok(),
            "tab_control_available"
            | "extension_stale"
            | "repair_deferred"
            | "stale_code_reload_recorded"
            | "scanned" => matches!(value, "true" | "false"),
            "capabilities" | "extension_capabilities" | "required_extension_capabilities" => value
                .split(',')
                .all(|name| CAPABILITIES.split_whitespace().any(|known| known == name)),
            "installed" | "active_profile_installed" => {
                matches!(value, "true" | "false" | "unknown")
            }
            "extension_stale_reasons" => value.split('|').all(|cause| {
                matches!(
                    cause,
                    "bridge_identity_mismatch"
                        | "browser_kind_mismatch"
                        | "loaded_worker_outdated"
                        | "worker_integrity_unverified"
                        | "worker_code_outdated"
                        | "debugger_api_unavailable"
                        | "missing_capabilities=alarmReconnect"
                        | "required_capabilities_missing"
                )
            }),
            _ => false,
        });
        assert!(
            literal || typed,
            "unallowlisted public detail token: {token}"
        );
    }
}

#[test]
fn must_pass_short_summary_preserves_explicit_staleness_and_fixed_capability_names() {
    let (_home, service) = fixture();
    for (case, stale) in [("ready", false), ("stale", true)] {
        crate::chrome_debugger_bridge::with_health_case_for_test(case, |_| {
            for session in [None, Some("normal-82")] {
                let public = row(&service, session);
                assert_allowlist(&public);
                let detail = public.detail.as_deref().expect("summary");
                assert!(
                    detail
                        .split_whitespace()
                        .any(|token| token.trim_end_matches(';')
                            == format!("extension_stale={stale}")),
                    "explicit stale fact missing: {detail}"
                );
                let capabilities = detail
                    .split_whitespace()
                    .find_map(|token| {
                        token
                            .strip_prefix("extension_capabilities=")
                            .or_else(|| token.strip_prefix("capabilities="))
                    })
                    .expect("short summary must advertise capabilities");
                let actual: std::collections::BTreeSet<_> =
                    capabilities.trim_end_matches(';').split(',').collect();
                let expected: std::collections::BTreeSet<_> = CAPABILITIES
                    .split_whitespace()
                    .filter(|name| case != "stale" || *name != "alarmReconnect")
                    .collect();
                assert_eq!(actual, expected, "capability vocabulary/readiness: {case}");
            }
        });
    }
}

#[test]
fn must_pass_normal_and_sessionless_preserve_status_cause_counters_and_bounded_advice() {
    let (_home, service) = fixture();
    for (case, status, cause) in CASES {
        crate::chrome_debugger_bridge::with_health_case_for_test(case, |raw| {
            assert_eq!(raw.status, status, "producer precondition: {case}");
            let raw_cause = if case == "disconnected" {
                "last_disconnect_detail=synthetic-disconnected-82"
            } else {
                cause
            };
            assert!(
                raw.detail
                    .as_deref()
                    .expect("raw cause")
                    .contains(raw_cause),
                "producer cause precondition: {case}"
            );
            for session in [None, Some("normal-82")] {
                let public = row(&service, session);
                assert_eq!(public.status, raw.status);
                let detail = public.detail.as_deref().expect("detail");
                assert!(detail.contains(cause), "{case}: {detail}");
                let advice = match case {
                    "absent" => Some("re-read health"),
                    "disconnected" => Some("wait for bridge registration and re-read health"),
                    "stale" => Some("install the bundled Synapse Chrome extension"),
                    _ => None,
                };
                if let Some(advice) = advice {
                    assert!(detail.contains(advice), "lost repair advice");
                }
                if case != "error" {
                    for fact in [
                        format!("host_count={}", usize::from(case != "absent")),
                        "queued_count=0".into(),
                        "pending_count=0".into(),
                    ] {
                        assert!(
                            raw.detail.as_deref().expect("raw").contains(&fact),
                            "fixture: {fact}"
                        );
                        assert!(detail.contains(&fact), "lost counter: {fact}");
                    }
                }
                assert_allowlist(&public);
                assert_eq!(
                    row(&service, session),
                    public,
                    "read must not mutate snapshot"
                );
            }
        });
    }
}

#[test]
fn must_fail_private_and_unknown_fields_in_normal_and_sessionless_health() {
    let (_home, service) = fixture();
    for (case, _, _) in CASES {
        crate::chrome_debugger_bridge::with_health_case_for_test(case, |raw| {
            let private = raw.detail.as_deref().expect("raw");
            assert!(
                private.contains("expected_extension_id="),
                "non-vacuous private identity"
            );
            if case == "ready" {
                for evidence in [
                    "extension_id=",
                    "endpoint=",
                    "active_host_id=synthetic-82",
                    "pid=",
                    "registered_unix_ms=",
                    "last_seen_unix_ms=",
                    "extension_service_worker_sha256=",
                    "extension_user_agent=",
                    "scripts\\install-synapse-chrome-debugger.ps1",
                    "profile=browser_debugger",
                    "confirm_break_glass=true",
                ] {
                    assert!(
                        private.contains(evidence),
                        "raw private precondition: {evidence}"
                    );
                }
            }
            let private_tokens: Vec<_> = private
                .split_whitespace()
                .filter(|token| {
                    token.split_once('=').is_some_and(|(key, _)| {
                        matches!(
                            key,
                            "expected_extension_id"
                                | "extension_id"
                                | "endpoint"
                                | "active_host_id"
                                | "pid"
                                | "registered_unix_ms"
                                | "last_seen_unix_ms"
                                | "expected_extension_service_worker_path"
                                | "extension_service_worker_sha256"
                                | "profile"
                                | "user_data_dir"
                                | "extension_code_stamp"
                                | "expected_code_stamp"
                                | "confirm_break_glass"
                                | "extension_user_agent"
                        )
                    })
                })
                .collect();
            assert!(!private_tokens.is_empty(), "private fixture");
            for session in [None, Some("normal-82")] {
                let public = row(&service, session);
                assert_allowlist(&public);
                for path in ["scripts\\install-synapse-chrome-debugger.ps1"] {
                    if private.contains(path) {
                        assert!(
                            !public.detail.as_deref().expect("public").contains(path),
                            "leaked raw path"
                        );
                    }
                }
                for token in &private_tokens {
                    assert!(private.contains(token), "raw precondition");
                    assert!(
                        !public.detail.as_deref().expect("public").contains(token),
                        "leaked {token}"
                    );
                }
            }
        });
    }
}

#[test]
fn must_pass_debugger_is_byte_for_byte_raw_in_all_five_states() {
    let (_home, service) = fixture();
    for (case, status, _) in CASES {
        crate::chrome_debugger_bridge::with_health_case_for_test(case, |raw| {
            assert_eq!(raw.status, status, "producer precondition");
            assert_eq!(
                serde_json::to_vec(&row(&service, Some("debugger-82"))).expect("debugger"),
                serde_json::to_vec(&raw).expect("producer"),
                "{case}"
            );
        });
    }
}

#[test]
fn must_pass_seam_restores_outer_case_after_unwind() {
    let (_home, service) = fixture();
    crate::chrome_debugger_bridge::with_health_case_for_test("absent", |outer| {
        assert_eq!(outer.status, "unavailable");
        let before = row(&service, Some("debugger-82"));
        assert_eq!(before, outer);
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::chrome_debugger_bridge::with_health_case_for_test("ready", |inner| {
                assert_eq!(inner.status, "ok");
                assert_eq!(row(&service, Some("debugger-82")), inner);
                panic!("synthetic-82-unwind");
            });
        }));
        let payload = panic.expect_err("must unwind");
        assert_eq!(payload.downcast_ref::<&str>(), Some(&"synthetic-82-unwind"));
        assert_eq!(row(&service, Some("debugger-82")), before, "restored");
    });
}
