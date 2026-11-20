//! Regression tests for #52: startup purge of legacy plaintext `CF_OCR_CACHE`
//! rows must compact the column family so purged plaintext does not survive on
//! disk inside `.sst` files.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use synapse_core::SCHEMA_VERSION;
use synapse_storage::{Db, cf};

use super::*;

fn sst_files_bytes(db_dir: &Path) -> Vec<u8> {
    let mut combined = Vec::new();
    let entries = std::fs::read_dir(db_dir).expect("read db_dir");
    for entry in entries {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("sst") {
            combined.extend(std::fs::read(&path).expect("read sst"));
        }
    }
    combined
}

fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

#[cfg(windows)]
#[test]
fn must_pass_startup_purge_compacts_sst_so_legacy_plaintext_is_absent_on_disk() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db_dir = temp.path().join("db");
    let db = Db::open(&db_dir, SCHEMA_VERSION).expect("open db");

    let sealed_secret = b"SYNAPSE_TEST_SEALED_KEEP_52";
    let legacy_secret = b"SYNAPSE_TEST_LEGACY_PLAINTEXT_52";
    let garbage_secret = b"SYNAPSE_TEST_LEGACY_GARBAGE_52";

    let sealed = seal(br#"{"result":{"full_text":"SYNAPSE_TEST_SEALED_KEEP_52"}}"#).expect("seal");
    db.put_batch_pressure_bypass(
        cf::CF_OCR_CACHE,
        vec![
            (b"a-sealed".to_vec(), sealed.clone()),
            (
                b"b-legacy".to_vec(),
                br#"{"schema_version":1,"result":{"full_text":"SYNAPSE_TEST_LEGACY_PLAINTEXT_52"}}"#
                    .to_vec(),
            ),
            (
                b"c-garbage".to_vec(),
                b"not-json SYNAPSE_TEST_LEGACY_GARBAGE_52".to_vec(),
            ),
        ],
    )
    .expect("seed rows");
    db.compact_cf(cf::CF_OCR_CACHE).expect("flush seed to sst");

    let before_sst = sst_files_bytes(&db_dir);
    assert!(
        contains_subslice(&before_sst, legacy_secret),
        "precondition: legacy plaintext must reside in an .sst file before purge"
    );
    assert!(
        contains_subslice(&before_sst, garbage_secret),
        "precondition: legacy garbage must reside in an .sst file before purge"
    );
    assert!(
        !contains_subslice(&before_sst, sealed_secret),
        "precondition: DPAPI-sealed text must never appear in plaintext in .sst files"
    );

    assert_eq!(purge_legacy_plaintext_rows(&db).expect("purge"), 2);

    let remaining = db.scan_cf(cf::CF_OCR_CACHE).expect("scan after purge");
    assert_eq!(remaining, vec![(b"a-sealed".to_vec(), sealed)]);

    let after_sst = sst_files_bytes(&db_dir);
    assert!(
        !contains_subslice(&after_sst, legacy_secret),
        "purged legacy OCR plaintext must be physically removed from .sst files"
    );
    assert!(
        !contains_subslice(&after_sst, garbage_secret),
        "purged unsealed OCR bytes must be physically removed from .sst files"
    );
}

#[cfg(windows)]
#[test]
fn must_pass_exact_page_boundary_purge_still_compacts_sst() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db_dir = temp.path().join("db");
    let db = Db::open(&db_dir, SCHEMA_VERSION).expect("open db");

    let boundary_secret = b"SYNAPSE_TEST_PAGE_BOUNDARY_52";
    let rows: Vec<(Vec<u8>, Vec<u8>)> = (0..PURGE_PAGE_ROWS)
        .map(|index| {
            (
                format!("legacy-{index:04}").into_bytes(),
                format!(
                    r#"{{"schema_version":1,"idx":{index},"text":"SYNAPSE_TEST_PAGE_BOUNDARY_52"}}"#
                )
                .into_bytes(),
            )
        })
        .collect();
    db.put_batch_pressure_bypass(cf::CF_OCR_CACHE, rows)
        .expect("seed exact page");
    db.compact_cf(cf::CF_OCR_CACHE).expect("flush to sst");
    assert!(
        contains_subslice(&sst_files_bytes(&db_dir), boundary_secret),
        "precondition: page-boundary plaintext must be in .sst before purge"
    );

    assert_eq!(
        purge_legacy_plaintext_rows(&db).expect("purge exact page"),
        PURGE_PAGE_ROWS
    );
    assert!(db.scan_cf(cf::CF_OCR_CACHE).expect("scan").is_empty());
    assert!(
        !contains_subslice(&sst_files_bytes(&db_dir), boundary_secret),
        "exact page-boundary purge must still compact CF_OCR_CACHE"
    );
}

#[cfg(windows)]
#[test]
fn must_fail_unsealed_row_is_never_kept_while_sealed_only_store_is_untouched() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db_dir = temp.path().join("db");
    let db = Db::open(&db_dir, SCHEMA_VERSION).expect("open db");

    let sealed = seal_at(
        br#"{"result":{"full_text":"SYNAPSE_TEST_SEALED_ONLY_52"}}"#,
        now_ns(),
    )
    .expect("seal");
    db.put_batch_pressure_bypass(
        cf::CF_OCR_CACHE,
        vec![(b"only-sealed".to_vec(), sealed.clone())],
    )
    .expect("seed sealed");
    db.compact_cf(cf::CF_OCR_CACHE).expect("flush sealed");

    assert_eq!(purge_legacy_plaintext_rows(&db).expect("no-op purge"), 0);
    let rows = db.scan_cf(cf::CF_OCR_CACHE).expect("scan");
    assert_eq!(rows, vec![(b"only-sealed".to_vec(), sealed)]);
    assert!(
        !contains_subslice(&sst_files_bytes(&db_dir), b"SYNAPSE_TEST_SEALED_ONLY_52"),
        "sealed row plaintext must never appear in .sst files"
    );
}
