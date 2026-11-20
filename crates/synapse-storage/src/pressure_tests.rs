use super::*;
use std::{error::Error, sync::Arc, time::Duration};

const TEST_SCHEMA_VERSION: u32 = 7;

#[test]
fn must_pass_disk_pressure_transitions_emit_codes_once() -> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    let db = Db::open(&temp.path().join("db"), TEST_SCHEMA_VERSION)?;
    let config = test_config();
    let cases = [
        (
            350,
            DiskPressureLevel::Level1,
            Some(error_codes::STORAGE_DISK_PRESSURE_LEVEL_1),
            false,
        ),
        (340, DiskPressureLevel::Level1, None, false),
        (
            250,
            DiskPressureLevel::Level2,
            Some(error_codes::STORAGE_DISK_PRESSURE_LEVEL_2),
            true,
        ),
        (
            150,
            DiskPressureLevel::Level3,
            Some(error_codes::STORAGE_DISK_PRESSURE_LEVEL_3),
            true,
        ),
        (
            50,
            DiskPressureLevel::Level4,
            Some(error_codes::STORAGE_DISK_PRESSURE_LEVEL_4),
            true,
        ),
        (
            150,
            DiskPressureLevel::Level3,
            Some(error_codes::STORAGE_DISK_PRESSURE_LEVEL_3),
            true,
        ),
        (500, DiskPressureLevel::Normal, None, false),
    ];

    for (free_bytes, expected_level, expected_code, expect_compaction) in cases {
        let before_level = db.pressure_level();
        let before_codes = db.pressure.transition_codes()?;
        let report =
            pressure::run_once_with_free_bytes(&*db.raw()?, &db.pressure, &config, free_bytes)?;
        let after_level = db.pressure_level();
        let after_codes = db.pressure.transition_codes()?;
        println!(
            "regression_state=pressure_state free_bytes={} before_level={before_level:?} before_codes={before_codes:?} after_level={after_level:?} emitted_code={:?} compacted_cfs={} gc_advised={} observed=level:{after_level:?},codes:{after_codes:?}",
            report.free_bytes,
            report.emitted_code,
            report.compacted_cfs.len(),
            report.gc_advised
        );

        assert_eq!(report.current_level, expected_level);
        assert_eq!(after_level, expected_level);
        assert_eq!(report.emitted_code, expected_code);
        if expect_compaction {
            assert_eq!(report.compacted_cfs.len(), cf::ALL_COLUMN_FAMILIES.len());
        } else {
            assert!(report.compacted_cfs.is_empty());
        }
    }

    assert_eq!(
        db.pressure.transition_codes()?,
        vec![
            error_codes::STORAGE_DISK_PRESSURE_LEVEL_1,
            error_codes::STORAGE_DISK_PRESSURE_LEVEL_2,
            error_codes::STORAGE_DISK_PRESSURE_LEVEL_3,
            error_codes::STORAGE_DISK_PRESSURE_LEVEL_4,
            error_codes::STORAGE_DISK_PRESSURE_LEVEL_3,
        ]
    );
    Ok(())
}

/// Thresholds (400/300/200/100) are exclusive lower bounds: free bytes equal
/// to a threshold stay at the milder level, one byte below moves down. Pins
/// `<` against a `<=` mutant at every boundary.
#[test]
fn must_pass_pressure_level_thresholds_are_exclusive() -> Result<(), Box<dyn Error>> {
    let config = test_config();
    for (free_bytes, expected_level) in [
        (400, DiskPressureLevel::Normal),
        (399, DiskPressureLevel::Level1),
        (300, DiskPressureLevel::Level1),
        (299, DiskPressureLevel::Level2),
        (200, DiskPressureLevel::Level2),
        (199, DiskPressureLevel::Level3),
        (100, DiskPressureLevel::Level3),
        (99, DiskPressureLevel::Level4),
    ] {
        let temp = tempfile::tempdir()?;
        let db = Db::open(&temp.path().join("db"), TEST_SCHEMA_VERSION)?;
        let report =
            pressure::run_once_with_free_bytes(&*db.raw()?, &db.pressure, &config, free_bytes)?;
        assert_eq!(
            report.current_level, expected_level,
            "free_bytes={free_bytes}"
        );
        assert_eq!(
            db.pressure_level(),
            expected_level,
            "free_bytes={free_bytes}"
        );
    }
    Ok(())
}

#[test]
fn must_fail_disk_pressure_write_gating() -> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    let db = Db::open(&temp.path().join("db"), TEST_SCHEMA_VERSION)?;
    let config = test_config();

    db.put_batch(cf::CF_TIMELINE, row("timeline-normal"))?;
    db.flush()?;
    let normal_timeline = db.scan_cf(cf::CF_TIMELINE)?;
    println!(
        "regression_state=cf_scan level=Normal timeline={} observed=timeline:{:?}",
        normal_timeline.len(),
        printable_keys(&normal_timeline)
    );
    assert_eq!(normal_timeline.len(), 1);

    pressure::run_once_with_free_bytes(&*db.raw()?, &db.pressure, &config, 150)?;
    let before_observations = db.scan_cf(cf::CF_OBSERVATIONS)?;
    assert_shed(
        &db,
        cf::CF_OBSERVATIONS,
        "obs-l3",
        DiskPressureLevel::Level3,
    );
    db.put_batch(cf::CF_EVENTS, row("event-l3"))?;
    assert_shed(
        &db,
        cf::CF_TIMELINE,
        "timeline-l3",
        DiskPressureLevel::Level3,
    );
    db.flush()?;
    let after_l3_observations = db.scan_cf(cf::CF_OBSERVATIONS)?;
    let after_l3_events = db.scan_cf(cf::CF_EVENTS)?;
    let after_l3_timeline = db.scan_cf(cf::CF_TIMELINE)?;
    println!(
        "regression_state=cf_scan level=Level3 before_observations={} after_observations={} after_events={} after_timeline={} observed=observations:{:?},events:{:?},timeline:{:?}",
        before_observations.len(),
        after_l3_observations.len(),
        after_l3_events.len(),
        after_l3_timeline.len(),
        printable_keys(&after_l3_observations),
        printable_keys(&after_l3_events),
        printable_keys(&after_l3_timeline)
    );
    assert!(after_l3_observations.is_empty());
    assert_eq!(after_l3_events.len(), 1);
    assert_eq!(
        after_l3_timeline.len(),
        1,
        "CF_TIMELINE writes must shed at Level3 (only the Normal-level row survives)"
    );

    pressure::run_once_with_free_bytes(&*db.raw()?, &db.pressure, &config, 50)?;
    assert_shed(
        &db,
        cf::CF_OBSERVATIONS,
        "obs-l4",
        DiskPressureLevel::Level4,
    );
    assert_shed(&db, cf::CF_EVENTS, "event-l4", DiskPressureLevel::Level4);
    assert_shed(&db, cf::CF_KV, "approval-l4", DiskPressureLevel::Level4);
    assert_shed(
        &db,
        cf::CF_ACTION_LOG,
        "action-l4",
        DiskPressureLevel::Level4,
    );
    assert_shed(
        &db,
        cf::CF_AGENT_EVENTS,
        "agent-l4",
        DiskPressureLevel::Level4,
    );
    db.put_batch(cf::CF_REFLEX_AUDIT, row("audit-l4"))?;
    db.put_batch(cf::CF_SESSIONS, row("session-l4"))?;
    assert_shed(
        &db,
        cf::CF_TIMELINE,
        "timeline-l4",
        DiskPressureLevel::Level4,
    );
    db.flush()?;
    let after_l4_observations = db.scan_cf(cf::CF_OBSERVATIONS)?;
    let after_l4_events = db.scan_cf(cf::CF_EVENTS)?;
    let after_l4_audit = db.scan_cf(cf::CF_REFLEX_AUDIT)?;
    let after_l4_sessions = db.scan_cf(cf::CF_SESSIONS)?;
    let after_l4_timeline = db.scan_cf(cf::CF_TIMELINE)?;
    assert!(db.scan_cf(cf::CF_KV)?.is_empty());
    assert!(db.scan_cf(cf::CF_ACTION_LOG)?.is_empty());
    assert!(db.scan_cf(cf::CF_AGENT_EVENTS)?.is_empty());
    assert_eq!(
        after_l4_timeline.len(),
        1,
        "CF_TIMELINE writes must shed at Level4 (only the Normal-level row survives)"
    );
    println!(
        "regression_state=cf_scan level=Level4 observations={} events={} audit={} sessions={} observed=observations:{:?},events:{:?},audit:{:?},sessions:{:?}",
        after_l4_observations.len(),
        after_l4_events.len(),
        after_l4_audit.len(),
        after_l4_sessions.len(),
        printable_keys(&after_l4_observations),
        printable_keys(&after_l4_events),
        printable_keys(&after_l4_audit),
        printable_keys(&after_l4_sessions)
    );

    assert!(after_l4_observations.is_empty());
    assert_eq!(after_l4_events.len(), 1);
    assert_eq!(after_l4_audit.len(), 1);
    assert_eq!(after_l4_sessions.len(), 1);
    Ok(())
}

/// Accept side of the gate: Level2 sheds nothing (observations and timeline
/// still persist); Level3 keeps the control-plane journals CF_AGENT_EVENTS
/// and CF_ACTION_LOG writable (#897) while CF_AGENT_TRANSCRIPTS sheds (#900).
#[test]
fn must_pass_level2_and_level3_keep_control_plane_cfs_writable() -> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    let db = Db::open(&temp.path().join("db"), TEST_SCHEMA_VERSION)?;
    let config = test_config();

    pressure::run_once_with_free_bytes(&*db.raw()?, &db.pressure, &config, 250)?;
    assert_eq!(db.pressure_level(), DiskPressureLevel::Level2);
    db.put_batch(cf::CF_OBSERVATIONS, row("obs-l2"))?;
    db.put_batch(cf::CF_TIMELINE, row("timeline-l2"))?;

    pressure::run_once_with_free_bytes(&*db.raw()?, &db.pressure, &config, 150)?;
    assert_eq!(db.pressure_level(), DiskPressureLevel::Level3);
    db.put_batch(cf::CF_AGENT_EVENTS, row("agent-l3"))?;
    db.put_batch(cf::CF_ACTION_LOG, row("action-l3"))?;
    assert_shed(
        &db,
        cf::CF_AGENT_TRANSCRIPTS,
        "transcript-l3",
        DiskPressureLevel::Level3,
    );
    db.flush()?;

    for (cf_name, expected_key) in [
        (cf::CF_OBSERVATIONS, "obs-l2"),
        (cf::CF_TIMELINE, "timeline-l2"),
        (cf::CF_AGENT_EVENTS, "agent-l3"),
        (cf::CF_ACTION_LOG, "action-l3"),
    ] {
        assert_eq!(
            printable_keys(&db.scan_cf(cf_name)?),
            vec![expected_key.to_owned()],
            "{cf_name}"
        );
    }
    assert!(db.scan_cf(cf::CF_AGENT_TRANSCRIPTS)?.is_empty());
    Ok(())
}

fn assert_shed(db: &Db, cf_name: &str, label: &'static str, level: DiskPressureLevel) {
    let result = db.put_batch(cf_name, row(label));
    assert_eq!(
        result.as_ref().err().map(StorageError::code),
        Some(error_codes::STORAGE_WRITE_SHED)
    );
    assert!(matches!(
        result,
        Err(StorageError::WriteShed {
            cf_name: rejected_cf,
            pressure_level,
            dropped_rows: 1,
        }) if rejected_cf == cf_name && pressure_level == level
    ));
}

#[tokio::test]
async fn must_pass_disk_pressure_periodic_task_runs_tick() -> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    let db = Db::open(&temp.path().join("db"), TEST_SCHEMA_VERSION)?;
    let task = pressure::spawn_with_free_bytes(
        Arc::clone(&db.inner),
        Arc::clone(&db.pressure),
        db.path.clone(),
        test_config(),
        vec![350],
    )?;
    // Poll the spawned tick's effect with a deadline instead of a fixed sleep.
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while db.pressure_level() != DiskPressureLevel::Level1 && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let after_level = db.pressure_level();
    let after_codes = db.pressure.transition_codes()?;
    println!(
        "regression_state=pressure_state case=periodic_task after_level={after_level:?} after_codes={after_codes:?} observed=level:{after_level:?}"
    );
    drop(task);
    assert_eq!(after_level, DiskPressureLevel::Level1);
    assert_eq!(
        after_codes,
        vec![error_codes::STORAGE_DISK_PRESSURE_LEVEL_1]
    );
    Ok(())
}

fn test_config() -> pressure::PressureConfig {
    pressure::PressureConfig::with_thresholds(Duration::from_millis(10), 400, 300, 200, 100)
}

fn row(label: &'static str) -> Vec<(Vec<u8>, Vec<u8>)> {
    vec![(
        label.as_bytes().to_vec(),
        format!(r#"{{"label":"{label}"}}"#).into_bytes(),
    )]
}

fn printable_keys(rows: &[(Vec<u8>, Vec<u8>)]) -> Vec<String> {
    rows.iter()
        .map(|(key, _value)| String::from_utf8_lossy(key).into_owned())
        .collect()
}
