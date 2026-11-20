//! Issue #11: a daemon upgrade must not turn into a terminal `Transport closed`
//! for an already-running stdio bridge.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{
    RECONNECT_SPAWN_GRACE, RECONNECT_WINDOW, ReconnectStep, daemon_spawn_executable,
    process_start_unix, reconnect_step, setup_maintenance_held, spawn_unless_setup_maintenance,
};

const HELD_LOCK: &str = "\u{feff}{\"schema\":\"synapse_setup_maintenance_lock/v1\",\"state\":\"held\",\"pid\":4242,\"pid_start_unix\":1000}";

/// Live process 4242 that started at unix second 1000.
fn live_4242(pid: u32) -> Option<u64> {
    (pid == 4242).then_some(1000)
}

#[test]
fn must_fail_live_setup_maintenance_lock_defers_self_spawn() {
    assert!(setup_maintenance_held(Some(HELD_LOCK), live_4242));
}

#[test]
fn must_fail_stale_released_or_unreadable_lock_is_ignored() {
    // dead owner pid
    assert!(!setup_maintenance_held(Some(HELD_LOCK), |_| None));
    let released = r#"{"schema":"synapse_setup_maintenance_lock/v1","state":"released","pid":4242,"pid_start_unix":1000}"#;
    assert!(!setup_maintenance_held(Some(released), live_4242));
    assert!(!setup_maintenance_held(None, live_4242));
    assert!(!setup_maintenance_held(Some("not json"), live_4242));
    let other = r#"{"schema":"other/v1","state":"held","pid":4242,"pid_start_unix":1000}"#;
    assert!(!setup_maintenance_held(Some(other), live_4242));
    let no_pid = r#"{"schema":"synapse_setup_maintenance_lock/v1","state":"held"}"#;
    assert!(!setup_maintenance_held(Some(no_pid), live_4242));
}

#[test]
fn must_fail_reused_pid_with_different_start_time_is_a_stale_owner() {
    // PID 4242 is alive but started long after the lock owner did: reused PID.
    assert!(!setup_maintenance_held(Some(HELD_LOCK), |pid| (pid
        == 4242)
        .then_some(5000)));
    // Tiny clock-resolution skew between PowerShell and sysinfo is tolerated.
    assert!(setup_maintenance_held(Some(HELD_LOCK), |pid| (pid == 4242).then_some(1001)));
    assert!(setup_maintenance_held(Some(HELD_LOCK), |pid| (pid == 4242).then_some(999)));
}

#[test]
fn must_pass_lock_without_recorded_start_time_falls_back_to_pid_liveness() {
    let legacy = r#"{"schema":"synapse_setup_maintenance_lock/v1","state":"held","pid":4242}"#;
    assert!(setup_maintenance_held(Some(legacy), live_4242));
    assert!(!setup_maintenance_held(Some(legacy), |_| None));
}

#[test]
fn must_fail_guarded_spawn_is_skipped_while_setup_maintenance_is_held() {
    let mut spawned = false;
    let did = spawn_unless_setup_maintenance(
        || Ok(()),
        || Some(HELD_LOCK.to_owned()),
        live_4242,
        || {
            spawned = true;
            Ok(())
        },
    )
    .expect("guard ok");
    assert!(!did);
    assert!(!spawned);
}

#[test]
fn must_pass_guarded_spawn_runs_when_lock_is_absent_or_stale() {
    for lock in [None, Some(HELD_LOCK.to_owned())] {
        let mut spawned = false;
        let did = spawn_unless_setup_maintenance(
            || Ok(()),
            || lock.clone(),
            |_| None,
            || {
                spawned = true;
                Ok(())
            },
        )
        .expect("guard ok");
        assert!(did && spawned);
    }
}

struct RecordingGuard<'a>(&'a RefCell<Vec<&'static str>>);

impl Drop for RecordingGuard<'_> {
    fn drop(&mut self) {
        self.0.borrow_mut().push("guard_released");
    }
}

#[test]
fn must_pass_spawn_guard_is_held_across_lock_check_and_spawn() {
    // TOCTOU fix: setup takes the same guard before it takes its lock, so the
    // bridge must acquire it before reading the lock and keep it until the
    // spawn has completed.
    let events = RefCell::new(Vec::new());
    let did = spawn_unless_setup_maintenance(
        || {
            events.borrow_mut().push("guard_acquired");
            Ok(RecordingGuard(&events))
        },
        || {
            events.borrow_mut().push("lock_read");
            None
        },
        |_| None,
        || {
            events.borrow_mut().push("spawn");
            Ok(())
        },
    )
    .expect("guard ok");
    assert!(did);
    assert_eq!(
        *events.borrow(),
        ["guard_acquired", "lock_read", "spawn", "guard_released"]
    );
}

#[test]
fn must_pass_spawn_guard_is_released_when_setup_holds_the_lock() {
    let events = RefCell::new(Vec::new());
    let did = spawn_unless_setup_maintenance(
        || Ok(RecordingGuard(&events)),
        || Some(HELD_LOCK.to_owned()),
        live_4242,
        || {
            events.borrow_mut().push("spawn");
            Ok(())
        },
    )
    .expect("guard ok");
    assert!(!did);
    assert_eq!(*events.borrow(), ["guard_released"]);
}

#[test]
fn must_fail_guard_acquire_failure_blocks_the_spawn() {
    let mut spawned = false;
    let result = spawn_unless_setup_maintenance(
        || -> anyhow::Result<()> { anyhow::bail!("guard timeout") },
        || None,
        |_| None,
        || {
            spawned = true;
            Ok(())
        },
    );
    let Err(error) = result else {
        panic!("a failed guard acquire must block the spawn with an error");
    };
    assert!(
        format!("{error:#}").contains("guard timeout"),
        "the guard error must propagate, got: {error:#}"
    );
    assert!(!spawned);
}

#[test]
fn must_pass_process_start_time_reports_live_process_and_none_for_dead_pid() {
    let start = process_start_unix(std::process::id()).expect("own process start");
    assert!(start > 1_600_000_000);
    assert!(process_start_unix(u32::MAX - 1).is_none());
}

#[test]
fn must_pass_bridge_running_from_retired_image_spawns_canonical_installed_binary() {
    let retired =
        Path::new(r"C:\Users\x\.cargo\bin\synapse-mcp.exe.retired-20261014120000123-4242");
    assert_eq!(
        daemon_spawn_executable(retired),
        PathBuf::from(r"C:\Users\x\.cargo\bin\synapse-mcp.exe")
    );
}

#[test]
fn must_pass_bridge_running_from_canonical_or_dev_image_spawns_itself() {
    let canonical = Path::new(r"C:\Users\x\.cargo\bin\synapse-mcp.exe");
    assert_eq!(daemon_spawn_executable(canonical), canonical.to_path_buf());
    let dev = Path::new(r"C:\scratch\copy\synapse-mcp-copy.exe");
    assert_eq!(daemon_spawn_executable(dev), dev.to_path_buf());
}

#[test]
fn must_pass_reconnect_waits_for_external_daemon_before_self_spawning() {
    assert_eq!(reconnect_step(Duration::ZERO), ReconnectStep::WaitForDaemon);
    assert_eq!(
        reconnect_step(RECONNECT_SPAWN_GRACE.saturating_sub(Duration::from_millis(1))),
        ReconnectStep::WaitForDaemon
    );
    assert_eq!(
        reconnect_step(RECONNECT_SPAWN_GRACE),
        ReconnectStep::SpawnIfAbsent
    );
}

#[test]
fn must_fail_reconnect_survives_a_long_upgrade_window_then_fails_closed() {
    // A single failed attempt (the old behaviour) must not end the bridge.
    assert!(RECONNECT_WINDOW > RECONNECT_SPAWN_GRACE);
    assert_eq!(
        reconnect_step(RECONNECT_WINDOW.saturating_sub(Duration::from_millis(1))),
        ReconnectStep::SpawnIfAbsent
    );
    assert_eq!(reconnect_step(RECONNECT_WINDOW), ReconnectStep::GiveUp);
}
