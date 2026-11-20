//! #97: the lower acceptance call must preserve physical-read failures too.
use super::*;
use crate::server::g7b_support::{code, now_ns};

#[test]
fn must_fail_accept_storage_io_keeps_error_and_does_not_write_resolution() {
    let dir = tempfile::tempdir().expect("directory");
    let db = Arc::new(Db::open(dir.path(), synapse_core::SCHEMA_VERSION).expect("DB"));
    let id = "sg1-00000000000000000000000000000097";
    let index = suggestion_id_index_key(id);
    assert_eq!(db.get_cf_exact(cf::CF_KV, &index).expect("before"), None);
    let before = db.scan_cf(cf::CF_KV).expect("snapshot");
    for dry_run in [true, false] {
        db.fail_next_read_for_test(cf::CF_KV, &index);
        let error =
            accept_suggestion_for_execution(&db, id, now_ns(), "plan/fu2", "exec/fu2", dry_run)
                .expect_err("real indexed read fault");
        assert_eq!(code(&error), "STORAGE_READ_FAILED");
        assert!(error.message.contains("injected storage read IO failure"));
        assert_eq!(db.scan_cf(cf::CF_KV).expect("after"), before);
        assert_eq!(
            load_suggestion_by_id(&db, id).expect("fault consumed"),
            None
        );
    }
}
