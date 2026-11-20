//! Gecko (Firefox) family behaviour: which profile is active (`installs.ini` /
//! `profiles.ini`), whether the bridge add-on is installed and running there
//! (`extensions.json`), which worker code it runs (unpacked dir or installed
//! XPI) and which `moz-extension://<uuid>` origin may register (`prefs.js`).
//! Every unknown state refuses every origin (fail closed).

mod extensions_json;
mod prefs_uuid;
mod profiles_ini;
pub mod xpi;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub use extensions_json::{EXTENSIONS_JSON_FILE, GeckoAddonRecord};
pub use prefs_uuid::{PREFS_JS_FILE, UUID_MAP_PREF, extension_uuid};
pub use profiles_ini::{
    GeckoProfile, GeckoProfileSet, INSTALLS_INI_FILE, PROFILES_DIR, PROFILES_INI_FILE,
};

use crate::discovery::expand_process_env_vars;
use crate::inspector::{BridgeInstallState, BrowserProfileInspector, OriginRule};
use crate::kind::BrowserFamily;
use crate::row::BrowserRow;
use crate::stamp::{parse_code_stamp, sha256_hex_lower};

/// Worker file inside the extension package (shared with the Chromium layout).
const SERVICE_WORKER_FILE: &str = "service_worker.js";
/// The daemon checks the origin on every bridge HTTP request; re-reading
/// `prefs.js` (hundreds of KiB) each time is avoided for this long. Matches the
/// Chromium profile-scan cache TTL in the daemon.
const ORIGIN_CACHE_TTL: Duration = Duration::from_secs(5);

/// Inspector for every `family: "gecko"` row.
#[derive(Clone, Copy, Debug, Default)]
pub struct GeckoProfileInspector;

/// Per-profile add-on lookup result.
enum AddonLookup {
    Found(GeckoAddonRecord),
    Absent,
    Invalid,
}

/// Profiles under one user-data root plus the bridge add-on row per profile.
struct GeckoScan {
    profiles: GeckoProfileSet,
    addons: BTreeMap<String, AddonLookup>,
    active: Option<String>,
}

impl GeckoScan {
    fn read(row: &BrowserRow, root: &Path) -> std::io::Result<Self> {
        let profiles = GeckoProfileSet::read(root)?;
        let addons = profiles
            .profiles()
            .iter()
            .map(|profile| (profile.name().to_owned(), lookup_addon(profile.dir(), row)))
            .collect::<BTreeMap<_, _>>();
        let holding = addons
            .iter()
            .filter(|(_, lookup)| matches!(lookup, AddonLookup::Found(_)))
            .map(|(name, _)| name.clone())
            .collect::<BTreeSet<_>>();
        let active = profiles.resolve_active(&holding);
        Ok(Self {
            profiles,
            addons,
            active,
        })
    }

    fn active_lookup(&self) -> Option<&AddonLookup> {
        self.active.as_ref().and_then(|name| self.addons.get(name))
    }

    fn ready_profiles(&self) -> Vec<String> {
        self.addons
            .iter()
            .filter(|(_, lookup)| {
                matches!(lookup, AddonLookup::Found(record) if record.readiness().is_ok())
            })
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Machine-readable state of the active profile's add-on.
    fn active_reason(&self) -> String {
        let found_elsewhere = self
            .addons
            .values()
            .any(|lookup| matches!(lookup, AddonLookup::Found(_)));
        match self.active_lookup() {
            _ if self.profiles.profiles().is_empty() => "no_gecko_profiles".to_owned(),
            Some(AddonLookup::Found(record)) => match record.readiness() {
                Ok(reason) | Err(reason) => reason,
            },
            Some(AddonLookup::Invalid) => "active_profile_extensions_json_invalid".to_owned(),
            _ if found_elsewhere => "active_profile_missing_extension".to_owned(),
            _ => "extension_id_absent_from_extensions_json".to_owned(),
        }
    }

    fn active_record(&self) -> Option<&GeckoAddonRecord> {
        match self.active_lookup() {
            Some(AddonLookup::Found(record)) => Some(record),
            _ => None,
        }
    }

    fn active_dir(&self) -> Option<&Path> {
        let name = self.active.as_deref()?;
        self.profiles.by_name(name).map(GeckoProfile::dir)
    }
}

impl GeckoProfileInspector {
    /// Install state under an explicit user-data root (fixtures, overrides).
    #[must_use]
    pub fn scan_root(row: &BrowserRow, user_data_root: &Path) -> BridgeInstallState {
        let Ok(scan) = GeckoScan::read(row, user_data_root) else {
            return BridgeInstallState {
                user_data_root: Some(user_data_root.to_path_buf()),
                ..BridgeInstallState::not_scanned("profiles_ini_unreadable")
            };
        };
        let extension_path = scan
            .active_record()
            .and_then(|record| record.path.as_deref())
            .map(PathBuf::from);
        let worker = extension_path.as_deref().and_then(read_installed_worker);
        let installed_profiles = scan.ready_profiles();
        BridgeInstallState {
            scanned: true,
            installed: !installed_profiles.is_empty(),
            user_data_root: Some(user_data_root.to_path_buf()),
            active_profile: scan.active.clone(),
            installed_profiles,
            extension_path,
            service_worker_sha256: worker.as_deref().map(sha256_hex_lower),
            code_stamp: worker
                .as_deref()
                .and_then(|bytes| parse_code_stamp(&String::from_utf8_lossy(bytes))),
            reason: scan.active_reason(),
        }
    }

    /// Origin rule under an explicit user-data root, uncached: the active
    /// profile's add-on must be running, and `prefs.js` must map its id to a
    /// canonical UUID.
    #[must_use]
    pub fn origin_for_root(row: &BrowserRow, user_data_root: &Path) -> OriginRule {
        let unknown = |reason: &str| OriginRule::Unknown {
            reason: reason.to_owned(),
        };
        let Ok(scan) = GeckoScan::read(row, user_data_root) else {
            return unknown("profiles_ini_unreadable");
        };
        let Some(record) = scan.active_record() else {
            return unknown(&scan.active_reason());
        };
        if let Err(reason) = record.readiness() {
            return unknown(&reason);
        }
        let Some(prefs) = scan
            .active_dir()
            .and_then(|dir| std::fs::read_to_string(dir.join(PREFS_JS_FILE)).ok())
        else {
            return unknown("prefs_js_unreadable");
        };
        extension_uuid(&prefs, &row.extension.id).map_or_else(
            || unknown("extension_uuid_absent_or_invalid_in_prefs_js"),
            |uuid| OriginRule::Exact(format!("{}://{uuid}", row.extension.origin_scheme)),
        )
    }

    /// The row's expanded user-data root, or the not-scanned reason.
    fn user_data_root(row: &BrowserRow) -> Result<PathBuf, &'static str> {
        if row.family != BrowserFamily::Gecko {
            return Err("row_family_not_gecko");
        }
        expand_process_env_vars(&row.user_data_root)
            .map(PathBuf::from)
            .ok_or("user_data_root_env_unset")
    }
}

impl BrowserProfileInspector for GeckoProfileInspector {
    fn install_state(&self, row: &BrowserRow) -> BridgeInstallState {
        match Self::user_data_root(row) {
            Ok(root) => Self::scan_root(row, &root),
            Err(reason) => BridgeInstallState::not_scanned(reason),
        }
    }

    fn expected_origin(&self, row: &BrowserRow) -> OriginRule {
        match Self::user_data_root(row) {
            Ok(root) => cached_origin(row, &root),
            Err(reason) => OriginRule::Unknown {
                reason: reason.to_owned(),
            },
        }
    }
}

type OriginCache = Mutex<HashMap<(String, PathBuf), (Instant, OriginRule)>>;

fn cached_origin(row: &BrowserRow, root: &Path) -> OriginRule {
    static CACHE: OnceLock<OriginCache> = OnceLock::new();
    let key = (row.id.as_str().to_owned(), root.to_path_buf());
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(guard) = cache.lock()
        && let Some((at, rule)) = guard.get(&key)
        && at.elapsed() < ORIGIN_CACHE_TTL
    {
        return rule.clone();
    }
    // Read outside the lock: concurrent requests never wait on file IO.
    let rule = GeckoProfileInspector::origin_for_root(row, root);
    if let Ok(mut guard) = cache.lock() {
        guard.insert(key, (Instant::now(), rule.clone()));
    }
    rule
}

fn lookup_addon(profile_dir: &Path, row: &BrowserRow) -> AddonLookup {
    let Ok(text) = std::fs::read_to_string(profile_dir.join(EXTENSIONS_JSON_FILE)) else {
        return AddonLookup::Absent;
    };
    match GeckoAddonRecord::find(&text, &row.extension.id) {
        Ok(Some(record)) => AddonLookup::Found(record),
        Ok(None) => AddonLookup::Absent,
        Err(_) => AddonLookup::Invalid,
    }
}

/// Worker bytes from an unpacked extension dir or an installed XPI file.
fn read_installed_worker(extension_path: &Path) -> Option<Vec<u8>> {
    if extension_path.is_dir() {
        return std::fs::read(extension_path.join(SERVICE_WORKER_FILE)).ok();
    }
    let archive = std::fs::read(extension_path).ok()?;
    xpi::read_entry(&archive, SERVICE_WORKER_FILE).ok()
}
