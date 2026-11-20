//! Regression tests for #180: a corrupt persisted lease row is reported once,
//! not on every 250 ms orphan-sweep tick; the row stays in place for repair.

use tempfile::TempDir;

use super::session_lease_orphan_sweep_tests::{lease_row, put_row};
use super::skipped_rows::{MAX_REMEMBERED_KEYS, SkippedRowWarnings, SweepSkips};
use super::*;
use crate::test_support;

fn sweep(warnings: &SkippedRowWarnings, keys: &[&[u8]]) -> usize {
    let mut skips = SweepSkips::default();
    let warned = keys
        .iter()
        .filter(|key| warnings.should_warn(&mut skips, key))
        .count();
    warnings.finish_sweep(skips);
    warned
}

#[test]
fn must_fail_a_corrupt_key_warns_once_across_many_sweeps() {
    let warnings = SkippedRowWarnings::default();
    assert_eq!(sweep(&warnings, &[b"k1"]), 1);
    for _ in 0..100 {
        assert_eq!(sweep(&warnings, &[b"k1"]), 0);
    }
    assert_eq!(warnings.remembered(), 1);
}

#[test]
fn must_fail_each_distinct_corrupt_key_warns_once() {
    let warnings = SkippedRowWarnings::default();
    assert_eq!(sweep(&warnings, &[b"k1", b"k2"]), 2);
    assert_eq!(sweep(&warnings, &[b"k1", b"k2", b"k3"]), 1);
    assert_eq!(sweep(&warnings, &[b"k1", b"k2", b"k3"]), 0);
}

#[test]
fn must_fail_a_repaired_row_is_forgotten_and_warns_again_if_corrupted_again() {
    let warnings = SkippedRowWarnings::default();
    assert_eq!(sweep(&warnings, &[b"k1", b"k2"]), 2);
    // k1 repaired or deleted: not corrupt in this sweep.
    assert_eq!(sweep(&warnings, &[b"k2"]), 0);
    assert!(!warnings.is_remembered(b"k1"));
    assert_eq!(sweep(&warnings, &[b"k1", b"k2"]), 1);
}

#[test]
fn must_pass_an_empty_sweep_clears_the_memory() {
    let warnings = SkippedRowWarnings::default();
    assert_eq!(sweep(&warnings, &[b"k1"]), 1);
    assert_eq!(sweep(&warnings, &[]), 0);
    assert_eq!(warnings.remembered(), 0);
}

#[test]
fn must_fail_memory_is_bounded_and_overflow_stays_quiet() {
    let warnings = SkippedRowWarnings::default();
    let keys: Vec<Vec<u8>> = (0..MAX_REMEMBERED_KEYS + 50)
        .map(|index| format!("key-{index}").into_bytes())
        .collect();
    let refs: Vec<&[u8]> = keys.iter().map(Vec::as_slice).collect();

    assert_eq!(sweep(&warnings, &refs), MAX_REMEMBERED_KEYS);
    assert_eq!(warnings.remembered(), MAX_REMEMBERED_KEYS);
    assert_eq!(sweep(&warnings, &refs), 0);
}

#[test]
fn must_fail_real_sweep_remembers_the_corrupt_row_without_deleting_it() -> anyhow::Result<()> {
    let _serial = test_support::lease_serial("orphan_sweep_test");
    let temp = TempDir::new()?;
    let service = super::session_lease_orphan_sweep_tests::service(temp.path())?;
    let state = service.m3_state_handle();
    let db = session_continuity_db_from_state(&state).map_err(anyhow::Error::msg)?;
    let key = session_lease_key("garbled-180");
    db.put_batch_pressure_bypass(cf::CF_SESSIONS, [(key.clone(), b"{not-json".to_vec())])?;
    put_row(&db, &lease_row("ghost-180", 0, 0))?;

    for _ in 0..5 {
        reconcile_orphaned_session_lease_rows(&state, "orphan_sweep")
            .map_err(anyhow::Error::msg)?;
    }

    assert!(SkippedRowWarnings::global().is_remembered(&key));
    assert!(session_lease_orphan_sweep_tests::row_exists(
        &db,
        "garbled-180"
    )?);
    assert!(!session_lease_orphan_sweep_tests::row_exists(
        &db,
        "ghost-180"
    )?);

    // Repair: delete the corrupt row; the next sweep forgets it.
    db.delete_batch(cf::CF_SESSIONS, [key.clone()])?;
    reconcile_orphaned_session_lease_rows(&state, "orphan_sweep").map_err(anyhow::Error::msg)?;
    assert!(!SkippedRowWarnings::global().is_remembered(&key));
    Ok(())
}
