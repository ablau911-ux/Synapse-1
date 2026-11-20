//! Row-driven executable discovery: App Paths (HKLM, then HKCU), then the
//! row's `%VAR%`-expanded `exe_candidates`, first existing file wins.

use std::path::{Path, PathBuf};

use crate::registry_win::{RegistryHive, RegistryReader};
use crate::row::BrowserRow;

/// Windows App Paths registration root (OS constant, not browser data).
const APP_PATHS_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\App Paths";

/// A located browser executable and the physical source that named it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocatedExecutable {
    pub path: PathBuf,
    pub source_of_truth: String,
}

type EnvLookup<'a> = &'a dyn Fn(&str) -> Option<String>;
type FileProbe<'a> = &'a dyn Fn(&Path) -> bool;

/// Locator with injectable registry, environment and file probe (testable
/// without touching the host).
pub struct BrowserExecutableLocator<'a> {
    registry: &'a dyn RegistryReader,
    env: EnvLookup<'a>,
    is_file: FileProbe<'a>,
}

impl<'a> BrowserExecutableLocator<'a> {
    #[must_use]
    pub fn new(
        registry: &'a dyn RegistryReader,
        env: EnvLookup<'a>,
        is_file: FileProbe<'a>,
    ) -> Self {
        Self {
            registry,
            env,
            is_file,
        }
    }

    /// First existing executable for `row`.
    #[must_use]
    pub fn locate(&self, row: &BrowserRow) -> Option<LocatedExecutable> {
        self.locate_in_app_paths(row)
            .or_else(|| self.locate_in_candidates(row))
    }

    /// First candidate path for `row` whether or not it exists, for "checked
    /// <path>" diagnostics.
    #[must_use]
    pub fn first_candidate(&self, row: &BrowserRow) -> Option<PathBuf> {
        row.exe_candidates
            .iter()
            .find_map(|candidate| expand_env_vars(candidate, self.env))
            .map(PathBuf::from)
    }

    fn locate_in_app_paths(&self, row: &BrowserRow) -> Option<LocatedExecutable> {
        let name = row.app_paths_name.as_deref()?;
        let subkey = format!("{APP_PATHS_KEY}\\{name}");
        [RegistryHive::LocalMachine, RegistryHive::CurrentUser]
            .into_iter()
            .find_map(|hive| {
                let raw = self.registry.read_string(hive, &subkey, "")?;
                let path = PathBuf::from(raw.trim().trim_matches('"'));
                (self.is_file)(&path).then(|| LocatedExecutable {
                    source_of_truth: format!(
                        "registry {}\\{subkey} (default value) browser_kind={}",
                        hive.label(),
                        row.id
                    ),
                    path,
                })
            })
    }

    fn locate_in_candidates(&self, row: &BrowserRow) -> Option<LocatedExecutable> {
        row.exe_candidates.iter().find_map(|candidate| {
            let path = PathBuf::from(expand_env_vars(candidate, self.env)?);
            (self.is_file)(&path).then(|| LocatedExecutable {
                source_of_truth: format!(
                    "browsers/{}.json exe_candidates entry {candidate:?}",
                    row.id
                ),
                path,
            })
        })
    }
}

/// Expands `%VAR%` references; `None` when a variable is unset or a `%` is
/// unterminated (never a half-expanded path).
#[must_use]
pub fn expand_env_vars(raw: &str, env: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    let mut expanded = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(start) = rest.find('%') {
        expanded.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let end = after.find('%')?;
        let name = &after[..end];
        if name.is_empty() {
            return None;
        }
        expanded.push_str(&env(name)?);
        rest = &after[end + 1..];
    }
    expanded.push_str(rest);
    Some(expanded)
}

/// [`expand_env_vars`] against the live process environment.
#[must_use]
pub fn expand_process_env_vars(raw: &str) -> Option<String> {
    expand_env_vars(raw, &process_env_var)
}

/// Environment lookup against the live process environment (unset or empty
/// is `None`); the locator's production `env` argument.
#[must_use]
pub fn process_env_var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}
