//! Error types: registry load errors and the fail-closed bridge routing error.

use std::fmt;

use synapse_core::error_codes;

/// A `browsers/*.json` table that cannot be loaded. Every variant names the
/// offending file(s) so the operator can fix the data, not guess.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BrowserRegistryError {
    #[error("BROWSER_ROW_INVALID file={file} detail={detail}")]
    RowInvalid { file: String, detail: String },
    #[error("BROWSER_REGISTRY_DUPLICATE field={field} value={value} files={first},{second}")]
    Duplicate {
        field: &'static str,
        value: String,
        first: String,
        second: String,
    },
    #[error("BROWSER_REGISTRY_IO path={path} detail={detail}")]
    Io { path: String, detail: String },
    #[error("BROWSER_REGISTRY_EMPTY origin={origin}")]
    Empty { origin: String },
}

/// A bridge routing refusal with a stable `synapse_core::error_codes` code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserBridgeError {
    pub code: &'static str,
    pub detail: String,
}

impl BrowserBridgeError {
    #[must_use]
    pub fn kind_unresolved(detail: impl Into<String>) -> Self {
        Self {
            code: error_codes::BROWSER_BRIDGE_KIND_UNRESOLVED,
            detail: detail.into(),
        }
    }

    #[must_use]
    pub fn capability_unsupported(kind: &str, capability: &str) -> Self {
        Self {
            code: error_codes::BROWSER_BRIDGE_CAPABILITY_UNSUPPORTED,
            detail: format!(
                "browser_kind={kind} capability={capability} reason=row_declares_capability_unsupported source_of_truth=browsers/{kind}.json unsupported_capabilities"
            ),
        }
    }

    #[must_use]
    pub fn params_invalid(detail: impl Into<String>) -> Self {
        Self {
            code: error_codes::TOOL_PARAMS_INVALID,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for BrowserBridgeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.detail)
    }
}

impl std::error::Error for BrowserBridgeError {}
