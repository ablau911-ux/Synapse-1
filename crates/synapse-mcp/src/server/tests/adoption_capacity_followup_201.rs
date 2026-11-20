//! #201: full requester must be refused before destructive owner teardown.
use super::*;

fn fill(fx: &Fixture, count: usize) {
    let now = f::now();
    let live = fx.svc.live_target_claim_sessions(now, None).unwrap();
    let mut registry = fx.svc.lock_target_claims().unwrap();
    for index in 0..count {
        registry
            .claim(
                "g7a201-new",
                SessionTarget::Window {
                    hwnd: 1_000_000 + index as i64,
                },
                600_000,
                now,
                &live,
            )
            .unwrap();
    }
    assert_eq!(
        registry
            .reads(now)
            .iter()
            .filter(|r| r.owner_session_id == "g7a201-new")
            .count(),
        count
    );
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_full_requester_preserves_old_owner_before_teardown_201() {
    let _lock = crate::test_support::lease_serial("g7a201");
    let fx = fixture(true);
    fill(&fx, MAX_TARGET_CLAIMS_PER_SESSION);
    let before = fx.svc.lock_target_claims().unwrap().reads(f::now());
    let error = fx
        .svc
        .target_claim_adopt_for_session("g7a201-new".into(), params(&fx))
        .await
        .err()
        .expect("capacity refusal precedes teardown");
    assert_eq!(f::code(&error), error_codes::TARGET_CLAIM_ADOPT_REFUSED);
    assert!(
        fx.svc
            .live_target_claim_sessions(f::now(), None)
            .unwrap()
            .contains("g7a201-owner")
    );
    let after = fx.svc.lock_target_claims().unwrap().reads(f::now());
    assert_eq!(after.len(), before.len());
    let old = before
        .iter()
        .find(|r| r.owner_session_id == "g7a201-owner")
        .unwrap();
    let retained = after
        .iter()
        .find(|r| r.target_key == old.target_key)
        .unwrap();
    assert_eq!(retained.owner_session_id, old.owner_session_id);
    assert_eq!(retained.generation, old.generation);
}

#[tokio::test(flavor = "current_thread")]
async fn must_pass_last_available_slot_allows_adoption_201() {
    let _lock = crate::test_support::lease_serial("g7a201");
    let fx = fixture(true);
    fill(&fx, MAX_TARGET_CLAIMS_PER_SESSION - 1);
    let response = fx
        .svc
        .target_claim_adopt_for_session("g7a201-new".into(), params(&fx))
        .await
        .unwrap()
        .0;
    assert_eq!(response.outcome, "adopted");
    assert_eq!(response.claim.owner_session_id, "g7a201-new");
    let rows = fx.svc.lock_target_claims().unwrap().reads(f::now());
    assert_eq!(
        rows.iter()
            .filter(|r| r.owner_session_id == "g7a201-new")
            .count(),
        MAX_TARGET_CLAIMS_PER_SESSION
    );
    assert!(
        !fx.svc
            .live_target_claim_sessions(f::now(), None)
            .unwrap()
            .contains("g7a201-owner")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_requester_cannot_spend_reserved_last_slot_during_teardown_201() {
    let _lock = crate::test_support::lease_serial("g7a201");
    let fx = fixture(true);
    fill(&fx, MAX_TARGET_CLAIMS_PER_SESSION - 1);
    let pause = adoption_handoff_pause_for_test(&fx.svc, "g7a201-owner");
    let adoption = fx
        .svc
        .target_claim_adopt_for_session("g7a201-new".into(), params(&fx));
    let contender = async {
        pause.wait_until_entered().await;
        let now = f::now();
        let live = fx.svc.live_target_claim_sessions(now, None).unwrap();
        let attempt = attempt_claim(
            &fx.svc.target_claims,
            "g7a201-new",
            SessionTarget::Window { hwnd: 2_000_000 },
            600_000,
            now,
            &live,
        )
        .unwrap();
        pause.resume();
        let error = attempt
            .result
            .expect_err("last slot belongs to adoption")
            .error("target_claim", "g7a201-new", "target_claim");
        assert_eq!(f::code(&error), error_codes::TARGET_CLAIM_LIMIT_REACHED);
        assert!(
            !attempt
                .claims_after
                .iter()
                .any(|r| r.target_key == "window:0x1e8480")
        );
    };
    let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        tokio::join!(adoption, contender)
    })
    .await
    .expect("bounded handoff");
    assert_adopted(result);
    assert_eq!(
        fx.svc
            .lock_target_claims()
            .unwrap()
            .reads(f::now())
            .iter()
            .filter(|r| r.owner_session_id == "g7a201-new")
            .count(),
        MAX_TARGET_CLAIMS_PER_SESSION
    );
}
