//! #206. Parent: target_claims. NEEDS-SEAM: trusted RequestContext fixture.
#![cfg(windows)]
use super::*;
use crate::server::g7a_support as f;

fn fixture() -> (tempfile::TempDir, SynapseService, i64, Vec<TargetClaimRead>) {
    let (dir, svc) = f::service();
    f::seed(&svc, "g7a206", f::now());
    let hwnd = (0..4096)
        .map(|i| 0xd17ee + i * 2)
        .find(|hwnd| synapse_a11y::foreground_context(*hwnd).is_err())
        .expect("verified dead HWND");
    let target = SessionTarget::Window { hwnd };
    let now = f::now();
    let live = BTreeSet::from(["g7a206".to_owned()]);
    let before = {
        let mut registry = svc.lock_target_claims().expect("claims");
        registry
            .claim("g7a206", target.clone(), 600_000, now, &live)
            .expect("seed");
        registry.prune_inactive(f::now(), &live);
        registry.reads(f::now())
    };
    assert_eq!(before.len(), 1);
    svc.session_targets_ref()
        .lock()
        .expect("targets")
        .insert("g7a206".to_owned(), target);
    assert_eq!(
        svc.session_target(Some("g7a206")).expect("active"),
        Some(SessionTarget::Window { hwnd })
    );
    (dir, svc, hwnd, before)
}

#[tokio::test]
async fn must_pass_omitted_target_status_returns_dead_active_target_and_held_claim() {
    let (_dir, svc, hwnd, before) = fixture();
    let context = crate::test_support::request_context_for_session("g7a206").await;
    let result = svc
        .target_claim_status(
            Parameters(TargetClaimStatusParams { target: None }),
            context,
        )
        .await
        .expect("status")
        .0;
    assert_eq!(result.session_id, "g7a206");
    assert_eq!(result.target_live, Some(false));
    assert_eq!(result.target_key, Some(format!("window:0x{hwnd:x}")));
    assert_eq!(result.claim_count, 1);
    let claim = result.target_claim.expect("held claim remains visible");
    assert_eq!(claim.owner_session_id, "g7a206");
    assert_eq!(claim.target_key, before[0].target_key);
    assert_eq!(claim.generation, before[0].generation);
    assert_eq!(claim.expires_at_unix_ms, before[0].expires_at_unix_ms);
    assert_eq!(
        svc.lock_target_claims()
            .expect("claims")
            .reads(f::now())
            .len(),
        1
    );
}

#[tokio::test]
async fn must_fail_zero_hwnd_status_preserves_claim_and_active_target() {
    let (_dir, svc, hwnd, _) = fixture();
    let before = serde_json::to_value(svc.lock_target_claims().expect("claims").reads(f::now()))
        .expect("before");
    let params = TargetClaimStatusParams {
        target: Some(TargetClaimTargetParam::Window { window_hwnd: 0 }),
    };
    let error = svc
        .target_claim_status(
            Parameters(params),
            crate::test_support::request_context_for_session("g7a206").await,
        )
        .await
        .err()
        .expect("zero HWND");
    assert_eq!(f::code(&error), error_codes::TOOL_PARAMS_INVALID);
    // Compare stable claim identity; ttl_remaining_ms naturally changes with time.
    let claims = svc.lock_target_claims().expect("claims").reads(f::now());
    assert_eq!(claims.len(), 1);
    assert_eq!(
        claims[0].target_key,
        before[0]["target_key"].as_str().expect("key")
    );
    assert_eq!(
        claims[0].generation,
        before[0]["generation"].as_u64().expect("generation")
    );
    assert_eq!(
        svc.session_target(Some("g7a206")).expect("active"),
        Some(SessionTarget::Window { hwnd })
    );
}
