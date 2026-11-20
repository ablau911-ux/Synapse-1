use synapse_telemetry::{TelemetryConfig, flush_for_process_exit, init_tracing};

#[test]
fn must_pass_exit_flush_persists_queued_log_before_guard_drop()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let guard = init_tracing(TelemetryConfig {
        log_dir: Some(dir.path().to_owned()),
        gc_interval: None,
        ..TelemetryConfig::default()
    })?;
    for sequence in 0..100 {
        tracing::info!(code = "EXIT_FLUSH_REGRESSION", sequence, "queued record");
    }
    flush_for_process_exit();
    let mut records = 0;
    for entry in std::fs::read_dir(dir.path())? {
        let text = std::fs::read_to_string(entry?.path())?;
        records += text
            .lines()
            .filter(|line| line.contains("EXIT_FLUSH_REGRESSION"))
            .count();
    }
    assert_eq!(records, 100);
    // Taking the worker twice and later dropping the owning guard is safe.
    flush_for_process_exit();
    drop(guard);
    Ok(())
}
