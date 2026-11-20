//! `/health` readback of the HTTP admission limits and counters (raid
//! 2026-10-15): #13 session cap + rejections, #14 terminated-session
//! tombstones, #17 connection admission + header timeouts, #20 in-flight
//! requests + duplicate rejections. Before the fix `http_health` reported only
//! `active_sessions` and `sse_subscribers`, so none of these bounds could be
//! verified from health. Drives the real health composition used by the
//! `/health` handler on an HTTP-mode service.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use synapse_core::{Health, SubsystemHealth};
use tokio_util::sync::CancellationToken;

use super::HttpHealthTelemetry;
use crate::server::SynapseService;

/// Isolated service whose M3 state runs in HTTP mode (same config as
/// `try_new_isolated_for_tests`, shutdown reason "http").
fn http_mode_service(root: &Path) -> SynapseService {
    SynapseService::try_with_m2_shutdown_reason_and_m3_config(
        CancellationToken::new(),
        "http",
        CancellationToken::new(),
        &crate::m2::M2ServiceConfig::default(),
        crate::m3::M3ServiceConfig::from_cli_parts(
            Some(root.join("db")),
            Some(root.to_path_buf()),
            false,
            "127.0.0.1:0".to_owned(),
            synapse_reflex::DEFAULT_MAX_SUBSCRIPTIONS_NONZERO,
            false,
            true,
            None,
            false,
            None,
        ),
        crate::m4::M4ServiceConfig::default(),
    )
    .expect("http-mode isolated service")
}

fn http_row(health: &Health) -> &SubsystemHealth {
    let row = health.subsystems.get("http").expect("http subsystem");
    assert_eq!(row.status, "ok", "precondition: service runs in HTTP mode");
    row
}

#[test]
fn must_pass_http_admission_telemetry_is_reported_verbatim() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let service = http_mode_service(dir.path());
    let telemetry = HttpHealthTelemetry {
        active_sessions: 5,
        max_sessions: 128,
        session_capacity_rejections_total: 7,
        active_connections: 3,
        connection_limit: 8,
        connection_admission_rejections_total: 56,
        header_read_timeouts_total: 9,
        inflight_requests: 4,
        duplicate_inflight_rejections_total: 2,
    };
    let health = service.health_payload_with_http_telemetry(telemetry);
    let http = http_row(&health);
    assert_eq!(http.active_sessions, Some(5));
    assert_eq!(http.max_sessions, Some(128), "#13 limit");
    assert_eq!(http.session_capacity_rejections_total, Some(7), "#13");
    assert_eq!(http.active_connections, Some(3), "#17");
    assert_eq!(http.connection_limit, Some(8), "#17");
    assert_eq!(http.connection_admission_rejections_total, Some(56), "#17");
    assert_eq!(http.header_read_timeouts_total, Some(9), "#17");
    assert_eq!(http.inflight_requests, Some(4), "#20");
    assert_eq!(http.duplicate_inflight_rejections_total, Some(2), "#20");
}

#[test]
fn must_pass_zero_counters_read_back_as_zero_not_absent() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let service = http_mode_service(dir.path());
    let telemetry = HttpHealthTelemetry {
        max_sessions: 1,
        connection_limit: 1,
        ..HttpHealthTelemetry::default()
    };
    let health = service.health_payload_with_http_telemetry(telemetry);
    let http = http_row(&health);
    assert_eq!(http.session_capacity_rejections_total, Some(0));
    assert_eq!(http.connection_admission_rejections_total, Some(0));
    assert_eq!(http.header_read_timeouts_total, Some(0));
    assert_eq!(http.inflight_requests, Some(0));
    assert_eq!(http.duplicate_inflight_rejections_total, Some(0));
}

#[test]
fn must_pass_14_tombstone_occupancy_limit_and_evictions_are_reported() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let service = http_mode_service(dir.path());
    let registry = service.terminated_sessions_handle();
    let capacity = registry.lock().unwrap().stats().capacity;
    assert_eq!(registry.lock().unwrap().len(), 0, "precondition: empty");
    {
        let mut terminated = registry.lock().unwrap();
        for index in 0..capacity + 3 {
            assert!(terminated.insert(format!("raid-14-{index}")));
        }
    }
    let health = service.health_payload_with_http_sessions(Some(0));
    let http = http_row(&health);
    assert_eq!(http.terminated_session_tombstones, Some(capacity));
    assert_eq!(http.terminated_session_tombstone_limit, Some(capacity));
    assert_eq!(http.terminated_session_tombstone_evictions_total, Some(3));
}

#[test]
fn must_fail_14_duplicate_termination_changes_neither_tombstones_nor_evictions() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let service = http_mode_service(dir.path());
    let registry = service.terminated_sessions_handle();
    assert!(registry.lock().unwrap().insert("raid-14-dup".to_owned()));
    let before = service.health_payload_with_http_sessions(Some(0));
    assert_eq!(http_row(&before).terminated_session_tombstones, Some(1));

    assert!(
        !registry.lock().unwrap().insert("raid-14-dup".to_owned()),
        "duplicate is refused as a no-op"
    );

    let after = service.health_payload_with_http_sessions(Some(0));
    let http = http_row(&after);
    assert_eq!(http.terminated_session_tombstones, Some(1));
    assert_eq!(http.terminated_session_tombstone_evictions_total, Some(0));
}
