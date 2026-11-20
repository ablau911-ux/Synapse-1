//! `recursion_clamps_total` reuses a recent count so `health` polls do not
//! rescan the whole audit CF each time; `_fresh` forces a rescan (#60).

use std::sync::Arc;

use serde_json::json;
use synapse_action::ActionHandle;
use synapse_core::{SCHEMA_VERSION, error_codes};
use synapse_reflex::{EventBus, ReflexRuntime};
use synapse_storage::{Db, cf};
use tempfile::tempdir;

fn clamp_row(id: &str) -> Result<(Vec<u8>, Vec<u8>), Box<dyn std::error::Error>> {
    let row = json!({
        "schema_version": SCHEMA_VERSION,
        "audit_id": id,
        "reflex_id": "clamp-cache-test",
        "ts_ns": 1,
        "status": "active",
        "error_code": error_codes::REFLEX_RECURSION_LIMIT,
        "details": null,
        "redacted": false,
    });
    Ok((
        format!("audit-{id}").into_bytes(),
        serde_json::to_vec(&row)?,
    ))
}

#[test]
fn must_pass_repeated_calls_reuse_the_count_until_a_fresh_rescan()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let db = Arc::new(Db::open(&temp.path().join("db"), SCHEMA_VERSION)?);
    db.put_batch(cf::CF_REFLEX_AUDIT, [clamp_row("a")?, clamp_row("b")?])?;
    let (handle, _rx) = ActionHandle::channel();
    let runtime = ReflexRuntime::spawn(Arc::clone(&db), handle, EventBus::default())?;

    assert_eq!(runtime.recursion_clamps_total()?, 2);
    db.put_batch(cf::CF_REFLEX_AUDIT, [clamp_row("c")?])?;
    assert_eq!(
        runtime.recursion_clamps_total()?,
        2,
        "a second call within the TTL must not rescan the audit CF"
    );
    assert_eq!(runtime.recursion_clamps_total_fresh()?, 3);
    assert_eq!(runtime.recursion_clamps_total()?, 3);
    Ok(())
}

fn non_clamp_row(id: &str) -> Result<(Vec<u8>, Vec<u8>), Box<dyn std::error::Error>> {
    let row = json!({
        "schema_version": SCHEMA_VERSION,
        "audit_id": id,
        "reflex_id": "clamp-cache-test",
        "ts_ns": 1,
        "status": "active",
        "error_code": error_codes::REFLEX_TICK_LATE,
        "details": null,
        "redacted": false,
    });
    Ok((
        format!("audit-{id}").into_bytes(),
        serde_json::to_vec(&row)?,
    ))
}

#[test]
fn must_fail_cached_zero_and_non_clamp_or_corrupt_rows_never_inflate_the_count()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let db = Arc::new(Db::open(&temp.path().join("db"), SCHEMA_VERSION)?);
    let (handle, _rx) = ActionHandle::channel();
    let runtime = ReflexRuntime::spawn(Arc::clone(&db), handle, EventBus::default())?;

    assert_eq!(runtime.recursion_clamps_total()?, 0);
    db.put_batch(
        cf::CF_REFLEX_AUDIT,
        [
            clamp_row("a")?,
            non_clamp_row("b")?,
            (b"audit-corrupt".to_vec(), b"{not-json".to_vec()),
        ],
    )?;
    assert_eq!(
        runtime.recursion_clamps_total()?,
        0,
        "a cached zero is still a cached count; it must not trigger a rescan"
    );
    assert_eq!(
        runtime.recursion_clamps_total_fresh()?,
        1,
        "only REFLEX_RECURSION_LIMIT rows count; other codes and corrupt rows do not"
    );
    assert_eq!(runtime.recursion_clamps_total()?, 1);
    Ok(())
}
