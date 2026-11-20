use super::*;
use crate::server::g7b_support::{code, now_ns};

const ID: &str = "sg1-00000000000000000000000000000097";

fn db() -> (tempfile::TempDir, Arc<Db>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(Db::open(dir.path(), synapse_core::SCHEMA_VERSION).expect("db"));
    (dir, db)
}

fn live(now: u64) -> SuggestionRecord {
    serde_json::from_value(serde_json::json!({
        "record_version": 1, "suggestion_id": ID,
        "routine_id": "rt1-0000000000000097", "source": "routine_intent",
        "created_ts_ns": now, "expiry_ts_ns": now + 600_000_000_000,
        "status": "live", "confidence": 0.9, "matched_prefix_len": 1,
        "total_steps": 2, "remaining_step_count": 1
    }))
    .expect("valid live fixture")
}

#[test]
fn must_pass_existing_live_suggestion_is_accepted_with_exact_readback_97() {
    let (_dir, db) = db();
    let now = now_ns();
    let original = live(now);
    write_suggestion(&db, &original).expect("seed via production writer");
    assert_eq!(
        load_suggestion_by_id(&db, ID).expect("before"),
        Some(original)
    );
    let accepted = accept_suggestion_for_execution(&db, ID, now + 1, "plan/g7b", "exec/g7b", false)
        .expect("live acceptance");
    assert_eq!(accepted.status, SuggestionStatus::Accepted);
    assert_eq!(accepted.proposed_plan_ref.as_deref(), Some("plan/g7b"));
    assert_eq!(accepted.resolved_ts_ns, Some(now + 1));
    assert_eq!(
        load_suggestion_by_id(&db, ID).expect("after"),
        Some(accepted)
    );
}

#[test]
fn must_fail_absence_corruption_and_nonlive_remain_distinct_97() {
    let (_dir, db) = db();
    assert_eq!(
        load_suggestion_by_id(&db, ID).expect("absent precondition"),
        None
    );
    let before = db.scan_cf(cf::CF_KV).expect("before");
    let error =
        accept_suggestion_for_execution(&db, ID, now_ns(), "p", "e", false).expect_err("absent");
    assert_eq!(code(&error), "SUGGESTION_NOT_FOUND");
    assert_eq!(db.scan_cf(cf::CF_KV).expect("after absence"), before);
    let mut record = live(now_ns());
    record.status = SuggestionStatus::Expired;
    write_suggestion(&db, &record).expect("seed nonlive");
    let before = db.scan_cf(cf::CF_KV).expect("nonlive before");
    let error =
        accept_suggestion_for_execution(&db, ID, now_ns(), "p", "e", false).expect_err("nonlive");
    assert_eq!(code(&error), "TOOL_PARAMS_INVALID");
    assert_eq!(db.scan_cf(cf::CF_KV).expect("nonlive after"), before);
    let index = suggestion_id_index_key(ID);
    db.put_batch(cf::CF_KV, [(index.clone(), b"{broken".to_vec())])
        .expect("corrupt seed");
    assert_eq!(
        db.get_cf_exact(cf::CF_KV, &index).expect("corrupt before"),
        Some(b"{broken".to_vec())
    );
    let before = db.scan_cf(cf::CF_KV).expect("corrupt snapshot");
    let error = accept_suggestion_for_execution(&db, ID, now_ns(), "p", "e", false)
        .expect_err("corrupt index");
    assert_eq!(code(&error), "STORAGE_CORRUPTED");
    assert_eq!(db.scan_cf(cf::CF_KV).expect("corrupt after"), before);
}
