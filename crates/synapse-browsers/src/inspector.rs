//! Per-family profile inspection: is the bridge extension installed in the
//! browser's active profile, which worker code is installed, and which
//! extension origin may register.

use std::path::PathBuf;

use crate::chromium::ChromiumProfileInspector;
use crate::gecko::GeckoProfileInspector;
use crate::kind::BrowserFamily;
use crate::row::BrowserRow;

/// Installed-extension readback for one browser row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BridgeInstallState {
    pub scanned: bool,
    pub installed: bool,
    pub user_data_root: Option<PathBuf>,
    pub active_profile: Option<String>,
    pub installed_profiles: Vec<String>,
    pub extension_path: Option<PathBuf>,
    pub service_worker_sha256: Option<String>,
    /// `BRIDGE_CODE_STAMP` of the installed worker (#237).
    pub code_stamp: Option<String>,
    /// Machine-readable reason (`user_data_root_unreadable`,
    /// `extension_id_absent_from_preferences`, `extension_profile_row_present`, ...).
    pub reason: String,
}

impl BridgeInstallState {
    #[must_use]
    pub fn not_scanned(reason: &str) -> Self {
        Self {
            reason: reason.to_owned(),
            ..Self::default()
        }
    }

    /// One-line `key=value` readback for health details.
    #[must_use]
    pub fn detail(&self, row: &BrowserRow) -> String {
        format!(
            "browser_kind={} bridge_profile_installation scanned={} installed={} user_data_root={} active_profile={} installed_profiles={} extension_path={} service_worker_sha256={} code_stamp={} reason={}",
            row.id,
            self.scanned,
            self.installed,
            self.user_data_root.as_ref().map_or_else(
                || "<none>".to_owned(),
                |path| format!("{:?}", path.display().to_string())
            ),
            self.active_profile.as_deref().unwrap_or("<unknown>"),
            if self.installed_profiles.is_empty() {
                "<none>".to_owned()
            } else {
                self.installed_profiles.join(",")
            },
            self.extension_path.as_ref().map_or_else(
                || "<none>".to_owned(),
                |path| format!("{:?}", path.display().to_string())
            ),
            self.service_worker_sha256
                .as_deref()
                .unwrap_or("not_available"),
            self.code_stamp.as_deref().unwrap_or("none"),
            self.reason
        )
    }
}

/// Which extension origin a registration may present.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OriginRule {
    /// Exactly this origin (trailing slash ignored).
    Exact(String),
    /// No origin is known yet (gecko before the profile UUID is read); every
    /// origin is refused.
    Unknown { reason: String },
}

impl OriginRule {
    #[must_use]
    pub fn accepts(&self, origin: &str) -> bool {
        match self {
            Self::Exact(expected) => {
                origin.trim().trim_end_matches('/') == expected.trim_end_matches('/')
            }
            Self::Unknown { .. } => false,
        }
    }
}

/// Family behaviour object for profile inspection.
pub trait BrowserProfileInspector: Send + Sync {
    fn install_state(&self, row: &BrowserRow) -> BridgeInstallState;
    fn expected_origin(&self, row: &BrowserRow) -> OriginRule;
}

/// Inspector for the row's family (the only dispatch on family).
#[must_use]
pub fn inspector_for(row: &BrowserRow) -> Box<dyn BrowserProfileInspector> {
    match row.family {
        BrowserFamily::Chromium => Box::new(ChromiumProfileInspector),
        BrowserFamily::Gecko => Box::new(GeckoProfileInspector),
    }
}
