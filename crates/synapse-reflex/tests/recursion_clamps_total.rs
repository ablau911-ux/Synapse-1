//! Regression test for #60: `recursion_clamps_total` pages the audit CF and
//! survives corrupt rows instead of failing wholesale.

use std::sync::Arc;

use serde_json::json;
use synapse_action::ActionHandle;
use synapse_core::{SCHEMA_VERSION, error_codes};
use synapse_reflex::{EventBus, ReflexRuntime};
use synapse_storage::{Db, cf};
use tempfile::tempdir;

fn audit_row(id: &str, error_code: Option<&str>) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let row = json!({
        "schema_version": SCHEMA_VERSION,
        "audit_id": id,
        "reflex_id": "clamp-test",
        "ts_ns": 1,
        "status": "active",
        "error_code": error_code,
        "details": null,
        "redacted": false,
    });
    Ok(serde_json::to_vec(&row)?)
}

#[test]
fn must_pass_counts_clamps_across_pages_and_skips_corrupt_rows()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let db = Arc::new(Db::open(&temp.path().join("db"), SCHEMA_VERSION)?);
    let mut rows = Vec::new();
    for index in 0..2_500_u32 {
        let clamp = index % 2 == 0;
        let code = clamp.then_some(error_codes::REFLEX_RECURSION_LIMIT);
        rows.push((
            format!("audit-{index:06}").into_bytes(),
            audit_row(&format!("a{index}"), code)?,
        ));
    }
    rows.push((b"audit-corrupt".to_vec(), b"{not-json".to_vec()));
    db.put_batch(cf::CF_REFLEX_AUDIT, rows)?;

    let (handle, _rx) = ActionHandle::channel();
    let runtime = ReflexRuntime::spawn(Arc::clone(&db), handle, EventBus::default())?;
    assert_eq!(runtime.recursion_clamps_total()?, 1_250);
    Ok(())
}
