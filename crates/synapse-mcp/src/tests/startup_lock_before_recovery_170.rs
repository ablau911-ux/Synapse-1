//! #170: crash-recovery (input release_all + ledger delete) must run only after
//! the single-instance lock is owned. A second launch must not touch the live
//! daemon's held inputs or its recovery ledger.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::recover_stale_inputs_as_lock_owner;
use crate::single_instance::SingleInstanceGuard;

/// Ledger file the recovery pass would read for `db` (an undecodable line makes
/// a recovery pass fail loudly, so "it ran" is observable without SendInput).
fn poison_ledger(db: &std::path::Path) -> std::path::PathBuf {
    let path = synapse_action::configure_crash_recovery_file(Some(db)).unwrap();
    std::fs::write(&path, "not-a-recovery-event\n").unwrap();
    path
}

#[test]
fn must_pass_lock_owner_runs_recovery() {
    let _serial = crash_recovery_serial();
    let dir = tempfile::tempdir().unwrap();
    let guard = SingleInstanceGuard::acquire(dir.path()).unwrap();
    let ledger = poison_ledger(dir.path());

    let result = recover_stale_inputs_as_lock_owner(&guard, Some(dir.path()));

    let Err(error) = result else {
        panic!("owner must read the poisoned ledger and fail loudly");
    };
    let chain = format!("{error:#}");
    assert!(
        chain.contains("recover stale action inputs from previous daemon"),
        "the failure must come from the recovery pass, got: {chain}"
    );
    assert!(ledger.exists());
}

/// `configure_crash_recovery_file` sets a process-global ledger path, so tests
/// that configure and then recover must not interleave.
fn crash_recovery_serial() -> std::sync::MutexGuard<'static, ()> {
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[test]
fn must_fail_second_launch_leaves_live_daemon_ledger_untouched() {
    let _serial = crash_recovery_serial();
    let dir = tempfile::tempdir().unwrap();
    let first = SingleInstanceGuard::acquire(dir.path()).unwrap();
    let ledger = poison_ledger(dir.path());
    let before = std::fs::read(&ledger).unwrap();

    // Second launch: lock refused, so no guard exists to authorise recovery.
    let second = SingleInstanceGuard::acquire(dir.path());
    assert!(second.is_err());
    if let Ok(guard) = &second {
        let _ = recover_stale_inputs_as_lock_owner(guard, Some(dir.path()));
    }

    assert_eq!(std::fs::read(&ledger).unwrap(), before);
    drop(first);
}
