//! Bridge tab and element target ids.
//!
//! Tab: `<row.tab_target_prefix><decimal tab id>` (chrome: `chrome-tab:42`).
//! Element: `<tab>:frame:<decimal frame id>:path:<dom path>` where the path is
//! dot-separated numeric child indexes or the `s` shadow-root hop (#1335).

use crate::error::BrowserBridgeError;
use crate::kind::BrowserKindId;
use crate::registry::BrowserRegistry;
use crate::row::BrowserRow;

const FRAME_MARKER: &str = ":frame:";
const PATH_MARKER: &str = ":path:";
const SHADOW_HOP_SEGMENT: &str = "s";
/// Generic shape of any bridge tab prefix (`<kind>-tab:`); used to tell an
/// unknown browser kind (fail closed) from a non-bridge id (raw CDP).
const GENERIC_TAB_MARKER: &str = "-tab:";

/// A parsed bridge tab target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BridgeTabTarget {
    kind: BrowserKindId,
    tab_id: u64,
    wire: String,
}

impl BridgeTabTarget {
    #[must_use]
    pub fn new(row: &BrowserRow, tab_id: u64) -> Self {
        Self {
            kind: row.id.clone(),
            tab_id,
            wire: row.tab_target_wire(&tab_id.to_string()),
        }
    }

    #[must_use]
    pub const fn kind(&self) -> &BrowserKindId {
        &self.kind
    }

    #[must_use]
    pub const fn tab_id(&self) -> u64 {
        self.tab_id
    }

    #[must_use]
    pub fn wire(&self) -> &str {
        &self.wire
    }

    /// `Ok(None)` for ids that are not bridge ids at all (raw CDP target ids).
    ///
    /// # Errors
    /// `BROWSER_BRIDGE_KIND_UNRESOLVED` for an undeclared `<kind>-tab:` prefix;
    /// `TOOL_PARAMS_INVALID` for a declared prefix with a malformed tab id.
    pub fn parse(
        registry: &BrowserRegistry,
        raw: &str,
    ) -> Result<Option<Self>, BrowserBridgeError> {
        let trimmed = raw.trim();
        let Some((row, rest)) = split_row_prefix(registry, trimmed) else {
            return unknown_prefix_or_none(trimmed);
        };
        let tab_id = parse_decimal(rest).ok_or_else(|| {
            BrowserBridgeError::params_invalid(format!(
                "bridge tab target {trimmed:?} must be shaped like {}<tabId> with a decimal tab id",
                row.tab_target_prefix
            ))
        })?;
        Ok(Some(Self {
            kind: row.id.clone(),
            tab_id,
            wire: trimmed.to_owned(),
        }))
    }
}

/// A parsed bridge element id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BridgeElementTarget {
    tab: BridgeTabTarget,
    frame_id: u64,
    path: String,
}

impl BridgeElementTarget {
    #[must_use]
    pub const fn tab(&self) -> &BridgeTabTarget {
        &self.tab
    }

    #[must_use]
    pub const fn frame_id(&self) -> u64 {
        self.frame_id
    }

    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// `Ok(None)` for ids that are not bridge element ids at all.
    ///
    /// # Errors
    /// `BROWSER_BRIDGE_KIND_UNRESOLVED` for an undeclared `<kind>-tab:` prefix;
    /// `TOOL_PARAMS_INVALID` for malformed tab/frame/path components.
    pub fn parse(
        registry: &BrowserRegistry,
        raw: &str,
    ) -> Result<Option<Self>, BrowserBridgeError> {
        let trimmed = raw.trim();
        let Some((row, rest)) = split_row_prefix(registry, trimmed) else {
            return unknown_prefix_or_none(trimmed);
        };
        let invalid = || {
            BrowserBridgeError::params_invalid(format!(
                "bridge element_id {trimmed:?} must be shaped like {}<tabId>:frame:<frameId>:path:<domPath> with numeric tab/frame ids and a dot path of child indexes or the shadow hop \"s\"",
                row.tab_target_prefix
            ))
        };
        let (tab_raw, after_frame) = rest.split_once(FRAME_MARKER).ok_or_else(invalid)?;
        let (frame_raw, path) = after_frame.split_once(PATH_MARKER).ok_or_else(invalid)?;
        let tab_id = parse_decimal(tab_raw).ok_or_else(invalid)?;
        let frame_id = parse_decimal(frame_raw).ok_or_else(invalid)?;
        if !valid_dom_path(path) {
            return Err(invalid());
        }
        Ok(Some(Self {
            tab: BridgeTabTarget {
                kind: row.id.clone(),
                tab_id,
                wire: row.tab_target_wire(tab_raw),
            },
            frame_id,
            path: path.to_owned(),
        }))
    }
}

impl BrowserRegistry {
    /// Whether `raw` is a well-formed tab target of a declared row.
    #[must_use]
    pub fn is_bridge_tab_target(&self, raw: &str) -> bool {
        matches!(BridgeTabTarget::parse(self, raw), Ok(Some(_)))
    }

    /// Whether `raw` starts with any declared tab prefix (no trimming; the
    /// legacy `starts_with("chrome-tab:")` checks this replaces did not trim).
    #[must_use]
    pub fn has_bridge_tab_prefix(&self, raw: &str) -> bool {
        self.rows()
            .iter()
            .any(|row| raw.starts_with(row.tab_target_prefix.as_str()))
    }

    /// Tab target of an element id: declared prefix, decimal tab id, then
    /// `:frame:`. Lenient on the rest, like the helper it replaces.
    #[must_use]
    pub fn tab_target_of_element_id(&self, element_id: &str) -> Option<String> {
        let (row, rest) = split_row_prefix(self, element_id)?;
        let digit_count = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digit_count == 0 {
            return None;
        }
        let (tab_id, remainder) = rest.split_at(digit_count);
        remainder
            .starts_with(FRAME_MARKER)
            .then(|| row.tab_target_wire(tab_id))
    }

    /// Human-readable list of accepted tab target shapes for error details,
    /// e.g. `chrome-tab:<id>` or `chrome-tab:<id>|edge-tab:<id>`.
    #[must_use]
    pub fn tab_target_shapes(&self) -> String {
        self.rows()
            .iter()
            .map(|row| format!("{}<id>", row.tab_target_prefix))
            .collect::<Vec<_>>()
            .join("|")
    }
}

fn split_row_prefix<'r, 'a>(
    registry: &'r BrowserRegistry,
    raw: &'a str,
) -> Option<(&'r BrowserRow, &'a str)> {
    registry.rows().iter().find_map(|row| {
        raw.strip_prefix(row.tab_target_prefix.as_str())
            .map(|rest| (row, rest))
    })
}

fn unknown_prefix_or_none<T>(trimmed: &str) -> Result<Option<T>, BrowserBridgeError> {
    let looks_like_bridge = trimmed
        .split_once(GENERIC_TAB_MARKER)
        .is_some_and(|(stem, _)| BrowserKindId::new(stem).is_ok());
    if looks_like_bridge {
        return Err(BrowserBridgeError::kind_unresolved(format!(
            "bridge target {trimmed:?} uses a <kind>-tab: prefix no declared browser row owns; refusing to guess a browser (source_of_truth=browsers/*.json tab_target_prefix)"
        )));
    }
    Ok(None)
}

fn parse_decimal(raw: &str) -> Option<u64> {
    if raw.is_empty() || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    raw.parse().ok()
}

fn valid_dom_path(path: &str) -> bool {
    !path.is_empty()
        && path.split('.').all(|part| {
            !part.is_empty()
                && (part == SHADOW_HOP_SEGMENT || part.bytes().all(|byte| byte.is_ascii_digit()))
        })
}
