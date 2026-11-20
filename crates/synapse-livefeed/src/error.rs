use thiserror::Error;

pub const LIVEFEED_NOT_RUNNING: &str = "LIVEFEED_NOT_RUNNING";
pub const LIVEFEED_ALREADY_RUNNING: &str = "LIVEFEED_ALREADY_RUNNING";
pub const LIVEFEED_BAD_CONFIG: &str = "LIVEFEED_BAD_CONFIG";
pub const LIVEFEED_BAD_REGION: &str = "LIVEFEED_BAD_REGION";
pub const LIVEFEED_CAPTURE_FAILED: &str = "LIVEFEED_CAPTURE_FAILED";

#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum LivefeedError {
    #[error("LIVEFEED_NOT_RUNNING: livefeed camera is not running")]
    NotRunning,
    #[error("LIVEFEED_ALREADY_RUNNING: livefeed camera is already running")]
    AlreadyRunning,
    #[error("LIVEFEED_BAD_CONFIG: {detail}")]
    BadConfig { detail: String },
    #[error("LIVEFEED_BAD_REGION: {detail}")]
    BadRegion { detail: String },
    #[error("LIVEFEED_CAPTURE_FAILED: {detail}")]
    CaptureFailed { detail: String },
}

pub type CameraError = LivefeedError;
pub type Result<T> = std::result::Result<T, LivefeedError>;

impl LivefeedError {
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NotRunning => LIVEFEED_NOT_RUNNING,
            Self::AlreadyRunning => LIVEFEED_ALREADY_RUNNING,
            Self::BadConfig { .. } => LIVEFEED_BAD_CONFIG,
            Self::BadRegion { .. } => LIVEFEED_BAD_REGION,
            Self::CaptureFailed { .. } => LIVEFEED_CAPTURE_FAILED,
        }
    }

    #[must_use]
    pub fn bad_config(detail: impl Into<String>) -> Self {
        Self::BadConfig {
            detail: detail.into(),
        }
    }

    #[must_use]
    pub fn bad_region(detail: impl Into<String>) -> Self {
        Self::BadRegion {
            detail: detail.into(),
        }
    }

    #[must_use]
    pub fn capture_failed(detail: impl Into<String>) -> Self {
        Self::CaptureFailed {
            detail: detail.into(),
        }
    }
}

impl From<synapse_capture::CaptureError> for LivefeedError {
    fn from(error: synapse_capture::CaptureError) -> Self {
        Self::CaptureFailed {
            detail: error.to_string(),
        }
    }
}
