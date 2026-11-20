//! Bridge endpoints: `<row.endpoint_base>/<lane>`. The chrome row keeps the
//! legacy `chrome-extension://<id>/chrome.tabs` form so persisted session
//! owner rows stay valid without a migration.

use crate::kind::BrowserKindId;
use crate::registry::BrowserRegistry;
use crate::row::BrowserRow;

/// Extension API lane an endpoint names. The lane wire names are protocol
/// vocabulary shared by every browser (the shared service worker's lanes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BridgeLane {
    Tabs,
    Debugger,
    WebNavigation,
}

impl BridgeLane {
    const ALL: [Self; 3] = [Self::Tabs, Self::Debugger, Self::WebNavigation];

    #[must_use]
    pub const fn wire(self) -> &'static str {
        match self {
            Self::Tabs => "chrome.tabs",
            Self::Debugger => "chrome.debugger",
            Self::WebNavigation => "chrome.webNavigation",
        }
    }

    #[must_use]
    pub fn from_wire(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|lane| lane.wire() == raw)
    }

    /// Tab-control lanes accepted where an owner row must be a bridge tab.
    #[must_use]
    pub const fn is_tab_lane(self) -> bool {
        matches!(self, Self::Tabs | Self::Debugger)
    }
}

/// A parsed bridge endpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BridgeEndpoint {
    kind: BrowserKindId,
    lane: BridgeLane,
}

impl BridgeEndpoint {
    #[must_use]
    pub const fn new(kind: BrowserKindId, lane: BridgeLane) -> Self {
        Self { kind, lane }
    }

    #[must_use]
    pub const fn kind(&self) -> &BrowserKindId {
        &self.kind
    }

    #[must_use]
    pub const fn lane(&self) -> BridgeLane {
        self.lane
    }

    /// Wire form, or `None` when the kind has no row.
    #[must_use]
    pub fn to_wire(&self, registry: &BrowserRegistry) -> Option<String> {
        registry
            .by_id(self.kind.as_str())
            .map(|row| row.endpoint(self.lane))
    }

    /// Exact parse against the declared rows.
    #[must_use]
    pub fn parse(registry: &BrowserRegistry, raw: &str) -> Option<Self> {
        let raw = raw.trim();
        registry.rows().iter().find_map(|row| {
            let lane = raw
                .strip_prefix(row.endpoint_base.as_str())?
                .strip_prefix('/')
                .and_then(BridgeLane::from_wire)?;
            Some(Self::new(row.id.clone(), lane))
        })
    }

    /// Legacy compatibility: before multi-browser support any
    /// `chrome-extension://<any id>/<lane>` endpoint was the chrome bridge.
    /// Such rows resolve to the legacy default kind.
    pub(crate) fn parse_legacy(registry: &BrowserRegistry, raw: &str) -> Option<Self> {
        let legacy: &BrowserRow = registry.legacy_default()?;
        let scheme = format!("{}://", legacy.extension.origin_scheme);
        let rest = raw.trim().strip_prefix(scheme.as_str())?;
        let (extension_id, lane) = rest.split_once('/')?;
        if extension_id.is_empty() {
            return None;
        }
        let lane = BridgeLane::from_wire(lane)?;
        Some(Self::new(legacy.id.clone(), lane))
    }
}
