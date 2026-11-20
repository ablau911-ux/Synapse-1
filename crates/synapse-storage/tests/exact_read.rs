use synapse_storage::{Db, cf};
use tempfile::TempDir;

#[test]
fn must_pass_get_cf_exact_returns_only_the_exact_key() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TempDir::new()?;
    let db = Db::open(&temp.path().join("db"), 1)?;
    db.put_batch(
        cf::CF_KV,
        [
            (b"row/1".to_vec(), b"short".to_vec()),
            (
                b"row/10".to_vec(),
                b"longer-key-with-shared-prefix".to_vec(),
            ),
        ],
    )?;
    assert_eq!(
        db.get_cf_exact(cf::CF_KV, b"row/1")?.as_deref(),
        Some(&b"short"[..])
    );
    assert_eq!(
        db.get_cf_exact(cf::CF_KV, b"row/10")?.as_deref(),
        Some(&b"longer-key-with-shared-prefix"[..])
    );
    Ok(())
}

#[test]
fn must_fail_prefix_absent_empty_and_unknown_cf_reads_return_nothing()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = TempDir::new()?;
    let db = Db::open(&temp.path().join("db"), 1)?;
    db.put_batch(cf::CF_KV, [(b"row/1".to_vec(), b"short".to_vec())])?;
    assert!(db.get_cf_exact(cf::CF_KV, b"row/")?.is_none());
    assert!(db.get_cf_exact(cf::CF_KV, b"row/2")?.is_none());
    assert!(db.get_cf_exact(cf::CF_KV, b"")?.is_none());
    assert_eq!(
        db.get_cf_exact("no_such_cf", b"row/1")
            .err()
            .map(|error| error.code()),
        Some(synapse_core::error_codes::STORAGE_READ_FAILED)
    );
    Ok(())
}
