//! Chromium user-data layout reads (moved out of the 9k-line chrome bridge so
//! every Chromium row shares them): `Local State` active-profile candidates,
//! profile enumeration for the `multi` and `single` layouts, and the bridge
//! extension's `extensions.settings.<id>` install row.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::inspector::BridgeInstallState;
use crate::row::{BrowserRow, ProfileLayout};
use crate::stamp::{parse_code_stamp, sha256_hex_lower};

/// Chromium layout file names (engine facts, identical for every Chromium row).
pub const LOCAL_STATE_FILE: &str = "Local State";
pub const PREFERENCE_FILES: [&str; 2] = ["Preferences", "Secure Preferences"];
pub const SNAPSHOTS_DIR: &str = "Snapshots";
pub const SERVICE_WORKER_FILE: &str = "service_worker.js";

/// `profile.last_used` then `profile.last_active_profiles`, deduplicated.
#[must_use]
pub fn active_profile_candidates(user_data_root: &Path) -> Vec<String> {
    let Ok(raw) = std::fs::read_to_string(user_data_root.join(LOCAL_STATE_FILE)) else {
        return Vec::new();
    };
    let Ok(parsed) = serde_json::from_str::<Value>(&raw) else {
        return Vec::new();
    };
    let profile = parsed.get("profile");
    let mut candidates = Vec::new();
    if let Some(last_used) = profile
        .and_then(|profile| profile.get("last_used"))
        .and_then(Value::as_str)
    {
        candidates.push(last_used.to_owned());
    }
    if let Some(last_active) = profile
        .and_then(|profile| profile.get("last_active_profiles"))
        .and_then(Value::as_array)
    {
        candidates.extend(
            last_active
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned),
        );
    }
    let mut seen = BTreeSet::new();
    candidates.retain(|candidate| seen.insert(candidate.clone()));
    candidates
}

/// First installed candidate, else the only installed profile, else the first
/// existing candidate dir.
#[must_use]
pub fn resolve_active_profile(
    user_data_root: &Path,
    candidates: &[String],
    installed_profiles: &BTreeSet<String>,
) -> Option<String> {
    if let Some(candidate) = candidates
        .iter()
        .find(|candidate| installed_profiles.contains(*candidate))
    {
        return Some(candidate.clone());
    }
    if installed_profiles.len() == 1 {
        return installed_profiles.iter().next().cloned();
    }
    candidates
        .iter()
        .find(|candidate| user_data_root.join(candidate).is_dir())
        .cloned()
}

/// `(profile name, profile dir)` for every profile holding a preference file.
///
/// # Errors
/// The root itself is unreadable.
pub fn profile_dirs(
    user_data_root: &Path,
    layout: ProfileLayout,
) -> std::io::Result<Vec<(String, PathBuf)>> {
    let has_prefs = |dir: &Path| PREFERENCE_FILES.iter().any(|file| dir.join(file).is_file());
    match layout {
        ProfileLayout::Gecko => Ok(Vec::new()),
        ProfileLayout::Single => {
            std::fs::metadata(user_data_root)?;
            let name = user_data_root.file_name().map_or_else(
                || ".".to_owned(),
                |name| name.to_string_lossy().into_owned(),
            );
            Ok(if has_prefs(user_data_root) {
                vec![(name, user_data_root.to_path_buf())]
            } else {
                Vec::new()
            })
        }
        ProfileLayout::Multi => {
            let mut dirs = std::fs::read_dir(user_data_root)?
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                .filter(|entry| entry.file_name() != SNAPSHOTS_DIR)
                .map(|entry| {
                    (
                        entry.file_name().to_string_lossy().into_owned(),
                        entry.path(),
                    )
                })
                .filter(|(_, path)| has_prefs(path))
                .collect::<Vec<_>>();
            dirs.sort();
            Ok(dirs)
        }
    }
}

/// Non-empty `extensions.settings.<extension_id>.path` from one preference file.
#[must_use]
pub fn extension_setting_path(preferences: &Value, extension_id: &str) -> Option<String> {
    preferences
        .get("extensions")?
        .get("settings")?
        .get(extension_id)?
        .get("path")
        .and_then(Value::as_str)
        .filter(|path| !path.trim().is_empty())
        .map(str::to_owned)
}

/// Install state of `row`'s bridge extension under `user_data_root`.
#[must_use]
pub fn scan_extension_install(row: &BrowserRow, user_data_root: &Path) -> BridgeInstallState {
    let Ok(profiles) = profile_dirs(user_data_root, row.profile_layout) else {
        return BridgeInstallState {
            user_data_root: Some(user_data_root.to_path_buf()),
            ..BridgeInstallState::not_scanned("user_data_root_unreadable")
        };
    };
    let mut installed = BTreeSet::new();
    let mut paths: Vec<(String, PathBuf)> = Vec::new();
    for (profile, dir) in &profiles {
        for file in PREFERENCE_FILES {
            let Some(path) = std::fs::read_to_string(dir.join(file))
                .ok()
                .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
                .and_then(|prefs| extension_setting_path(&prefs, &row.extension.id))
            else {
                continue;
            };
            installed.insert(profile.clone());
            paths.push((profile.clone(), PathBuf::from(path)));
        }
    }
    let candidates = active_profile_candidates(user_data_root);
    let active_profile = resolve_active_profile(user_data_root, &candidates, &installed);
    let extension_path = active_profile.as_ref().and_then(|active| {
        let mine = paths.iter().filter(|(profile, _)| profile == active);
        mine.clone()
            .map(|(_, path)| path)
            .find(|path| path.join(SERVICE_WORKER_FILE).is_file())
            .or_else(|| mine.map(|(_, path)| path).next())
            .cloned()
    });
    let worker = extension_path
        .as_ref()
        .and_then(|path| std::fs::read(path.join(SERVICE_WORKER_FILE)).ok());
    let reason = if profiles.is_empty() {
        "no_profile_dirs"
    } else if installed.is_empty() {
        "extension_id_absent_from_preferences_and_secure_preferences"
    } else if active_profile
        .as_ref()
        .is_some_and(|active| !installed.contains(active))
    {
        "active_profile_missing_extension"
    } else {
        "extension_profile_row_present"
    };
    BridgeInstallState {
        scanned: true,
        installed: !installed.is_empty(),
        user_data_root: Some(user_data_root.to_path_buf()),
        active_profile,
        installed_profiles: installed.into_iter().collect(),
        extension_path,
        service_worker_sha256: worker.as_deref().map(sha256_hex_lower),
        code_stamp: worker
            .as_deref()
            .and_then(|bytes| parse_code_stamp(&String::from_utf8_lossy(bytes))),
        reason: reason.to_owned(),
    }
}
