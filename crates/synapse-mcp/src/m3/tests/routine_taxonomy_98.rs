//! Wire as child of routine_missing_98_tests to reuse its exact storage fixtures.
use super::*;

fn untouched(db: &Db) -> Vec<(Vec<u8>, Vec<u8>)> {
    db.scan_cf(cf::CF_ROUTINE_STATE).expect("state readback")
}

#[test]
fn must_pass_inspect_confirm_and_feedback_use_real_state_rows_98() {
    let (_dir, db) = db_with_state_only_row();
    put_mined(&db, MINED);
    let inspected = inspect_routine(
        &db,
        &RoutineInspectParams {
            routine_id: MINED.into(),
        },
    )
    .expect("mined inspect");
    assert!(inspected.mined);
    assert!(!inspected.state_row_exists);
    let params: RoutineUpdateParams = serde_json::from_value(serde_json::json!({
        "routine_id": MINED, "action": "confirm"
    }))
    .expect("params");
    let confirmed = update_routine(&db, &params, "g7b").expect("confirm");
    assert_eq!(confirmed.lifecycle_after, RoutineLifecycle::Confirmed);
    let feedback: RoutineFeedbackParams = serde_json::from_value(serde_json::json!({
        "routine_id": MINED, "outcome": "accepted", "now_ts_ns": now_ts_ns()
    }))
    .expect("feedback params");
    let accepted = record_routine_feedback(&db, &feedback, "g7b").expect("feedback");
    assert_eq!(accepted.accept_count, 1);
    let readback = inspect_routine(
        &db,
        &RoutineInspectParams {
            routine_id: MINED.into(),
        },
    )
    .expect("physical readback");
    assert_eq!(readback.state.lifecycle, RoutineLifecycle::Confirmed);
    assert_eq!(readback.state.accept_count, 1);
}

#[test]
fn must_fail_absence_is_domain_error_on_inspect_feedback_and_update_98() {
    let (_dir, db) = db_with_state_only_row();
    assert!(
        load_routine_record(&db, ABSENT)
            .expect("record read")
            .is_none()
    );
    assert!(load_state_row(&db, ABSENT).expect("state read").is_none());
    let before = untouched(&db);
    let inspect = inspect_routine(
        &db,
        &RoutineInspectParams {
            routine_id: ABSENT.into(),
        },
    )
    .expect_err("absent inspect");
    assert_eq!(code_of(&inspect), "ROUTINE_NOT_FOUND");
    let feedback: RoutineFeedbackParams = serde_json::from_value(serde_json::json!({
        "routine_id": ABSENT, "outcome": "accepted"
    }))
    .expect("feedback");
    let error = record_routine_feedback(&db, &feedback, "g7b").expect_err("absent feedback");
    assert_eq!(code_of(&error), "ROUTINE_NOT_FOUND");
    for action in ["confirm", "disable", "enable", "archive"] {
        let params = serde_json::from_value(serde_json::json!({
            "routine_id": ABSENT, "action": action
        }))
        .expect("update");
        let error = update_routine(&db, &params, "g7b").expect_err("absent update");
        assert_eq!(code_of(&error), "ROUTINE_NOT_FOUND", "{action}");
        assert_eq!(untouched(&db), before);
    }
}

#[test]
fn must_fail_malformed_and_corrupt_routines_preserve_truthful_errors_98() {
    let (_dir, db) = db_with_state_only_row();
    let error = inspect_routine(
        &db,
        &RoutineInspectParams {
            routine_id: "bad".into(),
        },
    )
    .expect_err("malformed");
    assert_eq!(code_of(&error), "TOOL_PARAMS_INVALID");
    let key = routine_codec::routine_key(ABSENT).expect("valid id");
    let bytes = b"{broken".to_vec();
    db.put_batch(cf::CF_ROUTINES, [(key.clone(), bytes.clone())])
        .expect("seed corrupt");
    assert_eq!(
        db.get_cf_exact(cf::CF_ROUTINES, &key).expect("before"),
        Some(bytes.clone())
    );
    let error = inspect_routine(
        &db,
        &RoutineInspectParams {
            routine_id: ABSENT.into(),
        },
    )
    .expect_err("corrupt record");
    assert_eq!(code_of(&error), "STORAGE_CORRUPTED");
    assert_eq!(
        db.get_cf_exact(cf::CF_ROUTINES, &key).expect("after"),
        Some(bytes)
    );
}
