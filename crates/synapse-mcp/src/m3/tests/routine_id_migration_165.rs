//! Regression tests for #165: armed automations and operator state must
//! survive a re-mine bound to the right routine, and unmatched rows must be
//! disarmed with a reason instead of being silently re-bound.

use super::*;

#[cfg(test)]
#[path = "routine_persisted_reconciliation_165.rs"]
mod routine_persisted_reconciliation_165;

use synapse_core::SCHEMA_VERSION;
use synapse_core::routine_identity::routine_id;
use synapse_core::types::{
    ROUTINE_RECORD_VERSION, ROUTINE_STATE_RECORD_VERSION, RoutineDowClass, RoutineGranularity,
    RoutineLifecycle, RoutineStateRecord, RoutineStep,
};

use crate::m3::armed_routines::{ArmedRoutineRecord, load_armed_routine_record};
use crate::m3::profile_authoring::load_routine_automation_record;
use crate::m3::routines::load_state_row;

const LEGACY_EXCEL: &str = "rt1-aaaaaaaaaaaaaaaa";
const LEGACY_VANISHED: &str = "rt1-bbbbbbbbbbbbbbbb";

fn steps(apps: &[&str]) -> Vec<RoutineStep> {
    apps.iter()
        .map(|app| RoutineStep {
            app: (*app).to_owned(),
            document: None,
        })
        .collect()
}

fn routine(id: &str, apps: &[&str], mean_minute: u32) -> RoutineRecord {
    RoutineRecord {
        record_version: ROUTINE_RECORD_VERSION,
        ts_ns: 100,
        routine_id: id.to_owned(),
        granularity: RoutineGranularity::App,
        steps: steps(apps),
        dow_class: RoutineDowClass::Weekdays,
        mean_minute_of_day: mean_minute,
        tolerance_minutes: 10,
        schedule_label: "weekdays".to_owned(),
        support_days: 5,
        occurrence_count: 5,
        opportunity_days: 5,
        confidence: 0.8,
        window_start_ns: 0,
        window_end_ns: 10,
        active_days_in_window: 5,
        first_seen_day_start_ns: 1,
        last_seen_day_start_ns: 9,
        evidence: Vec::new(),
    }
}

fn state(id: &str, lifecycle: RoutineLifecycle, label: &str) -> RoutineStateRecord {
    RoutineStateRecord {
        record_version: ROUTINE_STATE_RECORD_VERSION,
        routine_id: id.to_owned(),
        lifecycle,
        label: Some(label.to_owned()),
        created_ts_ns: 1,
        updated_ts_ns: 1,
        last_mined_ts_ns: Some(1),
        present_in_last_mine: true,
        transitions: Vec::new(),
        transitions_truncated: 0,
        confidence_history: Vec::new(),
        confidence_history_truncated: 0,
        feedback_events: Vec::new(),
        feedback_events_truncated: 0,
        accept_count: 0,
        decline_count: 0,
        ignore_count: 0,
        abandon_count: 0,
        consecutive_declines: 0,
        cooldown_level: 0,
        cooldown_until_ts_ns: None,
    }
}

fn armed(id: &str) -> ArmedRoutineRecord {
    ArmedRoutineRecord {
        record_version: 1,
        row_kind: "armed_routine".to_owned(),
        routine_id: id.to_owned(),
        enabled: true,
        schedule_enabled: true,
        intent_enabled: false,
        failure_threshold: 3,
        consecutive_failures: 0,
        created_at_ns: 1,
        updated_at_ns: 1,
        armed_at_ns: Some(1),
        armed_by: Some("operator".to_owned()),
        arm_note: None,
        disarmed_at_ns: None,
        disarmed_by: None,
        disarm_reason: None,
        plan_ref: Some(format!("plan/v1/{id}")),
        last_schedule_fire_key: None,
        last_intent_fire_key: None,
        last_run_id: None,
        last_run_status: None,
        source_of_truth: "test".to_owned(),
    }
}

fn automation(id: &str) -> RoutineAutomationRecord {
    RoutineAutomationRecord {
        schema_version: 1,
        row_kind: "routine_automation".to_owned(),
        routine_id: id.to_owned(),
        profile_id: "excel".to_owned(),
        candidate_id: format!("routine-auto.{id}.feedfacefeedface"),
        candidate_row_key: "candidate".to_owned(),
        plan_ref: format!("plan/v1/{id}"),
        state: "installed".to_owned(),
        generated_at_ns: 1,
        updated_at_ns: 1,
        installed_at_ns: Some(1),
        rejected_at_ns: None,
        plan_fully_deterministic: true,
        total_steps: 2,
        deterministic_steps: 2,
        agent_task_steps: 0,
    }
}

fn put(db: &Db, cf_name: &str, key: Vec<u8>, value: &impl serde::Serialize) {
    db.put_batch_pressure_bypass(cf_name, [(key, encode_json(value).expect("encode"))])
        .expect("put row");
}

/// Store as the OLD binary left it: legacy ids everywhere, one routine that
/// is re-mined (excel) and one that is not (vanished).
fn legacy_db() -> (tempfile::TempDir, Arc<Db>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(Db::open(dir.path(), SCHEMA_VERSION).expect("open db"));
    for old in [
        routine(LEGACY_EXCEL, &["outlook", "excel"], 525),
        routine(LEGACY_VANISHED, &["gimp", "krita"], 900),
    ] {
        let key = routine_codec::routine_key(&old.routine_id).expect("key");
        put(&db, cf::CF_ROUTINES, key, &old);
    }
    for (id, lifecycle, label) in [
        (LEGACY_EXCEL, RoutineLifecycle::Confirmed, "Morning report"),
        (LEGACY_VANISHED, RoutineLifecycle::Confirmed, "Art session"),
    ] {
        let key = routine_codec::routine_state_key(id).expect("key");
        put(&db, cf::CF_ROUTINE_STATE, key, &state(id, lifecycle, label));
        put(
            &db,
            cf::CF_KV,
            armed_routine_key(id).into_bytes(),
            &armed(id),
        );
    }
    put(
        &db,
        cf::CF_KV,
        routine_automation_key(LEGACY_EXCEL).into_bytes(),
        &automation(LEGACY_EXCEL),
    );
    (dir, db)
}

/// The re-mined excel routine, spelled cosmetically differently, with its
/// new stable id and a drifted mean minute.
fn remined_excel() -> RoutineRecord {
    let steps = steps(&["Outlook.EXE", "excel"]);
    let id = routine_id(RoutineGranularity::App, &steps, 0);
    let mut record = routine(&id, &[], 531);
    record.steps = steps;
    record
}

#[test]
fn must_pass_armed_automation_and_state_follow_the_remined_routine() {
    let (_dir, db) = legacy_db();
    let fresh = remined_excel();
    let mut scanned = 0_u64;

    let summary = migrate_legacy_routine_ids(&db, std::slice::from_ref(&fresh), &mut scanned)
        .expect("migrate")
        .expect("first pass runs");
    assert_eq!(summary.routines_remapped, 1);
    assert_eq!(summary.state_rows_moved, 1);
    assert_eq!(summary.armed_rows_moved, 1);
    assert_eq!(summary.automation_rows_moved, 1);

    // Operator state, armed row, and automation row now live under the new id.
    let moved_state = load_state_row(&db, &fresh.routine_id)
        .expect("read state")
        .expect("state moved");
    assert_eq!(moved_state.label.as_deref(), Some("Morning report"));
    assert_eq!(moved_state.lifecycle, RoutineLifecycle::Confirmed);
    let moved_armed = load_armed_routine_record(&db, &fresh.routine_id)
        .expect("read armed")
        .expect("armed moved");
    assert!(moved_armed.enabled);
    assert_eq!(moved_armed.routine_id, fresh.routine_id);
    assert_eq!(
        moved_armed.plan_ref.as_deref(),
        Some("plan/v1/rt1-aaaaaaaaaaaaaaaa")
    );
    let moved_automation = load_routine_automation_record(&db, &fresh.routine_id)
        .expect("read automation")
        .expect("automation moved");
    assert_eq!(moved_automation.state, "installed");

    // ...and nothing is left behind under the legacy id.
    assert!(load_state_row(&db, LEGACY_EXCEL).expect("read").is_none());
    assert!(
        load_armed_routine_record(&db, LEGACY_EXCEL)
            .expect("read")
            .is_none()
    );
    assert!(
        load_routine_automation_record(&db, LEGACY_EXCEL)
            .expect("read")
            .is_none()
    );
}

#[test]
fn must_fail_unmatched_armed_routine_is_kept_but_disarmed_with_a_reason() {
    let (_dir, db) = legacy_db();
    let fresh = remined_excel();
    let mut scanned = 0_u64;
    let summary = migrate_legacy_routine_ids(&db, std::slice::from_ref(&fresh), &mut scanned)
        .expect("migrate")
        .expect("first pass runs");
    assert_eq!(summary.armed_rows_disarmed, 1);

    let kept = load_armed_routine_record(&db, LEGACY_VANISHED)
        .expect("read armed")
        .expect("unmatched armed row is kept");
    assert!(!kept.enabled, "unmatched armed routine must be disarmed");
    assert_eq!(
        kept.disarmed_by.as_deref(),
        Some(ROUTINE_ID_MIGRATION_ACTOR)
    );
    assert!(
        kept.disarm_reason.as_deref().is_some_and(|reason| reason
            .starts_with("ROUTINE_ID_UNMATCHED: armed routine rt1-bbbbbbbbbbbbbbbb ")),
        "{:?}",
        kept.disarm_reason
    );
    // Its operator state is preserved untouched under the old id.
    let kept_state = load_state_row(&db, LEGACY_VANISHED)
        .expect("read state")
        .expect("unmatched state row is kept");
    assert_eq!(kept_state.label.as_deref(), Some("Art session"));
    // And it was NOT re-bound to the unrelated re-mined routine.
    let rebound = load_armed_routine_record(&db, &fresh.routine_id)
        .expect("read armed")
        .expect("matched armed row");
    assert_eq!(
        rebound.plan_ref.as_deref(),
        Some("plan/v1/rt1-aaaaaaaaaaaaaaaa")
    );
}

#[test]
fn must_pass_migration_runs_once_and_a_second_pass_changes_nothing() {
    let (_dir, db) = legacy_db();
    let fresh = remined_excel();
    let mut scanned = 0_u64;
    assert!(
        migrate_legacy_routine_ids(&db, std::slice::from_ref(&fresh), &mut scanned)
            .expect("first")
            .is_some()
    );
    let kv_before = db.scan_cf_prefix(cf::CF_KV, b"").expect("kv snapshot");
    let state_before = db
        .scan_cf_prefix(cf::CF_ROUTINE_STATE, b"")
        .expect("state snapshot");
    assert!(
        migrate_legacy_routine_ids(&db, std::slice::from_ref(&fresh), &mut scanned)
            .expect("second")
            .is_none(),
        "marker row must stop a second pass"
    );
    assert_eq!(
        db.scan_cf_prefix(cf::CF_KV, b"").expect("kv readback"),
        kv_before,
        "the second pass must not rewrite any CF_KV row"
    );
    assert_eq!(
        db.scan_cf_prefix(cf::CF_ROUTINE_STATE, b"")
            .expect("state readback"),
        state_before,
        "the second pass must not rewrite any CF_ROUTINE_STATE row"
    );
    let still_armed = load_armed_routine_record(&db, &fresh.routine_id)
        .expect("read armed")
        .expect("armed row");
    assert!(
        still_armed.enabled,
        "a stable-id armed row survives re-mines"
    );
}

#[test]
fn must_fail_empty_store_migrates_to_an_empty_summary() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(Db::open(dir.path(), SCHEMA_VERSION).expect("open db"));
    let mut scanned = 0_u64;
    let summary = migrate_legacy_routine_ids(&db, &[], &mut scanned)
        .expect("migrate")
        .expect("first pass runs");
    assert_eq!(summary.routines_remapped, 0);
    assert_eq!(summary.state_rows_moved, 0);
    assert_eq!(summary.armed_rows_moved, 0);
    assert_eq!(summary.armed_rows_disarmed, 0);
    assert_eq!(summary.automation_rows_moved, 0);
    assert!(
        db.scan_cf_prefix(cf::CF_ROUTINE_STATE, b"")
            .expect("state readback")
            .is_empty(),
        "an empty store must stay empty"
    );
}
