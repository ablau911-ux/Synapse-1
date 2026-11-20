//! #201. Parent: target_claims. NEEDS-SEAM: own window + handoff pause.
//! No synthetic copied adoption logic. Invoke target_claim_adopt_for_session.
#![cfg(windows)]
use super::*;
use crate::server::g7a_support as f;

#[cfg(all(test, windows))]
#[path = "adoption_capacity_followup_201.rs"]
mod adoption_capacity_followup_201;

struct Fixture {
    _dir: tempfile::TempDir,
    svc: SynapseService,
    window: crate::test_support::OwnedClaimWindow,
}
fn dead_spawn(now: u64) -> crate::server::session_registry::SpawnedAgentRead {
    crate::server::session_registry::SpawnedAgentRead {
        spawn_id: "g7a201-dead".to_owned(),
        cli: "claude".to_owned(),
        launcher_process_id: u32::MAX,
        agent_process_id: Some(u32::MAX),
        started_by_session_id: None,
        launched_at_unix_ms: now - 900,
        launch_target: "test".to_owned(),
        log_dir: "test".to_owned(),
        template_id: None,
        template_version: None,
        control: None,
    }
}
fn fixture(dead_owner: bool) -> Fixture {
    let (dir, svc) = f::service();
    let window = crate::test_support::owned_claim_window().expect("own nonactivating window");
    assert!(synapse_a11y::foreground_context(window.hwnd()).is_ok());
    let now = f::now();
    f::seed(&svc, "g7a201-owner", now - 1000);
    f::seed(&svc, "g7a201-new", now);
    f::seed(&svc, "g7a201-racer", now);
    if dead_owner {
        assert!(!crate::m4::process_exists(u32::MAX));
        svc.session_registry_ref()
            .lock()
            .expect("registry")
            .record_spawned_agent("g7a201-owner", dead_spawn(now), now - 900);
    }
    let target = canonical_claim_target(SessionTarget::Window {
        hwnd: window.hwnd(),
    });
    let live = svc
        .live_target_claim_sessions(now, None)
        .expect("live sessions");
    let mut claims = svc.lock_target_claims().expect("claims");
    claims
        .claim("g7a201-owner", target, 600_000, now, &live)
        .expect("seed");
    claims.prune_inactive(f::now(), &live);
    assert_eq!(claims.reads(f::now()).len(), 1);
    drop(claims);
    Fixture {
        _dir: dir,
        svc,
        window,
    }
}
fn assert_adopted(result: Result<Json<TargetClaimAdoptResponse>, ErrorData>) {
    assert_eq!(
        result.expect("adopt").0.claim.owner_session_id,
        "g7a201-new"
    );
}
fn params(fx: &Fixture) -> TargetClaimAdoptParams {
    TargetClaimAdoptParams {
        owner_session_id: "g7a201-owner".to_owned(),
        target: Some(TargetClaimTargetParam::Window {
            window_hwnd: fx.window.hwnd(),
        }),
        ttl_ms: 600_000,
    }
}

#[tokio::test(flavor = "current_thread")]
async fn must_pass_dead_owner_adoption_returns_new_owner_and_closed_old_session() {
    let _lock = crate::test_support::lease_serial("g7a201");
    let fx = fixture(true);
    let response = fx
        .svc
        .target_claim_adopt_for_session("g7a201-new".to_owned(), params(&fx))
        .await
        .expect("adopt")
        .0;
    assert_eq!(response.outcome, "adopted");
    assert_eq!(response.claim.owner_session_id, "g7a201-new");
    assert_eq!(response.owner_teardown_report.failure_count, 0);
    let live = fx
        .svc
        .live_target_claim_sessions(f::now(), None)
        .expect("live");
    assert!(!live.contains("g7a201-owner"));
    assert_eq!(
        fx.svc.lock_target_claims().expect("claims").reads(f::now())[0].owner_session_id,
        "g7a201-new"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_shared_name_idle_owner_is_not_torn_down() {
    let _lock = crate::test_support::lease_serial("g7a201");
    let fx = fixture(false);
    let before = fx.svc.lock_target_claims().expect("claims").reads(f::now())[0].generation;
    let error = fx
        .svc
        .target_claim_adopt_for_session("g7a201-new".to_owned(), params(&fx))
        .await
        .err()
        .expect("no gone proof");
    assert_eq!(f::code(&error), error_codes::TARGET_CLAIM_ADOPT_REFUSED);
    assert!(
        fx.svc
            .live_target_claim_sessions(f::now(), None)
            .expect("live")
            .contains("g7a201-owner")
    );
    let claims = fx.svc.lock_target_claims().expect("claims").reads(f::now());
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].owner_session_id, "g7a201-owner");
    assert_eq!(claims[0].generation, before);
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_competing_claim_cannot_steal_adoption_handoff_after_teardown() {
    let _lock = crate::test_support::lease_serial("g7a201");
    let fx = fixture(true);
    // Proposed seam pauses AFTER teardown returns, BEFORE final attempt_claim.
    // A correct reservation must survive that entire destructive interval.
    let pause = adoption_handoff_pause_for_test(&fx.svc, "g7a201-owner");
    let adoption = fx
        .svc
        .target_claim_adopt_for_session("g7a201-new".to_owned(), params(&fx));
    let racer = async {
        pause.wait_until_entered().await;
        let now = f::now();
        let live = fx.svc.live_target_claim_sessions(now, None).expect("live");
        let target = canonical_claim_target(SessionTarget::Window {
            hwnd: fx.window.hwnd(),
        });
        let before = fx.svc.lock_target_claims().expect("claims").reads(now);
        let attempt = attempt_claim(
            &fx.svc.target_claims,
            "g7a201-racer",
            target,
            600_000,
            now,
            &live,
        )
        .expect("attempt");
        pause.resume(); // Always release before any assertion can unwind.
        let refusal = attempt
            .result
            .expect_err("adoption reserves the target through teardown");
        assert_eq!(
            f::code(&refusal.error("target_claim", "g7a201-racer", "target_claim")),
            error_codes::TARGET_CO_OWNED
        );
        let after = fx.svc.lock_target_claims().expect("claims").reads(f::now());
        assert_eq!(after.len(), before.len());
        assert_eq!(after[0].generation, before[0].generation);
    };
    let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        tokio::join!(adoption, racer)
    })
    .await
    .expect("bounded rendezvous");
    assert_adopted(result);
}
