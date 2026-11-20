use std::time::Duration;

use synapse_telemetry::{TelemetryConfig, init_tracing};
use tempfile::TempDir;

/// Periodic GC `state`: synthesise a too-old file in the log dir, init telemetry with
/// `keep_days=0` and a very short `gc_interval`, wait one tick + slack, assert the
/// file is gone. Proves the background worker actually re-runs `run_log_gc`
/// mid-uptime (the previous implementation only ran GC at startup, so a daemon
/// that lived past `keep_days` would silently retain files past the cap).
///
/// This test owns its own integration-test binary because `try_init` installs a
/// process-wide tracing subscriber and cannot be re-init'd in the same process.
#[test]
fn must_pass_periodic_gc_evicts_files_past_keep_days() -> Result<(), Box<dyn std::error::Error>> {
    let dir = TempDir::new()?;
    let stale = dir.path().join("synapse.log.1970-01-01");
    std::fs::write(&stale, b"ancient log line\n")?;
    assert!(stale.exists(), "fixture: stale file should exist pre-init");
    println!(
        "regression_state=periodic_gc edge=startup before=stale_exists:{} path={}",
        stale.exists(),
        stale.display()
    );

    let cfg = TelemetryConfig {
        log_dir: Some(dir.path().to_path_buf()),
        keep_days: 0,
        max_dir_bytes: 1024 * 1024,
        gc_interval: Some(Duration::from_millis(75)),
        ..TelemetryConfig::default()
    };
    let guard = init_tracing(cfg)?;

    // `run_log_gc` also runs at init and evicts the first stale file immediately,
    // so we synthesise a SECOND stale file AFTER init to prove the interval-driven
    // loop fires at least once during the daemon's uptime.
    let stale_after_init = dir.path().join("synapse.log.1970-01-02");
    std::fs::write(&stale_after_init, b"second ancient log line\n")?;
    let pre_periodic = stale_after_init.exists();
    println!(
        "regression_state=periodic_gc edge=mid_run before=stale_after_init_exists:{} path={}",
        pre_periodic,
        stale_after_init.display()
    );

    // Poll with a 2 s deadline (like periodic_gc_size_cap.rs) instead of a
    // fixed 400 ms sleep: load-proof, and a worker that never ticks still fails.
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while stale_after_init.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(25));
    }
    let post_periodic = stale_after_init.exists();
    println!(
        "regression_state=periodic_gc edge=mid_run after=stale_after_init_exists:{post_periodic}"
    );

    drop(guard);
    assert!(
        pre_periodic,
        "fixture: stale_after_init should have existed before the periodic tick"
    );
    assert!(
        !post_periodic,
        "periodic GC did not evict stale_after_init within the tick window"
    );
    Ok(())
}

/// A log dir that is not a directory is refused before GC or the subscriber
/// run: nothing is deleted and no process-wide subscriber is installed, so
/// this test cannot race the periodic test above in the same binary.
#[test]
fn must_fail_non_directory_log_dir_is_refused_and_gc_deletes_nothing()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = TempDir::new()?;
    let not_a_dir = dir.path().join("synapse.log.1970-01-01");
    std::fs::write(&not_a_dir, b"ancient log line\n")?;

    let cfg = TelemetryConfig {
        log_dir: Some(not_a_dir.clone()),
        keep_days: 0,
        max_dir_bytes: 1024 * 1024,
        gc_interval: Some(Duration::from_millis(75)),
        ..TelemetryConfig::default()
    };
    let result = init_tracing(cfg);
    let code = result
        .as_ref()
        .err()
        .map(synapse_telemetry::TelemetryError::code);
    drop(result);
    assert_eq!(code, Some("TELEMETRY_LOG_DIR_NOT_WRITABLE"));

    std::thread::sleep(Duration::from_millis(250));
    assert_eq!(
        std::fs::read(&not_a_dir)?,
        b"ancient log line\n".to_vec(),
        "a refused init must not run GC or a periodic worker against the path"
    );
    Ok(())
}
