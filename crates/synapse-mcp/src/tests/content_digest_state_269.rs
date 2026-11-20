//! #269: the digest key's persistence state is observable. A key that is not
//! persistent (unreadable key file, DPAPI unavailable) used to degrade with
//! only a log line; the loader now reports `persistent` vs `ephemeral` plus a
//! reason code, and the key, its file and the digests do not change.
//! Child of the #136 digest tests: reuses their `XorWrap` / `BrokenWrap`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use crate::test_support::{digest_key_state_serial, key_material_renderings};

#[test]
fn must_fail_unreadable_key_file_is_reported_ephemeral_with_the_unreadable_reason() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join(KEY_FILE_NAME);
    fs::write(&path, b"garbage").expect("seed unreadable file");
    let (key, state) = load_or_create_key_with_state(&path, &XorWrap(1));
    assert_eq!(key.len(), KEY_LEN);
    assert_eq!(
        state,
        DigestKeyState::Ephemeral {
            reason: WARN_KEY_UNREADABLE
        }
    );
    assert_eq!(
        fs::read(&path).expect("still there"),
        b"garbage",
        "an unreadable key file is never overwritten"
    );
}

#[test]
fn must_fail_unavailable_dpapi_is_reported_ephemeral_with_the_unprotected_reason() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join(KEY_FILE_NAME);
    let (_key, state) = load_or_create_key_with_state(&path, &BrokenWrap);
    assert_eq!(
        state,
        DigestKeyState::Ephemeral {
            reason: WARN_KEY_UNPROTECTED
        }
    );
    assert!(!path.exists(), "an unprotected key is never persisted");
}

#[test]
fn must_fail_blob_from_another_account_is_reported_ephemeral_and_kept() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join(KEY_FILE_NAME);
    let (original, first) = load_or_create_key_with_state(&path, &XorWrap(7));
    assert_eq!(first, DigestKeyState::Persistent);
    let blob_before = fs::read(&path).expect("blob");
    let (foreign, state) = load_or_create_key_with_state(&path, &BrokenWrap);
    assert_ne!(foreign, original, "the foreign process cannot read the key");
    assert_eq!(
        state,
        DigestKeyState::Ephemeral {
            reason: WARN_KEY_UNREADABLE
        }
    );
    assert_eq!(fs::read(&path).expect("blob"), blob_before);
}

/// Guard: the happy path must keep reporting `persistent`, with the same key
/// the stateless loader returns, so digests and the key file do not change.
#[test]
fn must_pass_created_then_reloaded_key_is_persistent_and_identical() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("nested").join(KEY_FILE_NAME);
    let (created, first) = load_or_create_key_with_state(&path, &XorWrap(0x5A));
    assert_eq!(first, DigestKeyState::Persistent);
    let blob = fs::read(&path).expect("key file written");
    assert!(!blob.windows(KEY_LEN).any(|w| w == created.as_slice()));
    let (reloaded, second) = load_or_create_key_with_state(&path, &XorWrap(0x5A));
    assert_eq!(second, DigestKeyState::Persistent);
    assert_eq!(created, reloaded);
    assert_eq!(load_or_create_key(&path, &XorWrap(0x5A)), created);
    assert_eq!(fs::read(&path).expect("key file"), blob);
}

/// Edge: concurrent first use. Every loader, winner or loser, ends up with the
/// persisted key, so none of them may report `ephemeral`.
#[test]
fn must_pass_lost_race_loser_is_persistent_like_the_winner() {
    let dir = TempDir::new().expect("tempdir");
    let path = std::sync::Arc::new(dir.path().join(KEY_FILE_NAME));
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let (path, barrier) = (path.clone(), barrier.clone());
            std::thread::spawn(move || {
                barrier.wait();
                load_or_create_key_with_state(&path, &XorWrap(3))
            })
        })
        .collect();
    let outcomes: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().expect("thread"))
        .collect();
    assert!(outcomes.windows(2).all(|pair| pair[0].0 == pair[1].0));
    assert!(
        outcomes
            .iter()
            .all(|(_key, state)| *state == DigestKeyState::Persistent),
        "{outcomes:?}"
    );
}

#[test]
fn must_fail_reported_state_never_carries_key_bytes() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join(KEY_FILE_NAME);
    for fixture in [Fixture::Unreadable, Fixture::Unavailable, Fixture::Created] {
        let (key, state) = match fixture {
            Fixture::Unreadable => {
                fs::write(&path, b"garbage").expect("seed");
                load_or_create_key_with_state(&path, &XorWrap(1))
            }
            Fixture::Unavailable => {
                fs::remove_file(&path).expect("reset");
                load_or_create_key_with_state(&path, &BrokenWrap)
            }
            Fixture::Created => load_or_create_key_with_state(&path, &XorWrap(2)),
        };
        if !matches!(fixture, Fixture::Created) {
            assert!(
                matches!(state, DigestKeyState::Ephemeral { .. }),
                "{fixture:?}: precondition, the key is not persistent"
            );
        }
        let rendered = format!("{state:?}");
        for needle in key_material_renderings(&key) {
            assert!(
                !rendered.contains(&needle),
                "{fixture:?}: state leaks key material {needle}"
            );
        }
    }
}

#[derive(Debug)]
enum Fixture {
    Unreadable,
    Unavailable,
    Created,
}

/// The reported process state is what `digest_key_state` returns; the forced
/// override is the seam the health tests drive.
#[test]
fn must_pass_digest_key_state_reports_the_forced_state_and_clears() {
    let _serial = digest_key_state_serial();
    let forced = DigestKeyState::Ephemeral {
        reason: WARN_KEY_UNREADABLE,
    };
    force_digest_key_state_for_tests(Some(forced));
    assert_eq!(digest_key_state(), forced);
    force_digest_key_state_for_tests(Some(DigestKeyState::Persistent));
    assert_eq!(digest_key_state(), DigestKeyState::Persistent);
    force_digest_key_state_for_tests(None);
}

/// Guard (must NOT change): the key file location and name.
#[test]
fn must_pass_key_file_location_is_unchanged() {
    let _env = crate::test_support::host_env_serial();
    let path = default_key_path();
    assert_eq!(
        path.file_name().and_then(|n| n.to_str()),
        Some(KEY_FILE_NAME)
    );
    assert_eq!(KEY_FILE_NAME, "content-digest-key.dpapi");
    assert_eq!(
        path.parent(),
        Some(crate::m3::local_synapse_dir().as_path())
    );
}
