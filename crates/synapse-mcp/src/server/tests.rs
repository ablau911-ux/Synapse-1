use super::{SessionTarget, SynapseService, explicit_action_target};

#[test]
fn must_pass_explicit_action_target_prefers_window_then_cdp() {
    // #984B: explicit per-call routing maps to the right SessionTarget so
    // multi-window/multi-agent action routing is deterministic.
    assert_eq!(
        explicit_action_target(None, None).expect("no override is allowed"),
        None,
        "no explicit params => fall back to the session target"
    );
    assert_eq!(
        explicit_action_target(Some(0x1234), None).expect("window override is valid"),
        Some(SessionTarget::Window { hwnd: 0x1234 }),
    );
    assert_eq!(
        explicit_action_target(Some(0x1234), Some("  ABC123  ")).expect("cdp override is valid"),
        Some(SessionTarget::Cdp {
            window_hwnd: 0x1234,
            cdp_target_id: "ABC123".to_owned(),
        }),
        "cdp_target_id is trimmed and paired with the window hwnd"
    );
    // Whitespace-only cdp_target_id is treated as absent, not as a CDP target.
    assert_eq!(
        explicit_action_target(Some(7), Some("   ")).expect("blank cdp id ignored"),
        Some(SessionTarget::Window { hwnd: 7 }),
    );
}

#[test]
fn must_fail_explicit_action_target_rejects_orphan_cdp() {
    let err = explicit_action_target(None, Some("ABC"))
        .expect_err("cdp_target_id without window_hwnd must be rejected");
    let code = err
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(serde_json::Value::as_str);
    assert_eq!(code, Some(synapse_core::error_codes::TOOL_PARAMS_INVALID));
}

#[test]
fn must_pass_health_payload_reports_m3_subsystems_initializing_or_disabled() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let service = SynapseService::try_new_isolated_for_tests(dir.path()).expect("isolated service");
    let payload = service.health_payload();
    assert!(payload.ok);
    assert_eq!(payload.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(payload.build, "dev");
    // Startup lease reconciliation opens storage, so it reports "ok" — and it
    // must be the isolated temp DB, never the production path (#143).
    assert_eq!(payload.subsystems["storage"].status, "ok");
    let expected_db = dir.path().join("db").display().to_string();
    assert_eq!(
        payload.subsystems["storage"].db_path.as_deref(),
        Some(expected_db.as_str())
    );
    assert_eq!(payload.subsystems["reflex"].status, "initializing");
    assert_eq!(payload.subsystems["profiles"].status, "initializing");
    assert!(!payload.subsystems.contains_key("hid_host"));
    assert_eq!(payload.subsystems["action"].status, "ok");
    assert_eq!(payload.subsystems["audio"].status, "disabled");
    assert_eq!(payload.subsystems["http"].status, "disabled");
}

/// `uptime_s` must be the elapsed time of the monotonic `started_at: Instant`
/// (a wall-clock jump cannot move it). Back-dating the start instant by 90 s
/// must show up as 90 s of uptime; a constant or a per-call clock does not.
#[test]
fn must_pass_uptime_is_the_elapsed_time_of_the_monotonic_start_instant() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let mut service =
        SynapseService::try_new_isolated_for_tests(dir.path()).expect("isolated service");
    service.started_at = std::time::Instant::now()
        .checked_sub(std::time::Duration::from_secs(90))
        .expect("monotonic clock is past 90 s");
    let uptime = service.health_payload().uptime_s;
    assert!(
        (90..100).contains(&uptime),
        "uptime_s={uptime} must track started_at.elapsed()"
    );
}

#[path = "tests/started_row_closed_198.rs"]
mod started_row_closed_198_tests;

#[path = "tests/rate_limit_reset_generation_199.rs"]
mod rate_limit_reset_generation_199_tests;

#[path = "tests/unclaimed_refusal_text_222.rs"]
mod unclaimed_refusal_text_222_tests;

#[path = "tests/terminated_capacity_ceiling_14.rs"]
mod terminated_capacity_ceiling_14_tests;

#[path = "tests/scope_transition_publish_135.rs"]
mod scope_transition_publish_135_tests;

#[path = "tests/livefeed_default_build_44.rs"]
mod livefeed_default_build_44_tests;

#[cfg(feature = "livefeed")]
#[path = "tests/livefeed_facade_44.rs"]
mod livefeed_facade_44_tests;

#[cfg(not(feature = "livefeed"))]
#[path = "tests/livefeed_feature_off_43.rs"]
mod livefeed_feature_off_43_tests;

#[path = "tests/ocr_cache_pressure_shed_270.rs"]
mod ocr_cache_pressure_shed_270_tests;

#[path = "tests/pressure_gate_inventory_270.rs"]
mod pressure_gate_inventory_270_tests;

#[cfg(windows)]
#[path = "tests/shell_handler_contracts_b2.rs"]
mod shell_handler_contracts_b2;

#[cfg(windows)]
#[path = "tests/shell_user_active_268.rs"]
mod shell_user_active_268;

#[path = "tests/author_b2_perception_audit.rs"]
mod author_b2_perception_audit;

#[cfg(windows)]
#[path = "tests/batch2_handler_contracts.rs"]
mod batch2_handler_contracts;

#[cfg(windows)]
#[path = "tests/browser_password_handlers_244_r2.rs"]
mod browser_password_handlers_244_r2;
