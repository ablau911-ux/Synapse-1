//! #202: claim conflicts must follow the window hierarchy, not only the exact
//! key string: window vs tab, and child control vs top-level root HWND.
use std::collections::BTreeSet;

use super::{SessionTarget, TargetClaimRegistry, canonical_claim_target_with};

const ROOT: i64 = 0x100;
const CHILD: i64 = 0x200;
const OTHER_ROOT: i64 = 0x300;

/// Fake GA_ROOT: CHILD lives inside ROOT; everything else is its own root.
fn root_of(hwnd: i64) -> i64 {
    if hwnd == CHILD { ROOT } else { hwnd }
}

fn live() -> BTreeSet<String> {
    BTreeSet::from(["a".to_owned(), "b".to_owned()])
}

fn window(hwnd: i64) -> SessionTarget {
    SessionTarget::Window { hwnd }
}

fn cdp(window_hwnd: i64, id: &str) -> SessionTarget {
    SessionTarget::Cdp {
        window_hwnd,
        cdp_target_id: id.to_owned(),
    }
}

fn registry_with_claim(target: SessionTarget) -> TargetClaimRegistry {
    let mut registry = TargetClaimRegistry::default();
    registry
        .claim_with_roots("a", target, 10_000, 1_000, &live(), &root_of)
        .expect("owner a claim");
    registry
}

#[test]
fn must_fail_window_claim_blocks_mutation_into_child_control() {
    let registry = registry_with_claim(window(ROOT));

    let conflict = registry.conflict_with_roots("b", &window(CHILD), &root_of);

    assert_eq!(
        conflict.map(|entry| entry.owner_session_id),
        Some("a".to_owned())
    );
}

#[test]
fn must_fail_child_claim_blocks_mutation_on_root_window() {
    let registry = registry_with_claim(window(CHILD));

    assert!(
        registry
            .conflict_with_roots("b", &window(ROOT), &root_of)
            .is_some()
    );
}

#[test]
fn must_fail_window_claim_blocks_cdp_verbs_on_tabs_inside_the_window() {
    let registry = registry_with_claim(window(ROOT));

    assert!(
        registry
            .conflict_with_roots("b", &cdp(ROOT, "tab-1"), &root_of)
            .is_some()
    );
    assert!(
        registry
            .conflict_with_roots("b", &cdp(CHILD, "tab-1"), &root_of)
            .is_some()
    );
}

#[test]
fn must_fail_cdp_claim_blocks_window_level_mutation_on_hosting_root() {
    let registry = registry_with_claim(cdp(ROOT, "tab-1"));

    assert!(
        registry
            .conflict_with_roots("b", &window(ROOT), &root_of)
            .is_some()
    );
    assert!(
        registry
            .conflict_with_roots("b", &window(CHILD), &root_of)
            .is_some()
    );
}

#[test]
fn must_fail_cdp_claim_blocks_same_tab_even_through_a_child_hwnd() {
    let registry = registry_with_claim(cdp(ROOT, "tab-1"));

    assert!(
        registry
            .conflict_with_roots("b", &cdp(CHILD, "tab-1"), &root_of)
            .is_some()
    );
}

#[test]
fn must_pass_cdp_claim_does_not_block_a_different_tab() {
    let registry = registry_with_claim(cdp(ROOT, "tab-1"));

    assert!(
        registry
            .conflict_with_roots("b", &cdp(ROOT, "tab-2"), &root_of)
            .is_none()
    );
}

#[test]
fn must_pass_unrelated_window_is_never_blocked() {
    let registry = registry_with_claim(window(ROOT));

    assert!(
        registry
            .conflict_with_roots("b", &window(OTHER_ROOT), &root_of)
            .is_none()
    );
    assert!(
        registry
            .conflict_with_roots("b", &cdp(OTHER_ROOT, "tab-1"), &root_of)
            .is_none()
    );
}

#[test]
fn must_pass_owner_never_conflicts_with_its_own_claims() {
    let registry = registry_with_claim(window(ROOT));

    assert!(
        registry
            .conflict_with_roots("a", &window(CHILD), &root_of)
            .is_none()
    );
    assert!(
        registry
            .conflict_with_roots("a", &cdp(ROOT, "tab-1"), &root_of)
            .is_none()
    );
}

#[test]
fn must_fail_claiming_an_overlapping_target_of_a_live_peer_is_refused() {
    let mut registry = registry_with_claim(window(ROOT));

    let conflict = registry
        .claim_with_roots("b", cdp(ROOT, "tab-1"), 10_000, 1_001, &live(), &root_of)
        .expect_err("cdp claim inside a peer's window claim must conflict");
    assert_eq!(conflict.entry.owner_session_id, "a");

    let child = registry.claim_with_roots("b", window(CHILD), 10_000, 1_002, &live(), &root_of);
    assert!(
        child.is_err(),
        "child HWND claim must conflict with the root claim"
    );
}

#[test]
fn must_pass_claiming_over_a_dead_peers_overlapping_claim_succeeds() {
    let mut registry = registry_with_claim(window(ROOT));
    let only_b = BTreeSet::from(["b".to_owned()]);

    let claimed = registry
        .claim_with_roots("b", window(CHILD), 10_000, 1_001, &only_b, &root_of)
        .expect("a non-live owner must not block");

    assert_eq!(claimed.0.owner_session_id, "b");
}

#[test]
fn must_pass_claim_targets_are_canonicalized_to_the_top_level_root() {
    assert_eq!(
        canonical_claim_target_with(window(CHILD), &root_of),
        window(ROOT)
    );
    assert_eq!(
        canonical_claim_target_with(cdp(CHILD, "tab-1"), &root_of),
        cdp(ROOT, "tab-1")
    );
    assert_eq!(
        canonical_claim_target_with(window(ROOT), &root_of),
        window(ROOT)
    );
}

#[test]
fn must_pass_tab_claim_owner_keeps_window_level_and_web_element_actions() {
    let registry = registry_with_claim(cdp(ROOT, "tab-1"));

    // Owner "a": window-level action, child-HWND web-element click, other tab.
    assert!(
        registry
            .conflict_with_roots("a", &window(ROOT), &root_of)
            .is_none()
    );
    assert!(
        registry
            .conflict_with_roots("a", &window(CHILD), &root_of)
            .is_none()
    );
    assert!(
        registry
            .conflict_with_roots("a", &cdp(ROOT, "tab-2"), &root_of)
            .is_none()
    );
    // Another session is still blocked on the same window-level action.
    assert!(
        registry
            .conflict_with_roots("b", &window(CHILD), &root_of)
            .is_some()
    );
}

#[test]
fn must_pass_web_element_action_in_own_tab_is_not_blocked_by_a_peer_tab() {
    // a holds tab T1; b acts on a web element in its own tab T2 (same window).
    let registry = registry_with_claim(cdp(ROOT, "T1"));
    assert!(
        registry
            .conflict_with_roots("b", &cdp(CHILD, "T2"), &root_of)
            .is_none()
    );
}

#[test]
fn must_fail_web_element_action_in_a_peer_tab_is_blocked_whatever_the_id_case() {
    let registry = registry_with_claim(cdp(ROOT, "abcdef0123"));
    assert!(
        registry
            .conflict_with_roots("b", &cdp(CHILD, "ABCDEF0123"), &root_of)
            .is_some()
    );
}
