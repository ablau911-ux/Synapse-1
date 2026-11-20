//! Regression tests for #136: content digests are keyed and the key persists.

use sha2::{Digest, Sha256};
use tempfile::TempDir;

use super::*;

/// Reversible stand-in for DPAPI so key lifecycle paths are deterministic.
struct XorWrap(u8);

impl KeyWrap for XorWrap {
    fn wrap(&self, key: &[u8]) -> anyhow::Result<Vec<u8>> {
        let mut blob = vec![0xAB];
        blob.extend(key.iter().map(|byte| byte ^ self.0));
        Ok(blob)
    }

    fn unwrap_blob(&self, blob: &[u8]) -> anyhow::Result<Vec<u8>> {
        match blob.split_first() {
            Some((0xAB, rest)) => Ok(rest.iter().map(|byte| byte ^ self.0).collect()),
            _ => anyhow::bail!("foreign blob"),
        }
    }
}

struct BrokenWrap;

impl KeyWrap for BrokenWrap {
    fn wrap(&self, _key: &[u8]) -> anyhow::Result<Vec<u8>> {
        anyhow::bail!("DPAPI unavailable")
    }

    fn unwrap_blob(&self, _blob: &[u8]) -> anyhow::Result<Vec<u8>> {
        anyhow::bail!("DPAPI unavailable")
    }
}

fn plain_sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[test]
fn must_pass_keyed_digest_is_deterministic_and_not_the_plain_sha256() {
    let first = keyed_text_digest_hex("1234");
    assert_eq!(first, keyed_text_digest_hex("1234"));
    assert_ne!(first, keyed_text_digest_hex("1235"));
    assert_eq!(first.len(), 64);
    assert_ne!(first, plain_sha256_hex(b"1234"));
}

#[test]
fn must_fail_a_single_keystroke_digest_cannot_be_matched_by_plain_sha256_candidates() {
    let observed = keyed_text_digest_hex("a");
    for candidate in 0x20_u8..0x7f {
        let plain = plain_sha256_hex(&[candidate]);
        assert_ne!(
            observed, plain,
            "candidate {candidate} matched a plain hash"
        );
    }
}

#[test]
fn must_pass_key_is_created_persisted_wrapped_and_reloaded_identically() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("nested").join(KEY_FILE_NAME);
    let created = load_or_create_key(&path, &XorWrap(0x5A));
    let blob = fs::read(&path).expect("key file written");
    assert_ne!(&blob[1..], created.as_slice(), "key must not be stored raw");
    assert!(!blob.windows(KEY_LEN).any(|w| w == created.as_slice()));
    let reloaded = load_or_create_key(&path, &XorWrap(0x5A));
    assert_eq!(created, reloaded);
}

#[test]
fn must_fail_unreadable_blob_is_preserved_and_an_in_memory_key_is_used() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join(KEY_FILE_NAME);
    fs::write(&path, b"garbage").expect("seed unreadable file");
    let key = load_or_create_key(&path, &XorWrap(1));
    assert_eq!(key.len(), KEY_LEN);
    assert_eq!(fs::read(&path).expect("still there"), b"garbage");
}

#[test]
fn must_fail_blob_from_another_account_is_never_overwritten() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join(KEY_FILE_NAME);
    let original = load_or_create_key(&path, &XorWrap(7));
    let blob_before = fs::read(&path).expect("blob");
    let _foreign = load_or_create_key(&path, &BrokenWrap);
    assert_eq!(fs::read(&path).expect("blob"), blob_before);
    assert_eq!(load_or_create_key(&path, &XorWrap(7)), original);
}

#[test]
fn must_fail_concurrent_first_use_converges_on_one_key() {
    let dir = TempDir::new().expect("tempdir");
    let path = std::sync::Arc::new(dir.path().join(KEY_FILE_NAME));
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let (path, barrier) = (path.clone(), barrier.clone());
            std::thread::spawn(move || {
                barrier.wait();
                load_or_create_key(&path, &XorWrap(3))
            })
        })
        .collect();
    let keys: Vec<DigestKey> = handles
        .into_iter()
        .map(|handle| handle.join().expect("thread"))
        .collect();
    assert!(
        keys.windows(2).all(|pair| pair[0] == pair[1]),
        "keys diverged"
    );
    assert_eq!(load_or_create_key(&path, &XorWrap(3)), keys[0]);
    let leftovers = fs::read_dir(dir.path())
        .expect("dir")
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
        .count();
    assert_eq!(leftovers, 0, "temporary key files must be cleaned up");
}

#[test]
fn must_pass_hmac_matches_rfc_4231_test_case_2() {
    assert_eq!(
        crate::server::escalation::hmac_sha256_hex(b"Jefe", b"what do ya want for nothing?"),
        "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
    );
}

#[test]
fn must_fail_unavailable_wrap_falls_back_to_an_in_memory_key_without_writing_it() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join(KEY_FILE_NAME);
    let key = load_or_create_key(&path, &BrokenWrap);
    assert_eq!(key.len(), KEY_LEN);
    assert!(!path.exists(), "an unprotected key is never persisted");
}

#[cfg(windows)]
#[test]
fn must_pass_real_dpapi_wrap_round_trips_and_hides_the_key() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join(KEY_FILE_NAME);
    let key = load_or_create_key(&path, &DpapiKeyWrap);
    let blob = fs::read(&path).expect("key file written");
    assert!(!blob.windows(KEY_LEN).any(|w| w == key.as_slice()));
    assert_eq!(load_or_create_key(&path, &DpapiKeyWrap), key);
}

// Child module: reuses the XorWrap/BrokenWrap fixtures above.
#[path = "content_digest_state_269.rs"]
mod content_digest_state_269;
