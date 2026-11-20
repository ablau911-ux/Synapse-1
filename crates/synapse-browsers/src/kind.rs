//! Browser identity. A kind is data (`browsers/<id>.json`), never an enum of
//! browser names; only the engine family is a code-level split.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Kind that persisted rows written before multi-browser support belong to.
/// This is a compatibility anchor (serde default for old rows and the route
/// used when no window/target names a browser), not browser vocabulary: the
/// row itself still lives in `browsers/chrome.json`.
pub const LEGACY_DEFAULT_KIND: &str = "chrome";

/// Validated browser kind id: `[a-z0-9_]+`, the row's file stem.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct BrowserKindId(String);

impl BrowserKindId {
    /// # Errors
    /// Returns the rejection reason when `raw` is empty or not `[a-z0-9_]+`.
    pub fn new(raw: &str) -> Result<Self, String> {
        if raw.is_empty()
            || !raw
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(format!("browser kind id {raw:?} must match [a-z0-9_]+"));
        }
        Ok(Self(raw.to_owned()))
    }

    #[must_use]
    pub fn legacy_default() -> Self {
        Self(LEGACY_DEFAULT_KIND.to_owned())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn is_legacy_default(&self) -> bool {
        self.0 == LEGACY_DEFAULT_KIND
    }
}

impl TryFrom<String> for BrowserKindId {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(&value)
    }
}

impl From<BrowserKindId> for String {
    fn from(value: BrowserKindId) -> Self {
        value.0
    }
}

impl fmt::Display for BrowserKindId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Engine family. Selects the behaviour objects (profile inspector, origin
/// rule, extension adapter); the only code-level browser split.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserFamily {
    Chromium,
    Gecko,
}

impl BrowserFamily {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Chromium => "chromium",
            Self::Gecko => "gecko",
        }
    }
}
