//! The loaded browser table: `browsers/*.json`, loaded once per process.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::endpoint::{BridgeEndpoint, BridgeLane};
use crate::error::{BrowserBridgeError, BrowserRegistryError};
use crate::kind::{BrowserFamily, LEGACY_DEFAULT_KIND};
use crate::row::BrowserRow;

/// Directory name of the row table, beside the installed exe and in the crate.
pub(crate) const ROWS_DIR_NAME: &str = "browsers";
/// The legacy default row compiled in, so the chrome bridge keeps working when
/// no `browsers\` dir is deployed yet (zero behaviour change for Chrome).
const EMBEDDED_LEGACY_ROW: (&str, &str) = (
    "embedded:browsers/chrome.json",
    include_str!("../browsers/chrome.json"),
);

/// All declared browser rows, unique by id, tab prefix, health key, endpoint
/// base and registration token file.
#[derive(Clone, Debug, Default)]
pub struct BrowserRegistry {
    rows: Vec<BrowserRow>,
    origin: String,
}

impl BrowserRegistry {
    /// Process-wide registry: the deployed `browsers\` beside the exe, else the
    /// crate's `browsers/` (dev runs), always containing the legacy default row.
    #[must_use]
    pub fn global() -> &'static Self {
        static GLOBAL: OnceLock<BrowserRegistry> = OnceLock::new();
        GLOBAL.get_or_init(Self::load_default)
    }

    /// Candidate row dirs in precedence order.
    #[must_use]
    pub fn bundled_dirs() -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        if let Ok(exe) = std::env::current_exe()
            && let Some(exe_dir) = exe.parent()
        {
            dirs.push(exe_dir.join(ROWS_DIR_NAME));
        }
        dirs.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(ROWS_DIR_NAME));
        dirs
    }

    fn load_default() -> Self {
        let mut notes = Vec::new();
        for dir in Self::bundled_dirs() {
            if !dir.is_dir() {
                continue;
            }
            match Self::load_dir(&dir) {
                Ok(registry) => return registry.with_legacy_default(),
                Err(error) => {
                    tracing::error!(
                        code = "BROWSER_REGISTRY_LOAD_FAILED",
                        dir = %dir.display(),
                        error = %error,
                        "browser row table failed to load; trying the next source"
                    );
                    notes.push(error.to_string());
                }
            }
        }
        let mut registry = Self::embedded_baseline();
        if !notes.is_empty() {
            registry.origin = format!("{} fallback_after={}", registry.origin, notes.join(";"));
        }
        registry
    }

    /// Registry holding only the compiled-in legacy default row.
    #[must_use]
    pub fn embedded_baseline() -> Self {
        Self::from_json_sources(&[EMBEDDED_LEGACY_ROW]).unwrap_or_else(|error| {
            tracing::error!(
                code = "BROWSER_REGISTRY_EMBEDDED_ROW_INVALID",
                error = %error,
                "compiled-in legacy browser row is invalid; registry is empty"
            );
            Self {
                rows: Vec::new(),
                origin: format!("embedded_invalid error={error}"),
            }
        })
    }

    /// Loads every `*.json` in `dir`, in file-name order.
    ///
    /// # Errors
    /// Unreadable dir/file, invalid row, duplicate key or no rows.
    pub fn load_dir(dir: &Path) -> Result<Self, BrowserRegistryError> {
        let io = |path: &Path, error: std::io::Error| BrowserRegistryError::Io {
            path: path.display().to_string(),
            detail: error.to_string(),
        };
        let mut files = std::fs::read_dir(dir)
            .map_err(|error| io(dir, error))?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_file()
                    && path
                        .extension()
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
            })
            .collect::<Vec<_>>();
        files.sort();
        let mut sources = Vec::with_capacity(files.len());
        for path in &files {
            let text = std::fs::read_to_string(path).map_err(|error| io(path, error))?;
            sources.push((path.display().to_string(), text));
        }
        let borrowed = sources
            .iter()
            .map(|(name, text)| (name.as_str(), text.as_str()))
            .collect::<Vec<_>>();
        let mut registry = Self::from_json_sources(&borrowed)?;
        registry.origin = dir.display().to_string();
        Ok(registry)
    }

    /// Parses `(file name, json text)` rows and enforces uniqueness.
    ///
    /// # Errors
    /// Invalid row, duplicate key or no rows.
    pub fn from_json_sources(sources: &[(&str, &str)]) -> Result<Self, BrowserRegistryError> {
        let mut parsed = Vec::with_capacity(sources.len());
        for (file, text) in sources {
            let row = serde_json::from_str::<BrowserRow>(text).map_err(|error| {
                BrowserRegistryError::RowInvalid {
                    file: (*file).to_owned(),
                    detail: error.to_string(),
                }
            })?;
            row.validate()
                .map_err(|detail| BrowserRegistryError::RowInvalid {
                    file: (*file).to_owned(),
                    detail,
                })?;
            parsed.push(((*file).to_owned(), row));
        }
        if parsed.is_empty() {
            return Err(BrowserRegistryError::Empty {
                origin: "no *.json rows".to_owned(),
            });
        }
        check_unique(&parsed)?;
        Ok(Self {
            rows: parsed.into_iter().map(|(_, row)| row).collect(),
            origin: "sources".to_owned(),
        })
    }

    fn with_legacy_default(mut self) -> Self {
        if self.legacy_default().is_some() {
            return self;
        }
        let embedded = Self::embedded_baseline();
        let conflicts = embedded.rows.iter().any(|legacy| {
            self.rows.iter().any(|row| {
                row.tab_target_prefix == legacy.tab_target_prefix
                    || row.health_key == legacy.health_key
                    || row.registration_token_file == legacy.registration_token_file
            })
        });
        if conflicts {
            tracing::error!(
                code = "BROWSER_REGISTRY_LEGACY_ROW_CONFLICT",
                origin = %self.origin,
                "deployed rows lack the legacy default kind but reuse its wire ids; legacy row not added"
            );
            return self;
        }
        let mut rows = embedded.rows;
        rows.append(&mut self.rows);
        self.rows = rows;
        self.origin = format!("{} +embedded_legacy_default", self.origin);
        self
    }

    #[must_use]
    pub fn rows(&self) -> &[BrowserRow] {
        &self.rows
    }

    /// Where the rows came from (dir path, `sources`, or embedded fallback).
    #[must_use]
    pub fn origin(&self) -> &str {
        &self.origin
    }

    #[must_use]
    pub fn by_id(&self, id: &str) -> Option<&BrowserRow> {
        self.rows.iter().find(|row| row.id.as_str() == id)
    }

    #[must_use]
    pub fn legacy_default(&self) -> Option<&BrowserRow> {
        self.by_id(LEGACY_DEFAULT_KIND)
    }

    #[must_use]
    pub fn by_health_key(&self, health_key: &str) -> Option<&BrowserRow> {
        self.rows.iter().find(|row| row.health_key == health_key)
    }

    /// Row owning a tab target or element id prefix.
    #[must_use]
    pub fn by_tab_target(&self, raw: &str) -> Option<&BrowserRow> {
        let raw = raw.trim();
        self.rows
            .iter()
            .find(|row| raw.starts_with(row.tab_target_prefix.as_str()))
    }

    /// Wire endpoint of `lane` for the row owning `target_id`'s tab prefix,
    /// else the legacy default row (the readback the old literal produced).
    #[must_use]
    pub fn bridge_endpoint_for_target(&self, target_id: &str, lane: BridgeLane) -> String {
        self.by_tab_target(target_id)
            .or_else(|| self.legacy_default())
            .map(|row| row.endpoint(lane))
            .unwrap_or_default()
    }

    /// Like [`Self::bridge_endpoint_for_target`], but a row whose endpoint is
    /// keyed by its extension origin (the legacy chrome form) names the
    /// extension id the worker actually reported, exactly as the old
    /// `chrome_debugger_endpoint(extension_id)` readback did.
    #[must_use]
    pub fn bridge_endpoint_for_reported_target(
        &self,
        target_id: &str,
        reported_extension_id: Option<&str>,
        lane: BridgeLane,
    ) -> String {
        self.bridge_endpoint_for_reported_kind(
            self.by_tab_target(target_id).map(|row| row.id.as_str()),
            reported_extension_id,
            lane,
        )
    }

    /// Same readback for a known browser kind (window-scoped results without
    /// a tab target); an unknown or absent kind uses the legacy default row.
    #[must_use]
    pub fn bridge_endpoint_for_reported_kind(
        &self,
        kind: Option<&str>,
        reported_extension_id: Option<&str>,
        lane: BridgeLane,
    ) -> String {
        let Some(row) = kind
            .and_then(|kind| self.by_id(kind))
            .or_else(|| self.legacy_default())
        else {
            return String::new();
        };
        match reported_extension_id.filter(|id| !id.trim().is_empty()) {
            Some(reported) if row.endpoint_base == row.extension_origin() => format!(
                "{}://{reported}/{}",
                row.extension.origin_scheme,
                lane.wire()
            ),
            _ => row.endpoint(lane),
        }
    }

    /// Row of a bridge endpoint (exact, then legacy `chrome-extension://*`).
    #[must_use]
    pub fn endpoint_kind(&self, raw: &str) -> Option<&BrowserRow> {
        BridgeEndpoint::parse(self, raw)
            .or_else(|| BridgeEndpoint::parse_legacy(self, raw))
            .and_then(|endpoint| self.by_id(endpoint.kind().as_str()))
    }

    /// Replaces the duplicated `is_chrome_bridge_endpoint` /
    /// `is_chrome_debugger_endpoint` helpers: a declared or legacy bridge
    /// endpoint on a tab-control lane.
    #[must_use]
    pub fn is_bridge_tab_lane_endpoint(&self, raw: &str) -> bool {
        BridgeEndpoint::parse(self, raw)
            .or_else(|| BridgeEndpoint::parse_legacy(self, raw))
            .is_some_and(|endpoint| endpoint.lane().is_tab_lane())
    }

    /// Row whose image is `process_name`; rows sharing an image are told apart
    /// by `exe_path_markers`.
    ///
    /// # Errors
    /// `BROWSER_BRIDGE_KIND_UNRESOLVED` when several rows still match.
    pub fn by_process(
        &self,
        process_name: &str,
        process_path: Option<&str>,
    ) -> Result<Option<&BrowserRow>, BrowserBridgeError> {
        let candidates = self
            .rows
            .iter()
            .filter(|row| row.matches_process_name(process_name))
            .filter(|row| row.path_marker_match(process_path) != Some(false))
            .collect::<Vec<_>>();
        if candidates.len() <= 1 {
            return Ok(candidates.first().copied());
        }
        let marked = candidates
            .iter()
            .copied()
            .filter(|row| row.path_marker_match(process_path) == Some(true))
            .collect::<Vec<_>>();
        if let [row] = marked.as_slice() {
            return Ok(Some(*row));
        }
        Err(BrowserBridgeError::kind_unresolved(format!(
            "process {process_name:?} path={} matches browser rows {} and exe_path_markers do not single one out; refusing to guess",
            process_path.unwrap_or("<unknown>"),
            candidates
                .iter()
                .map(|row| row.id.as_str())
                .collect::<Vec<_>>()
                .join(",")
        )))
    }

    /// Lowercase Chromium image names (legacy suffix list plus every Chromium
    /// row's `process_names`), deduplicated.
    #[must_use]
    pub fn chromium_process_names(&self) -> Vec<String> {
        let mut names = self
            .rows
            .iter()
            .filter(|row| row.family == BrowserFamily::Chromium)
            .flat_map(|row| {
                row.legacy_chromium_process_names
                    .iter()
                    .chain(row.process_names.iter())
            })
            .map(|name| name.to_ascii_lowercase())
            .collect::<Vec<_>>();
        names.sort();
        names.dedup();
        names
    }

    /// `is_chromium_family` semantics: case-insensitive suffix match.
    #[must_use]
    pub fn is_chromium_process(&self, process_name: &str) -> bool {
        let lower = process_name.to_ascii_lowercase();
        !lower.is_empty()
            && self
                .chromium_process_names()
                .iter()
                .any(|candidate| lower.ends_with(candidate.as_str()))
    }

    /// Whether a CDP `/json/version` `Browser` value names a Chromium engine.
    #[must_use]
    pub fn cdp_product_is_chromium(&self, product: &str) -> bool {
        self.rows
            .iter()
            .filter(|row| row.family == BrowserFamily::Chromium)
            .flat_map(|row| row.cdp_product_tokens.iter())
            .any(|token| product.contains(token.as_str()))
    }
}

fn check_unique(parsed: &[(String, BrowserRow)]) -> Result<(), BrowserRegistryError> {
    type KeyFn = fn(&BrowserRow) -> &str;
    let keys: [(&'static str, KeyFn); 5] = [
        ("id", |row| row.id.as_str()),
        ("tab_target_prefix", |row| row.tab_target_prefix.as_str()),
        ("health_key", |row| row.health_key.as_str()),
        ("endpoint_base", |row| row.endpoint_base.as_str()),
        ("registration_token_file", |row| {
            row.registration_token_file.as_str()
        }),
    ];
    for (field, key) in keys {
        let mut seen: BTreeMap<String, &str> = BTreeMap::new();
        for (file, row) in parsed {
            let value = key(row).to_ascii_lowercase();
            if let Some(first) = seen.get(&value) {
                return Err(BrowserRegistryError::Duplicate {
                    field,
                    value,
                    first: (*first).to_owned(),
                    second: file.clone(),
                });
            }
            seen.insert(value, file.as_str());
        }
    }
    check_legacy_health_key(parsed)
}

/// Full health always publishes the legacy bridge, even if its row is absent.
fn check_legacy_health_key(parsed: &[(String, BrowserRow)]) -> Result<(), BrowserRegistryError> {
    let (file, text) = EMBEDDED_LEGACY_ROW;
    let legacy = serde_json::from_str::<BrowserRow>(text).map_err(|error| {
        BrowserRegistryError::RowInvalid {
            file: file.to_owned(),
            detail: error.to_string(),
        }
    })?;
    if let Some((second, row)) = parsed
        .iter()
        .find(|(_, row)| !row.id.is_legacy_default() && row.health_key == legacy.health_key)
    {
        return Err(BrowserRegistryError::Duplicate {
            field: "health_key",
            value: row.health_key.clone(),
            first: file.to_owned(),
            second: second.clone(),
        });
    }
    Ok(())
}
