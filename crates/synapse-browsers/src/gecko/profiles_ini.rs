//! Firefox profile enumeration under the user-data root
//! (`%APPDATA%\Mozilla\Firefox`):
//! - `profiles.ini`: `[ProfileN]` sections (`Path=`, `IsRelative=`,
//!   `Default=1`) and the per-install `[Install<hash>]` `Default=` sections;
//! - `installs.ini`: `[<install hash>]` `Default=` (the default profile of each
//!   Firefox installation, preferred since Firefox 67);
//! - every directory under `Profiles\` (profile-group profiles created by the
//!   new profile manager are not listed in `profiles.ini`).
//!
//! A profile is named by its directory name (`87ieum3w.default-release`), the
//! same name whichever source listed it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Firefox engine layout file and directory names.
pub const PROFILES_INI_FILE: &str = "profiles.ini";
pub const INSTALLS_INI_FILE: &str = "installs.ini";
pub const PROFILES_DIR: &str = "Profiles";

const PROFILE_SECTION_PREFIX: &str = "Profile";
const INSTALL_SECTION_PREFIX: &str = "Install";

/// One Firefox profile directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeckoProfile {
    name: String,
    dir: PathBuf,
}

impl GeckoProfile {
    fn from_dir(dir: PathBuf) -> Option<Self> {
        let name = dir.file_name()?.to_string_lossy().into_owned();
        (!name.is_empty()).then_some(Self { name, dir })
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

/// Profiles plus the default-profile candidates in precedence order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GeckoProfileSet {
    profiles: Vec<GeckoProfile>,
    default_candidates: Vec<String>,
}

type IniSection = (String, Vec<(String, String)>);

impl GeckoProfileSet {
    /// Reads `profiles.ini` (required), `installs.ini` (optional) and the
    /// `Profiles\*` directories under `root`.
    ///
    /// # Errors
    /// `profiles.ini` is unreadable.
    pub fn read(root: &Path) -> std::io::Result<Self> {
        let profiles_ini = std::fs::read_to_string(root.join(PROFILES_INI_FILE))?;
        let installs_ini = std::fs::read_to_string(root.join(INSTALLS_INI_FILE)).ok();
        let mut set = Self::parse(root, &profiles_ini, installs_ini.as_deref());
        set.add_profile_dirs(&root.join(PROFILES_DIR));
        Ok(set)
    }

    /// Parses the two ini files without touching the filesystem.
    #[must_use]
    pub fn parse(root: &Path, profiles_ini: &str, installs_ini: Option<&str>) -> Self {
        let profile_sections = parse_ini(profiles_ini);
        let mut set = Self::default();
        let mut flagged_default = Vec::new();
        for (section, entries) in &profile_sections {
            if !section.starts_with(PROFILE_SECTION_PREFIX) {
                continue;
            }
            let Some(profile) = profile_from_section(root, entries) else {
                continue;
            };
            if value(entries, "Default") == Some("1") {
                flagged_default.push(profile.name.clone());
            }
            set.push_profile(profile);
        }
        let install_ini_sections = installs_ini.map(parse_ini).unwrap_or_default();
        let install_defaults = install_ini_sections
            .iter()
            .chain(
                profile_sections
                    .iter()
                    .filter(|(section, _)| section.starts_with(INSTALL_SECTION_PREFIX)),
            )
            .filter_map(|(_, entries)| value(entries, "Default"))
            .filter_map(|raw| GeckoProfile::from_dir(resolve_profile_path(root, raw, true)));
        for profile in install_defaults {
            set.push_candidate(profile.name.clone());
            set.push_profile(profile);
        }
        for name in flagged_default {
            set.push_candidate(name);
        }
        set
    }

    #[must_use]
    pub fn profiles(&self) -> &[GeckoProfile] {
        &self.profiles
    }

    #[must_use]
    pub fn default_candidates(&self) -> &[String] {
        &self.default_candidates
    }

    #[must_use]
    pub fn by_name(&self, name: &str) -> Option<&GeckoProfile> {
        self.profiles.iter().find(|profile| profile.name == name)
    }

    /// First default candidate holding the extension, else the only profile
    /// holding it, else the first default candidate whose directory exists.
    #[must_use]
    pub fn resolve_active(&self, holding_extension: &BTreeSet<String>) -> Option<String> {
        if let Some(candidate) = self
            .default_candidates
            .iter()
            .find(|candidate| holding_extension.contains(*candidate))
        {
            return Some(candidate.clone());
        }
        if holding_extension.len() == 1 {
            return holding_extension.iter().next().cloned();
        }
        self.default_candidates
            .iter()
            .find(|candidate| self.by_name(candidate).is_some_and(|p| p.dir.is_dir()))
            .cloned()
    }

    fn add_profile_dirs(&mut self, profiles_dir: &Path) {
        let Ok(entries) = std::fs::read_dir(profiles_dir) else {
            return;
        };
        let mut dirs = entries
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        dirs.sort();
        for profile in dirs.into_iter().filter_map(GeckoProfile::from_dir) {
            self.push_profile(profile);
        }
    }

    fn push_profile(&mut self, profile: GeckoProfile) {
        if self.by_name(&profile.name).is_none() {
            self.profiles.push(profile);
        }
    }

    fn push_candidate(&mut self, name: String) {
        if !self.default_candidates.contains(&name) {
            self.default_candidates.push(name);
        }
    }
}

fn profile_from_section(root: &Path, entries: &[(String, String)]) -> Option<GeckoProfile> {
    let raw = value(entries, "Path")?;
    let relative = value(entries, "IsRelative") != Some("0");
    GeckoProfile::from_dir(resolve_profile_path(root, raw, relative))
}

/// `Profiles/x.default` (forward slashes, relative to `root`) or an absolute
/// Windows path.
fn resolve_profile_path(root: &Path, raw: &str, relative: bool) -> PathBuf {
    let raw = raw.trim();
    if !relative || Path::new(raw).is_absolute() || raw.contains(':') {
        return PathBuf::from(raw);
    }
    raw.split(['/', '\\'])
        .filter(|part| !part.is_empty() && *part != ".")
        .fold(root.to_path_buf(), |path, part| path.join(part))
}

fn value<'a>(entries: &'a [(String, String)], key: &str) -> Option<&'a str> {
    entries
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

/// Minimal INI reader: `[section]` headers and `key=value` lines; blank lines
/// and `;` / `#` comments are skipped; a UTF-8 BOM is ignored.
fn parse_ini(text: &str) -> Vec<IniSection> {
    let mut sections: Vec<IniSection> = Vec::new();
    for line in text.trim_start_matches('\u{feff}').lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            sections.push((name.trim().to_owned(), Vec::new()));
        } else if let (Some((key, val)), Some((_, entries))) =
            (line.split_once('='), sections.last_mut())
        {
            entries.push((key.trim().to_owned(), val.trim().to_owned()));
        }
    }
    sections
}
