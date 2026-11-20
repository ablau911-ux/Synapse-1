//! #269: `health` reports whether the content-digest key is persistent or
//! ephemeral (plus the reason code), so a degraded key is visible instead of
//! only a log line. Drives the real health composition used by the `health`
//! tool (`health_payload_for_session`) and the HTTP `/health` handler
//! (`health_payload_with_http_sessions`) through the process-wide key state.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use synapse_core::{Health, SubsystemHealth};

use crate::content_digest::{
    DigestKeyState, WARN_KEY_UNPROTECTED, WARN_KEY_UNREADABLE, active_digest_key_for_tests,
    force_digest_key_state_for_tests,
};
use crate::server::SynapseService;
use crate::test_support::{digest_key_state_serial, key_material_renderings};

const ROW: &str = "content_digest_key";

/// Clears the process-wide forced key state when the test ends.
struct ForcedKeyState;

impl Drop for ForcedKeyState {
    fn drop(&mut self) {
        force_digest_key_state_for_tests(None);
    }
}

fn force(state: DigestKeyState) -> ForcedKeyState {
    force_digest_key_state_for_tests(Some(state));
    ForcedKeyState
}

fn ephemeral(reason: &'static str) -> DigestKeyState {
    DigestKeyState::Ephemeral { reason }
}

/// Both entry points: the `health` tool and the HTTP `/health` handler.
fn payloads(service: &SynapseService) -> [(&'static str, Health); 2] {
    [
        ("health tool", service.health_payload_for_session(None)),
        (
            "http /health",
            service.health_payload_with_http_sessions(Some(0)),
        ),
    ]
}

fn row<'a>(health: &'a Health, entry: &str) -> &'a SubsystemHealth {
    health.subsystems.get(ROW).unwrap_or_else(|| {
        panic!(
            "{entry}: health must carry a `{ROW}` subsystem, found {:?}",
            health.subsystems.keys().collect::<Vec<_>>()
        )
    })
}

#[test]
fn must_fail_ephemeral_digest_key_is_visible_in_health_with_its_reason() {
    let _serial = digest_key_state_serial();
    let dir = tempfile::TempDir::new().expect("temp dir");
    let service = SynapseService::try_new_isolated_for_tests(dir.path()).expect("isolated service");
    for reason in [WARN_KEY_UNREADABLE, WARN_KEY_UNPROTECTED] {
        let _forced = force(ephemeral(reason));
        for (entry, health) in payloads(&service) {
            let row = row(&health, entry);
            assert_eq!(row.status, "ephemeral", "{entry}/{reason}");
            let detail = row.detail.as_deref().unwrap_or_default();
            assert!(
                detail.contains(reason),
                "{entry}: detail must carry the reason code {reason}, got {detail:?}"
            );
        }
    }
}

#[test]
fn must_fail_persistent_digest_key_is_reported_persistent_without_a_reason() {
    let _serial = digest_key_state_serial();
    let dir = tempfile::TempDir::new().expect("temp dir");
    let service = SynapseService::try_new_isolated_for_tests(dir.path()).expect("isolated service");
    let _forced = force(DigestKeyState::Persistent);
    for (entry, health) in payloads(&service) {
        let row = row(&health, entry);
        assert_eq!(row.status, "persistent", "{entry}");
        let detail = row.detail.as_deref().unwrap_or_default();
        for reason in [WARN_KEY_UNREADABLE, WARN_KEY_UNPROTECTED] {
            assert!(
                !detail.contains(reason),
                "{entry}: a persistent key carries no ephemeral reason, got {detail:?}"
            );
        }
    }
}

/// Guard (must NOT change): an ephemeral key is visibility, not a new failure
/// mode. It never turns the row or the overall payload into an error.
#[test]
fn must_pass_ephemeral_digest_key_does_not_degrade_overall_health() {
    let _serial = digest_key_state_serial();
    let dir = tempfile::TempDir::new().expect("temp dir");
    let service = SynapseService::try_new_isolated_for_tests(dir.path()).expect("isolated service");
    let baseline_ok = {
        let _forced = force(DigestKeyState::Persistent);
        service.health_payload_for_session(None).ok
    };
    let _forced = force(ephemeral(WARN_KEY_UNREADABLE));
    let health = service.health_payload_for_session(None);
    assert_ne!(row(&health, "health tool").status, "error");
    assert!(
        health.ok || !baseline_ok,
        "an ephemeral digest key must not flip an otherwise ok health payload"
    );
}

#[test]
fn must_fail_health_never_carries_key_material() {
    let _serial = digest_key_state_serial();
    let dir = tempfile::TempDir::new().expect("temp dir");
    let service = SynapseService::try_new_isolated_for_tests(dir.path()).expect("isolated service");
    let key = active_digest_key_for_tests();
    for state in [
        ephemeral(WARN_KEY_UNREADABLE),
        ephemeral(WARN_KEY_UNPROTECTED),
        DigestKeyState::Persistent,
    ] {
        let _forced = force(state);
        for (entry, health) in payloads(&service) {
            let _ = row(&health, entry);
            let json = serde_json::to_string(&health).expect("health serializes");
            for needle in key_material_renderings(&key) {
                assert!(
                    !json.contains(&needle),
                    "{entry}/{state:?}: health leaks digest key material {needle}"
                );
            }
        }
    }
}
