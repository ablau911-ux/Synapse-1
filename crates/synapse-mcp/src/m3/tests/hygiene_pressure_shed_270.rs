//! #270: the hygiene flag write gate refuses a pressure-shed write with
//! `STORAGE_WRITE_SHED`, while a genuine I/O failure after the gate keeps
//! `STORAGE_WRITE_FAILED`. Child of hygiene_span_digest_161_tests: reuses its
//! runtime and flag fixtures.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;

use crate::m3::tests::pressure_shed_support_270::{
    assert_shed, assert_write_failed, force_pressure, snapshot,
};

#[test]
fn must_fail_hygiene_flag_write_pressure_refusal_is_shed_and_writes_nothing_270()
-> anyhow::Result<()> {
    let (_temp, db, runtime) = runtime_with_db()?;
    let records = vec![keyed_flag_for(INJECTION)?];
    force_pressure(&db);
    let before = snapshot(&db);
    let runtime = runtime.lock().unwrap();
    let error =
        write_flag_records(&runtime, records).expect_err("pressure must refuse the flag write");
    assert_shed(&error, "hygiene flag write");
    assert_eq!(snapshot(&db), before, "a refusal writes nothing");
    assert!(flag_rows(&db)?.is_empty(), "no flag row was written");
    Ok(())
}

/// Guard: a real write failure after the gate (injected WAL flush failure at
/// normal pressure) is not a shed and keeps its own code.
#[test]
fn must_pass_genuine_flag_write_failure_keeps_storage_write_failed_270() -> anyhow::Result<()> {
    let (_temp, db, runtime) = runtime_with_db()?;
    let records = vec![keyed_flag_for(INJECTION)?];
    let runtime = runtime.lock().unwrap();
    // Settle the one-time legacy-row migration so it cannot consume the fault.
    span_digest::ensure_legacy_flags_migrated(&runtime);
    assert!(
        db.pressure_permits_write(cf::CF_KV),
        "precondition: no disk pressure"
    );
    db.fail_next_flush_for_test();
    let error = write_flag_records(&runtime, records)
        .expect_err("an injected flush failure is a real write failure");
    assert_write_failed(&error, "hygiene flag write");
    assert!(
        !error.message.contains("disk pressure"),
        "a genuine failure must not read as a shed: {}",
        error.message
    );
    Ok(())
}
