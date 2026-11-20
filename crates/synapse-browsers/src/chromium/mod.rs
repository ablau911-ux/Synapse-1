//! Chromium family behaviour: profile inspection, origin rule and the policy
//! self-shield evaluation, all parametrized by the browser row.

mod policy;
mod profile_scan;

use std::path::Path;

pub use policy::{PolicyShieldEvaluation, SHIELDED_PERMISSION, evaluate_self_policy_shield};
pub use profile_scan::{
    LOCAL_STATE_FILE, PREFERENCE_FILES, SERVICE_WORKER_FILE, SNAPSHOTS_DIR,
    active_profile_candidates, extension_setting_path, profile_dirs, resolve_active_profile,
    scan_extension_install,
};

use crate::discovery::expand_process_env_vars;
use crate::inspector::{BridgeInstallState, BrowserProfileInspector, OriginRule};
use crate::row::BrowserRow;

/// Inspector for every `family: "chromium"` row.
#[derive(Clone, Copy, Debug, Default)]
pub struct ChromiumProfileInspector;

impl ChromiumProfileInspector {
    /// Scans an explicit user-data root (fixtures, tests, overrides).
    #[must_use]
    pub fn scan_root(row: &BrowserRow, user_data_root: &Path) -> BridgeInstallState {
        scan_extension_install(row, user_data_root)
    }
}

impl BrowserProfileInspector for ChromiumProfileInspector {
    fn install_state(&self, row: &BrowserRow) -> BridgeInstallState {
        match expand_process_env_vars(&row.user_data_root) {
            Some(root) => Self::scan_root(row, Path::new(&root)),
            None => BridgeInstallState::not_scanned("user_data_root_env_unset"),
        }
    }

    fn expected_origin(&self, row: &BrowserRow) -> OriginRule {
        row.chromium_origin_rule()
    }
}
