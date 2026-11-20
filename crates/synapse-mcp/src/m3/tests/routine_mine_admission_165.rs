//! #165 round 2: real mining refusal must precede old-id migration writes.
use super::*;
use crate::m3::routine_identity_ledger::admission_test_hook::with_hook;
use std::cell::Cell;
use std::rc::Rc;

const COMMIT_HEADROOM: usize = 8192;

fn pending_empty_upgrade(db: &Arc<Db>) -> (Owners, RoutineMineParams) {
    let owners = pair_fixture(db);
    clear_cf(db, cf::CF_KV, ROUTINE_IDENTITY_PREFIX.as_bytes());
    clear_cf(db, cf::CF_KV, ROUTINE_ID_MIGRATION_MARKER_KEY.as_bytes());
    let params = replace_episodes(db, None, None);
    assert_eq!(db.pressure_level(), DiskPressureLevel::Normal);
    assert_migration_pending(db, &owners);
    (owners, params)
}

fn assert_migration_pending(db: &Arc<Db>, owners: &Owners) {
    assert!(
        db.get_cf_exact(cf::CF_KV, ROUTINE_ID_MIGRATION_MARKER_KEY.as_bytes())
            .expect("migration marker read")
            .is_none(),
        "#165: refused commit must not mark migration complete"
    );
    for original in owners.records.values() {
        let armed = load_armed_routine_record(db, &original.routine_id)
            .expect("armed owner read")
            .expect("unmatched armed owner retained");
        assert!(armed.enabled, "#165: refused mine disarmed an absent owner");
        assert_eq!(armed.disarmed_at_ns, None);
        assert_eq!(armed.disarm_reason, None);
    }
}

#[test]
fn must_fail_real_mine_under_active_pressure_without_physical_writes_165() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(Db::open(dir.path(), SCHEMA_VERSION).expect("db"));
    let owners = pair_fixture(&db);
    clear_cf(&db, cf::CF_KV, ROUTINE_ID_MIGRATION_MARKER_KEY.as_bytes());
    let params = replace_spellings(&db, Some(" outlook.exe"));
    assert!(!params.dry_run);
    let report = db
        .run_pressure_check_with_free_bytes_sample(0)
        .expect("pressure");
    assert_eq!(report.current_level, DiskPressureLevel::Level4);
    assert!(!db.pressure_permits_write(cf::CF_ROUTINES));
    assert!(!db.pressure_permits_write(cf::CF_ROUTINE_STATE));
    assert!(!db.pressure_permits_write(cf::CF_KV));
    let before = snapshot(&db);
    let ledger_before = db
        .scan_cf_prefix(cf::CF_KV, ROUTINE_IDENTITY_PREFIX.as_bytes())
        .expect("physical ledger before");
    assert!(
        !ledger_before.is_empty(),
        "exercise retained physical owners"
    );
    let error = mine_and_store_routines(&db, &params)
        .expect_err("#165: real mining must refuse already-active disk pressure");
    assert_pressure_error(&error);
    assert_eq!(
        snapshot(&db),
        before,
        "all routine/state/KV bytes unchanged"
    );
    assert_eq!(
        db.scan_cf_prefix(cf::CF_KV, ROUTINE_IDENTITY_PREFIX.as_bytes())
            .expect("physical ledger after"),
        ledger_before
    );
    assert_migration_pending(&db, &owners);
}

#[test]
fn must_fail_pressure_at_ledger_commit_before_old_id_migration_165() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(Db::open(dir.path(), SCHEMA_VERSION).expect("db"));
    let (owners, params) = pending_empty_upgrade(&db);
    let before = snapshot(&db);
    let fired = Rc::new(Cell::new(false));
    let observed = Rc::clone(&fired);
    let result = with_hook(
        move |db| {
            observed.set(true);
            assert_eq!(db.pressure_level(), DiskPressureLevel::Normal);
            let report = db
                .run_pressure_check_with_free_bytes_sample(0)
                .expect("pressure");
            assert_eq!(report.current_level, DiskPressureLevel::Level4);
            assert!(!db.pressure_permits_write(cf::CF_KV));
        },
        || mine_and_store_routines(&db, &params),
    );
    assert!(fired.get(), "must reach real persist after the early gate");
    let error = result.expect_err("#165: ledger admission must shed late pressure");
    assert_pressure_error(&error);
    assert_migration_pending(&db, &owners);
    assert_eq!(
        snapshot(&db),
        before,
        "no migration, backfill or state writes"
    );
}

fn prepare_cap_growth(db: &Db, owners: &Owners) -> (Vec<u8>, Vec<u8>, StoreSnapshot) {
    pad_ledger_to(db, &owners.records["outlook.exe"], LEDGER_BYTE_CAP);
    let key = ledger_key(PADDING_ID);
    let full = db
        .get_cf_exact(cf::CF_KV, &key)
        .expect("padding read")
        .expect("padding");
    // Expected physical state includes ONLY the injected external ledger growth.
    let expected = snapshot(db);
    let mut small: RoutineIdentityOwner =
        synapse_storage::decode_json(&full).expect("padding decode");
    let small_len = small.steps[0].app.len() - COMMIT_HEADROOM;
    small.steps[0].app.truncate(small_len);
    put(db, cf::CF_KV, key.clone(), &small);
    assert_eq!(ledger_bytes(db), LEDGER_BYTE_CAP - COMMIT_HEADROOM);
    assert!(stored_routines(db).iter().map(owner_bytes).sum::<usize>() < COMMIT_HEADROOM);
    (key, full, expected)
}

#[test]
fn must_fail_cap_at_ledger_commit_before_old_id_migration_165() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(Db::open(dir.path(), SCHEMA_VERSION).expect("db"));
    let (owners, params) = pending_empty_upgrade(&db);
    let (key, full, expected) = prepare_cap_growth(&db, &owners);
    let fired = Rc::new(Cell::new(false));
    let observed = Rc::clone(&fired);
    let result = with_hook(
        move |db| {
            observed.set(true);
            assert_eq!(db.pressure_level(), DiskPressureLevel::Normal);
            db.put_batch(cf::CF_KV, [(key, full)])
                .expect("inject cap at commit");
            assert_eq!(ledger_bytes(db), LEDGER_BYTE_CAP);
        },
        || mine_and_store_routines(&db, &params),
    );
    assert!(
        fired.get(),
        "cap must refuse at persist, after old preflight could pass"
    );
    let error = result.expect_err("#165: ledger cap must refuse pending owner backfill");
    assert_cap_error(&error);
    assert_migration_pending(&db, &owners);
    assert_eq!(
        snapshot(&db),
        expected,
        "only injected growth; no migration or backfill writes"
    );
}
