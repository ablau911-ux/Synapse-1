//! One declared browser row (`browsers/<id>.json`) and its validation.

use serde::{Deserialize, Serialize};

use crate::endpoint::BridgeLane;
use crate::inspector::OriginRule;
use crate::kind::{BrowserFamily, BrowserKindId};

/// Extension identity and install-page facts for one browser.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionSpec {
    /// Extension id (Chromium: the 32-char id fixed by the manifest `key`;
    /// Gecko: the gecko id).
    pub id: String,
    /// `chrome-extension` (Chromium) or `moz-extension` (Gecko).
    pub origin_scheme: String,
    pub extensions_page_url: String,
    /// Source dir under `extensions/`.
    pub manifest_dir: String,
    /// Deploy dir under `%LOCALAPPDATA%\synapse\`.
    pub deploy_subdir: String,
}

/// Enterprise policy root used for the `ExtensionSettings` popup self-shield.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicySpec {
    pub root: String,
    pub extension_settings_value: String,
}

/// UI Automation names the installer clicks on the extensions page.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallUiSpec {
    pub developer_mode_names: Vec<String>,
    pub load_unpacked_names: Vec<String>,
}

/// On-disk profile layout below `user_data_root`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileLayout {
    /// `Local State` plus one `<Profile>\Preferences` per profile.
    Multi,
    /// `Preferences` directly in the root (Opera).
    Single,
    /// `profiles.ini` / `installs.ini` (Firefox).
    Gecko,
}

const fn default_true() -> bool {
    true
}

/// One browser the bridge can drive. `deny_unknown_fields`: a typo in the data
/// table fails the load instead of silently dropping a setting.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserRow {
    pub id: BrowserKindId,
    pub label: String,
    pub family: BrowserFamily,
    /// Image names that resolve a window to this kind (lowercase `*.exe`).
    pub process_names: Vec<String>,
    /// Raw-CDP lane image names (`is_chromium_family` parity: the 12 legacy
    /// names, suffix-matched). Union over all rows forms the Chromium set.
    #[serde(default)]
    pub legacy_chromium_process_names: Vec<String>,
    /// Path fragments that disambiguate rows sharing an image name.
    #[serde(default)]
    pub exe_path_markers: Vec<String>,
    /// `%VAR%`-expanded install candidates, in probe order.
    #[serde(default)]
    pub exe_candidates: Vec<String>,
    #[serde(default)]
    pub app_paths_name: Option<String>,
    pub user_data_root: String,
    pub profile_layout: ProfileLayout,
    pub window_title_suffixes: Vec<String>,
    pub tab_target_prefix: String,
    /// Endpoint base; the wire endpoint is `<endpoint_base>/<lane>`.
    pub endpoint_base: String,
    pub health_key: String,
    /// File name under `%APPDATA%\synapse\` holding this kind's token.
    pub registration_token_file: String,
    pub extension: ExtensionSpec,
    #[serde(default)]
    pub policy: Option<PolicySpec>,
    #[serde(default)]
    pub native_messaging_cleanup_roots: Vec<String>,
    /// Component/UI extensions that popup suppression must never disable.
    #[serde(default)]
    pub protected_extension_ids: Vec<String>,
    /// Substrings of CDP `/json/version` `Browser` that identify this engine.
    #[serde(default)]
    pub cdp_product_tokens: Vec<String>,
    /// Bridge capabilities this browser cannot provide; commands needing one
    /// fail per command with `BROWSER_BRIDGE_CAPABILITY_UNSUPPORTED`.
    #[serde(default)]
    pub unsupported_capabilities: Vec<String>,
    /// Whether a live host must report `chrome.debugger` availability.
    #[serde(default = "default_true")]
    pub debugger_api_expected: bool,
    #[serde(default)]
    pub install_ui: Option<InstallUiSpec>,
    #[serde(default)]
    pub winget_id: Option<String>,
}

impl BrowserRow {
    /// Extension origin without trailing slash, e.g. `chrome-extension://<id>`.
    #[must_use]
    pub fn extension_origin(&self) -> String {
        format!("{}://{}", self.extension.origin_scheme, self.extension.id)
    }

    /// Static origin rule for Chromium rows (the id is fixed by the manifest
    /// key). Gecko origins are per-profile and come from the gecko inspector.
    #[must_use]
    pub fn chromium_origin_rule(&self) -> OriginRule {
        match self.family {
            BrowserFamily::Chromium => OriginRule::Exact(self.extension_origin()),
            BrowserFamily::Gecko => OriginRule::Unknown {
                reason: "gecko_origin_is_per_profile_uuid".to_owned(),
            },
        }
    }

    #[must_use]
    pub fn endpoint(&self, lane: BridgeLane) -> String {
        format!("{}/{}", self.endpoint_base, lane.wire())
    }

    /// Wire tab target for an already-validated decimal tab id.
    #[must_use]
    pub fn tab_target_wire(&self, tab_id: &str) -> String {
        format!("{}{tab_id}", self.tab_target_prefix)
    }

    #[must_use]
    pub fn supports_capability(&self, capability: &str) -> bool {
        !self
            .unsupported_capabilities
            .iter()
            .any(|unsupported| unsupported == capability)
    }

    /// Global required set minus this row's unsupported capabilities.
    #[must_use]
    pub fn required_capabilities<'a>(&self, global: &[&'a str]) -> Vec<&'a str> {
        global
            .iter()
            .copied()
            .filter(|capability| self.supports_capability(capability))
            .collect()
    }

    /// Window title without this browser's suffix (U+200B variants included
    /// when the row lists them).
    #[must_use]
    pub fn strip_window_title_suffix<'a>(&self, title: &'a str) -> &'a str {
        self.window_title_suffixes
            .iter()
            .find_map(|suffix| title.strip_suffix(suffix.as_str()))
            .unwrap_or(title)
    }

    /// Whether `process_name` (bare name or full path) is one of this row's
    /// images, case-insensitively; a bare stem matches `<stem>.exe`.
    #[must_use]
    pub fn matches_process_name(&self, process_name: &str) -> bool {
        let name = file_name_lower(process_name);
        if name.is_empty() {
            return false;
        }
        self.process_names.iter().any(|candidate| {
            let candidate = candidate.to_ascii_lowercase();
            candidate == name || candidate == format!("{name}.exe")
        })
    }

    /// `Some(true/false)` when the row declares path markers, `None` otherwise.
    #[must_use]
    pub fn path_marker_match(&self, process_path: Option<&str>) -> Option<bool> {
        if self.exe_path_markers.is_empty() {
            return None;
        }
        let path = process_path?.to_ascii_lowercase().replace('/', "\\");
        Some(
            self.exe_path_markers
                .iter()
                .any(|marker| path.contains(&marker.to_ascii_lowercase())),
        )
    }

    /// Structural validation beyond serde.
    ///
    /// # Errors
    /// Returns the first violated rule as text.
    pub fn validate(&self) -> Result<(), String> {
        self.validate_identity()?;
        self.validate_wire_ids()?;
        self.validate_family()
    }

    fn validate_identity(&self) -> Result<(), String> {
        require(!self.label.trim().is_empty(), "label must not be empty")?;
        require(
            !self.process_names.is_empty(),
            "process_names must list at least one image",
        )?;
        for name in &self.process_names {
            require(
                name.ends_with(".exe")
                    && *name == name.to_ascii_lowercase()
                    && !name.contains('\\'),
                &format!("process_names entry {name:?} must be a lowercase bare *.exe name"),
            )?;
        }
        require(
            !self.window_title_suffixes.is_empty(),
            "window_title_suffixes must not be empty",
        )?;
        require(
            !self.user_data_root.trim().is_empty(),
            "user_data_root must not be empty",
        )?;
        require(
            self.unsupported_capabilities
                .iter()
                .all(|capability| !capability.trim().is_empty()),
            "unsupported_capabilities entries must not be empty",
        )
    }

    fn validate_wire_ids(&self) -> Result<(), String> {
        let prefix_ok = self
            .tab_target_prefix
            .strip_suffix("-tab:")
            .is_some_and(|stem| BrowserKindId::new(stem).is_ok());
        require(
            prefix_ok,
            &format!(
                "tab_target_prefix {:?} must match [a-z0-9_]+-tab:",
                self.tab_target_prefix
            ),
        )?;
        require(
            BrowserKindId::new(&self.health_key).is_ok(),
            &format!("health_key {:?} must match [a-z0-9_]+", self.health_key),
        )?;
        let token = &self.registration_token_file;
        require(
            !token.contains(['\\', '/', ':']) && !token.contains("..") && token.ends_with(".txt"),
            &format!("registration_token_file {token:?} must be a bare *.txt file name"),
        )?;
        require(
            self.endpoint_base.contains("://") && !self.endpoint_base.ends_with('/'),
            &format!(
                "endpoint_base {:?} must be <scheme>://<authority> without trailing slash",
                self.endpoint_base
            ),
        )?;
        require(
            !self.extension.id.trim().is_empty(),
            "extension.id must not be empty",
        )
    }

    fn validate_family(&self) -> Result<(), String> {
        let (scheme, layout_ok) = match self.family {
            BrowserFamily::Chromium => (
                "chrome-extension",
                self.profile_layout != ProfileLayout::Gecko,
            ),
            BrowserFamily::Gecko => ("moz-extension", self.profile_layout == ProfileLayout::Gecko),
        };
        require(
            self.extension.origin_scheme == scheme,
            &format!(
                "extension.origin_scheme must be {scheme:?} for family {}",
                self.family.as_str()
            ),
        )?;
        require(
            layout_ok,
            &format!(
                "profile_layout does not fit family {}",
                self.family.as_str()
            ),
        )?;
        require(
            self.family == BrowserFamily::Chromium || !self.debugger_api_expected,
            "debugger_api_expected must be false for family gecko (no chrome.debugger)",
        )
    }
}

fn require(condition: bool, message: &str) -> Result<(), String> {
    if condition {
        Ok(())
    } else {
        Err(message.to_owned())
    }
}

fn file_name_lower(process_name: &str) -> String {
    process_name
        .trim()
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
}
