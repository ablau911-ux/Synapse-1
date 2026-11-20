//! Add-on install state from a Firefox profile's `extensions.json`
//! (`XPIDatabase.sys.mjs`: `{ "schemaVersion", "addons": [ { "id", "version",
//! "active", "userDisabled", "appDisabled", "signedState", "path", ... } ] }`).
//!
//! `signedState` values (`AddonManager.sys.mjs`): -2 broken, -1 unknown,
//! 0 missing, 1 preliminary, 2 signed, 3 system, 4 privileged; absent means
//! signing is not required for that add-on.

use serde_json::Value;

/// Firefox engine layout file name.
pub const EXTENSIONS_JSON_FILE: &str = "extensions.json";

const SIGNED_STATE_BROKEN: i64 = -2;
const SIGNED_STATE_MISSING: i64 = 0;

/// One add-on row of `extensions.json`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeckoAddonRecord {
    pub version: Option<String>,
    pub active: bool,
    pub user_disabled: bool,
    pub app_disabled: bool,
    pub signed_state: Option<i64>,
    pub path: Option<String>,
}

impl GeckoAddonRecord {
    /// Finds `extension_id` in `extensions.json` text.
    ///
    /// # Errors
    /// The text is not JSON or has no `addons` array.
    pub fn find(extensions_json: &str, extension_id: &str) -> Result<Option<Self>, String> {
        let parsed: Value = serde_json::from_str(extensions_json)
            .map_err(|error| format!("extensions_json_invalid: {error}"))?;
        let addons = parsed
            .get("addons")
            .and_then(Value::as_array)
            .ok_or_else(|| "extensions_json_without_addons_array".to_owned())?;
        Ok(addons
            .iter()
            .find(|addon| addon.get("id").and_then(Value::as_str) == Some(extension_id))
            .map(Self::from_value))
    }

    fn from_value(addon: &Value) -> Self {
        let flag = |key: &str| addon.get(key).and_then(Value::as_bool).unwrap_or(false);
        Self {
            version: addon
                .get("version")
                .and_then(Value::as_str)
                .map(str::to_owned),
            active: flag("active"),
            user_disabled: flag("userDisabled"),
            app_disabled: flag("appDisabled"),
            signed_state: addon.get("signedState").and_then(Value::as_i64),
            path: addon
                .get("path")
                .and_then(Value::as_str)
                .filter(|path| !path.trim().is_empty())
                .map(str::to_owned),
        }
    }

    /// `Ok(reason)` when Firefox runs the add-on, `Err(reason)` otherwise.
    /// Firefox's own verdict (`active`, `appDisabled`) decides; a broken
    /// signature is never ready (tamper signal), and an unsigned add-on that
    /// Firefox still runs (signatures not enforced: Dev/Nightly/ESR) is ready
    /// with a reason that says so. Every reason carries `signed_state`.
    ///
    /// # Errors
    /// The machine-readable not-ready reason.
    pub fn readiness(&self) -> Result<String, String> {
        let signed = self
            .signed_state
            .map_or_else(|| "absent".to_owned(), |state| state.to_string());
        let with_state = |reason: &str| format!("{reason}:signed_state={signed}");
        if self.signed_state == Some(SIGNED_STATE_BROKEN) {
            return Err(with_state("extension_signature_broken"));
        }
        if self.user_disabled {
            return Err(with_state("extension_user_disabled"));
        }
        if self.app_disabled {
            return Err(with_state("extension_app_disabled"));
        }
        if !self.active {
            return Err(with_state("extension_inactive"));
        }
        if self
            .signed_state
            .is_some_and(|state| state <= SIGNED_STATE_MISSING)
        {
            return Ok(with_state("extension_ready_signature_not_enforced"));
        }
        Ok(with_state("extension_ready"))
    }
}
