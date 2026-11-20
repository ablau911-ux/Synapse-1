use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Describes how a reported confidence score was obtained.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConfidenceSource {
    Engine,
    Uia,
    Synthetic,
    Heuristic,
    NotApplicable,
    #[default]
    Unsupported,
}

impl ConfidenceSource {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Engine => "engine",
            Self::Uia => "uia",
            Self::Synthetic => "synthetic",
            Self::Heuristic => "heuristic",
            Self::Unsupported => "unsupported",
            Self::NotApplicable => "not_applicable",
        }
    }

    #[must_use]
    pub fn meets_threshold(self, confidence: f32, threshold: f32) -> bool {
        match self {
            Self::Unsupported | Self::NotApplicable => true,
            _ => confidence >= threshold,
        }
    }

    #[must_use]
    pub const fn least_trusted(self, other: Self) -> Self {
        // Variants are declared from most to least trusted.
        if self as u8 >= other as u8 {
            self
        } else {
            other
        }
    }
}
