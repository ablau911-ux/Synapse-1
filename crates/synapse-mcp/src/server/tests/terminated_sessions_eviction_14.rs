//! #14: the terminated-session registry must stay bounded under delete churn,
//! evict oldest-first, and treat a duplicate insert as an idempotent no-op.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::SharedTerminatedSessions;
use crate::server::terminated_sessions::TerminatedSessions;

const CHURN: usize = 100_000;

fn churned() -> SharedTerminatedSessions {
    let shared = SharedTerminatedSessions::default();
    {
        let mut terminated = shared.lock().unwrap();
        for index in 0..CHURN {
            terminated.insert(format!("s-{index}"));
        }
    }
    shared
}

#[test]
fn must_fail_registry_does_not_grow_with_delete_churn() {
    let shared = churned();
    let terminated = shared.lock().unwrap();
    let stats = terminated.stats();
    assert!(
        stats.capacity < CHURN,
        "precondition: churn must exceed the cap"
    );
    assert_eq!(terminated.len(), stats.capacity, "registry sits at its cap");
    assert_eq!(stats.len, stats.capacity);
    assert_eq!(
        stats.evicted_total,
        u64::try_from(CHURN - stats.capacity).unwrap(),
        "every id beyond the cap evicted exactly one older id"
    );
}

#[test]
fn must_pass_eviction_drops_oldest_and_keeps_newest() {
    let shared = churned();
    let terminated = shared.lock().unwrap();
    assert!(
        !terminated.contains("s-0"),
        "oldest id must have been evicted"
    );
    assert!(
        terminated.contains(&format!("s-{}", CHURN - 1)),
        "newest id must still be remembered"
    );
}

#[test]
fn must_fail_duplicate_insert_is_idempotent_and_does_not_refresh_age() {
    let shared = SharedTerminatedSessions::default();
    let mut terminated = shared.lock().unwrap();
    assert!(terminated.insert("dup".to_owned()));
    let before = terminated.len();
    assert!(!terminated.insert("dup".to_owned()));
    assert_eq!(terminated.len(), before);
    assert!(terminated.contains("dup"));
}

/// A duplicate insert must keep the id's ORIGINAL age: after re-inserting
/// "dup" behind newer ids, filling past the cap still evicts "dup" first.
#[test]
fn must_fail_duplicate_insert_does_not_move_the_id_to_the_newest_slot() {
    let mut terminated = TerminatedSessions::with_capacity(3);
    assert!(terminated.insert("dup".to_owned()));
    assert!(terminated.insert("a".to_owned()));
    assert!(!terminated.insert("dup".to_owned()), "duplicate is a no-op");
    assert!(terminated.insert("b".to_owned()));
    assert_eq!(terminated.len(), 3);

    assert!(terminated.insert("c".to_owned()));

    assert!(
        !terminated.contains("dup"),
        "dup kept its original (oldest) age and is evicted first"
    );
    for kept in ["a", "b", "c"] {
        assert!(terminated.contains(kept), "{kept} must still be remembered");
    }
    assert_eq!(terminated.stats().evicted_total, 1);
}
