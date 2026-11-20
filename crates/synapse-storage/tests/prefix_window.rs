use synapse_storage::{Db, cf};
use tempfile::TempDir;

#[test]
fn must_pass_bounded_prefix_window_stops_at_prefix_boundary_and_resumes()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = TempDir::new()?;
    let db = Db::open(&temp.path().join("db"), 1)?;
    db.put_batch(
        cf::CF_KV,
        [
            (b"other/00".to_vec(), b"outside-before".to_vec()),
            (b"orders/01".to_vec(), b"first".to_vec()),
            (b"orders/02".to_vec(), b"second".to_vec()),
            (b"orders/03".to_vec(), b"third".to_vec()),
            (b"orders0/01".to_vec(), b"outside-after".to_vec()),
        ],
    )?;
    db.flush()?;

    let (first, has_next) = db.scan_cf_prefix_window(cf::CF_KV, b"orders/", b"orders/", 2, 1024)?;
    assert_eq!(
        first.iter().map(|(key, _)| key.clone()).collect::<Vec<_>>(),
        [b"orders/01".to_vec(), b"orders/02".to_vec()]
    );
    assert!(has_next);

    let mut resume = first[1].0.clone();
    resume.push(0);
    let (second, has_next) = db.scan_cf_prefix_window(cf::CF_KV, b"orders/", &resume, 2, 1024)?;
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].0, b"orders/03");
    assert!(!has_next);

    let (empty, has_next) =
        db.scan_cf_prefix_window(cf::CF_KV, b"missing/", b"missing/", 2, 1024)?;
    assert!(empty.is_empty());
    assert!(!has_next);
    Ok(())
}

#[test]
fn must_pass_prefix_window_byte_budget_returns_one_row_per_window()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = TempDir::new()?;
    let db = Db::open(&temp.path().join("db"), 1)?;
    db.put_batch(
        cf::CF_KV,
        [
            (b"rows/01".to_vec(), vec![b'a'; 8]),
            (b"rows/02".to_vec(), vec![b'b'; 8]),
            (b"rows/03".to_vec(), vec![b'c'; 8]),
        ],
    )?;
    db.flush()?;
    let (first, more) = db.scan_cf_prefix_window(cf::CF_KV, b"rows/", b"rows/", 10, 12)?;
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].0, b"rows/01");
    assert!(more);
    let mut resume = first[0].0.clone();
    resume.push(0);
    let (second, more) = db.scan_cf_prefix_window(cf::CF_KV, b"rows/", &resume, 10, 12)?;
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].0, b"rows/02");
    assert!(more);
    Ok(())
}
