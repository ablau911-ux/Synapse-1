//! Regression test for #60: a locked/undeletable file in the shared log dir
//! must not abort telemetry init. Owns its own binary because `init_tracing`
//! installs a process-wide subscriber.

#[cfg(windows)]
#[test]
fn must_fail_locked_expired_log_file_does_not_abort_init() -> Result<(), Box<dyn std::error::Error>>
{
    use std::os::windows::fs::OpenOptionsExt;

    use synapse_telemetry::{TelemetryConfig, init_tracing};
    use tempfile::TempDir;

    let dir = TempDir::new()?;
    let locked = dir.path().join("synapse.log.1970-01-01");
    std::fs::write(&locked, b"ancient locked line\n")?;
    // share_mode(0): no delete sharing, so remove_file fails while held open.
    let _held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&locked)?;
    let removable = dir.path().join("synapse.log.1970-01-02");
    std::fs::write(&removable, b"ancient removable line\n")?;

    let cfg = TelemetryConfig {
        log_dir: Some(dir.path().to_path_buf()),
        keep_days: 0,
        max_dir_bytes: 1024 * 1024,
        gc_interval: None,
        ..TelemetryConfig::default()
    };
    let _guard = init_tracing(cfg)?;

    assert!(locked.exists(), "locked file cannot be removed");
    assert!(
        !removable.exists(),
        "other expired files are still collected"
    );
    Ok(())
}
