//! #270 shared helpers: force real disk pressure on a real `Db`, snapshot the
//! CFs a refused write could touch, and assert the one refusal contract every
//! pressure gate shares with `routine_mine` (#165): code `STORAGE_WRITE_SHED`,
//! the `mcp_error` data shape, and nothing written.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rmcp::ErrorData;
use synapse_core::error_codes;
use synapse_storage::{Db, DiskPressureLevel, cf};

/// The CFs the pressure-gated writers in `m3` may touch.
const WATCHED_CFS: [&str; 5] = [
    cf::CF_KV,
    cf::CF_ROUTINES,
    cf::CF_ROUTINE_STATE,
    cf::CF_EPISODES,
    cf::CF_OCR_CACHE,
];

pub(crate) type CfSnapshot = Vec<(&'static str, Vec<(Vec<u8>, Vec<u8>)>)>;

/// Real pressure through the real storage sampler: zero free bytes is Level4,
/// which refuses every non-bypass write the gates guard.
pub(crate) fn force_pressure(db: &Db) {
    let report = db
        .run_pressure_check_with_free_bytes_sample(0)
        .expect("pressure sample");
    assert_eq!(report.current_level, DiskPressureLevel::Level4);
    for watched in WATCHED_CFS {
        assert!(
            !db.pressure_permits_write(watched),
            "precondition: {watched} must refuse writes under Level4"
        );
    }
}

pub(crate) fn snapshot(db: &Db) -> CfSnapshot {
    WATCHED_CFS
        .iter()
        .map(|name| (*name, db.scan_cf(name).expect("cf scan")))
        .collect()
}

/// The shape `routine_mine` returns for a shed write (`mcp_error`).
pub(crate) fn reference_shed_error() -> ErrorData {
    crate::m1::mcp_error(error_codes::STORAGE_WRITE_SHED, "reference")
}

/// A pressure refusal is `STORAGE_WRITE_SHED` with the `routine_mine` data
/// shape and says why. `STORAGE_WRITE_FAILED` is reserved for real I/O failures.
pub(crate) fn assert_shed(error: &ErrorData, tool: &str) {
    let reference = reference_shed_error();
    let data = error
        .data
        .as_ref()
        .unwrap_or_else(|| panic!("{tool}: pressure refusal carries no typed data: {error:?}"));
    assert_eq!(
        data["code"],
        error_codes::STORAGE_WRITE_SHED,
        "{tool}: a pressure refusal must be STORAGE_WRITE_SHED, got {error:?}"
    );
    assert_eq!(
        error.data, reference.data,
        "{tool}: data shape differs from routine_mine"
    );
    assert_eq!(error.code, reference.code, "{tool}: JSON-RPC code differs");
    assert!(
        error.message.contains("disk pressure"),
        "{tool}: the refusal must say it was shed by disk pressure: {}",
        error.message
    );
}

/// Real storage I/O failures keep their own code.
pub(crate) fn assert_write_failed(error: &ErrorData, tool: &str) {
    let data = error
        .data
        .as_ref()
        .unwrap_or_else(|| panic!("{tool}: failure carries no typed data: {error:?}"));
    assert_eq!(
        data["code"],
        error_codes::STORAGE_WRITE_FAILED,
        "{tool}: a genuine write failure must stay STORAGE_WRITE_FAILED, got {error:?}"
    );
}
