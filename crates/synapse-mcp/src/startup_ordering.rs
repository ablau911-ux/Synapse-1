//! #170: startup steps that act on process-global, daemon-owned state.
//!
//! Crash recovery releases every key/button/pad in the previous daemon's
//! ledger and deletes the ledger. Running it before the single-instance lock
//! let a refused second launch release the LIVE daemon's held inputs. The
//! `&SingleInstanceGuard` parameter makes the order a compile-time rule: the
//! only way to run recovery is to already own the lock.

use std::path::Path;

use anyhow::Context;

use crate::single_instance::SingleInstanceGuard;

pub(crate) fn recover_stale_inputs_as_lock_owner(
    _lock_owner: &SingleInstanceGuard,
    db_path: Option<&Path>,
) -> anyhow::Result<()> {
    let recovery_file = synapse_action::configure_crash_recovery_file(db_path)
        .context("configure action crash recovery ledger")?;
    let recovery_report = synapse_action::recover_stale_inputs_from_configured_path()
        .context("recover stale action inputs from previous daemon")?;
    tracing::info!(
        code = "ACTION_CRASH_RECOVERY_CONFIGURED",
        recovery_file = %recovery_file.display(),
        recovered_keys = recovery_report.recovered_keys,
        recovered_buttons = recovery_report.recovered_buttons,
        recovered_pads = recovery_report.recovered_pads,
        ignored_trailing_bytes = recovery_report.ignored_trailing_bytes,
        "action crash recovery ledger configured"
    );
    Ok(())
}

#[cfg(test)]
#[path = "tests/startup_lock_before_recovery_170.rs"]
mod startup_lock_before_recovery_170;
