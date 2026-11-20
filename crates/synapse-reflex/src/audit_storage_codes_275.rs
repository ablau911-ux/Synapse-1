//! #275: inject storage failures below the real audit writers, not a copy of
//! their error mapper. The audit CF is deliberately exempt from pressure shed.
use std::{cell::RefCell, error::Error, sync::Arc};

use chrono::Utc;
use synapse_action::ActionHandle;
use synapse_core::{ReflexLifetime, ReflexState, ReflexStatus, StoredReflexAudit, error_codes};
use synapse_storage::{Db, DiskPressureLevel, StorageError, StorageResult, cf};

use crate::{EventBus, ReflexRuntime};

thread_local! {
    static NEXT_WRITE_ERROR: RefCell<Option<StorageError>> = const { RefCell::new(None) };
}

// Test-only storage boundary. With no injected failure, retain today's real
// write behavior. Thread-local, one-shot faults cannot leak between tests.
pub(super) fn write_audit(db: &Db, audit: &StoredReflexAudit) -> StorageResult<()> {
    if let Some(error) = NEXT_WRITE_ERROR.with(|slot| slot.borrow_mut().take()) {
        return Err(error);
    }
    crate::write_audit(db, audit)
}

#[derive(Clone, Copy, Debug)]
enum Writer {
    Registration,
    Cancellation,
    Disabled,
}

fn check_error(writer: Writer, shed: bool) -> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    let db = Arc::new(Db::open(
        &temp.path().join("db"),
        synapse_core::SCHEMA_VERSION,
    )?);
    let (actions, _rx) = ActionHandle::channel();
    let runtime = ReflexRuntime::spawn(Arc::clone(&db), actions, EventBus::default())?;
    let status = ReflexStatus {
        id: "audit-storage-275".to_owned(),
        kind_summary: "on_event".to_owned(),
        state: ReflexState::Active,
        registered_at: Utc::now(),
        last_fired_at: None,
        fire_count: 0,
        priority: 1,
        lifetime: ReflexLifetime::UntilCancelled,
        exclusive: false,
        last_error_code: None,
        input_target: None,
        last_error: None,
    };
    assert!(db.scan_cf(cf::CF_REFLEX_AUDIT)?.is_empty());
    let storage_error = if shed {
        StorageError::WriteShed {
            cf_name: cf::CF_REFLEX_AUDIT.to_owned(),
            pressure_level: DiskPressureLevel::Level4,
            dropped_rows: 1,
        }
    } else {
        StorageError::WriteFailed {
            cf_name: cf::CF_REFLEX_AUDIT.to_owned(),
            detail: "#275 synthetic storage write failure".to_owned(),
        }
    };
    let expected_code = storage_error.code();
    NEXT_WRITE_ERROR.with(|slot| *slot.borrow_mut() = Some(storage_error));
    let result = match writer {
        Writer::Registration => runtime.write_registration_audit(&status),
        Writer::Cancellation => runtime.write_cancellation_audit(&status),
        Writer::Disabled => runtime.write_disabled_audits_with_reason(&[status], "test-275"),
    };
    let consumed = NEXT_WRITE_ERROR.with(|slot| slot.borrow_mut().take().is_none());
    assert!(
        consumed,
        "test must reach the real {writer:?} audit write call site"
    );
    assert!(
        db.scan_cf(cf::CF_REFLEX_AUDIT)?.is_empty(),
        "failed write persisted an audit"
    );
    let error = result
        .err()
        .ok_or("audit write failure must not be acknowledged")?;
    assert_eq!(
        error.code(),
        expected_code,
        "{writer:?} must preserve StorageError::code()"
    );
    assert_ne!(error.code(), error_codes::REFLEX_PARAMS_INVALID);
    Ok(())
}

#[test]
fn must_fail_registration_preserves_write_failed_275() -> Result<(), Box<dyn Error>> {
    check_error(Writer::Registration, false)
}

#[test]
fn must_fail_registration_preserves_write_shed_275() -> Result<(), Box<dyn Error>> {
    check_error(Writer::Registration, true)
}

#[test]
fn must_fail_cancellation_preserves_write_failed_275() -> Result<(), Box<dyn Error>> {
    check_error(Writer::Cancellation, false)
}

#[test]
fn must_fail_cancellation_preserves_write_shed_275() -> Result<(), Box<dyn Error>> {
    check_error(Writer::Cancellation, true)
}

#[test]
fn must_fail_disabled_preserves_write_failed_275() -> Result<(), Box<dyn Error>> {
    check_error(Writer::Disabled, false)
}

#[test]
fn must_fail_disabled_preserves_write_shed_275() -> Result<(), Box<dyn Error>> {
    check_error(Writer::Disabled, true)
}
