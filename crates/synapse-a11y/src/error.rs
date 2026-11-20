use synapse_core::error_codes;
use thiserror::Error;

pub type A11yResult<T> = Result<T, A11yError>;

#[derive(Debug, Error)]
pub enum A11yError {
    /// A semantic click entered the provider. `delivered` means it returned
    /// successfully before a separate readback failed; otherwise delivery is unknown.
    #[error("{source}")]
    ClickDispatch {
        source: Box<A11yError>,
        delivered: bool,
    },
    /// #189: the click producer proved that no dispatch call was made.
    #[error("{source}")]
    ClickNotDispatched { source: Box<A11yError> },
    #[error("Windows UI Automation is not available: {detail}")]
    NotAvailable { detail: String },
    #[error("no foreground window is available: {detail}")]
    NoForeground { detail: String },
    #[error("target window not found: {detail}")]
    WindowNotFound { detail: String },
    #[error("UI Automation element is stale: {detail}")]
    ElementStale { detail: String },
    #[error("UI Automation element has no supported click control pattern: {detail}")]
    ElementPatternUnsupported { detail: String },
    #[error("UI Automation element does not support ValuePattern: {detail}")]
    ElementValueUnsupported { detail: String },
    #[error("UI Automation element ValuePattern is read-only: {detail}")]
    ElementValueReadOnly { detail: String },
    #[error("UI Automation element is not enabled: {detail}")]
    ElementNotEnabled { detail: String },
    #[error("Chromium DevTools Protocol is unreachable: {detail}")]
    CdpUnreachable { detail: String },
    #[error("Chromium DevTools Protocol attach failed: {detail}")]
    CdpAttachFailed { detail: String },
    #[error("Chromium accessibility tree retrieval failed: {detail}")]
    CdpAxtreeFailed { detail: String },
    #[error("browser wait timed out: {detail}")]
    BrowserWaitTimeout { detail: String },
    #[error("CDP mouse stroke exceeded its time budget: {detail}")]
    CdpStrokeTimeout { detail: String },
    #[error("UI Automation worker job timed out: {detail}")]
    UiaWorkerTimeout {
        phase: UiaWorkerPhase,
        detail: String,
    },
    /// #189: the job was sent to the worker, then its reply channel disconnected.
    #[error("accessibility backend failed: {detail}")]
    UiaWorkerReplyLost { detail: String },
    #[error("foreground activation refused: {detail}")]
    ForegroundActivationRefused { detail: String },
    /// #2348: `act select` on a native element refused before any pattern
    /// call: no options, no option with that name, or two of them.
    #[error("option not selected: {detail}")]
    OptionRefused { detail: String },
    #[error("invalid element id: {detail}")]
    InvalidElementId { detail: String },
    #[error("accessibility backend failed: {detail}")]
    Internal { detail: String },
}

/// #189: where a bounded UIA worker job stopped, relative to the job send.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiaWorkerPhase {
    /// Admission refused: too many live workers; the job was never sent.
    WorkerLimit,
    /// Queue timeout: the worker stayed busy past this request's queue-wait
    /// deadline; never sent, and the worker is not retired (#260).
    WorkerBusy,
    /// Admission refused: the worker was retired after an earlier timeout; never sent.
    WorkerUnavailable,
    /// The job was sent and its reply did not arrive before the deadline.
    ReplyRecv,
}

impl UiaWorkerPhase {
    /// True when the job reached the worker, so its effect is unknown.
    #[must_use]
    pub const fn job_sent(self) -> bool {
        matches!(self, Self::ReplyRecv)
    }
}

impl A11yError {
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::ClickDispatch { source, .. } | Self::ClickNotDispatched { source } => {
                source.code()
            }
            Self::NotAvailable { .. } => error_codes::A11Y_NOT_AVAILABLE,
            Self::NoForeground { .. } => error_codes::A11Y_NO_FOREGROUND,
            Self::WindowNotFound { .. } => error_codes::TARGET_WINDOW_NOT_FOUND,
            Self::ElementStale { .. } => error_codes::A11Y_ELEMENT_STALE,
            Self::ElementPatternUnsupported { .. } => {
                error_codes::ACTION_ELEMENT_PATTERN_UNSUPPORTED
            }
            Self::ElementValueUnsupported { .. } => error_codes::ACTION_ELEMENT_PATTERN_UNSUPPORTED,
            Self::ElementValueReadOnly { .. }
            | Self::ElementNotEnabled { .. }
            | Self::OptionRefused { .. } => error_codes::ACTION_TARGET_INVALID,
            Self::CdpUnreachable { .. } => error_codes::A11Y_CDP_UNREACHABLE,
            Self::CdpAttachFailed { .. } => error_codes::A11Y_CDP_ATTACH_FAILED,
            Self::CdpAxtreeFailed { .. } => error_codes::A11Y_CDP_AXTREE_FAILED,
            Self::BrowserWaitTimeout { .. } => error_codes::BROWSER_WAIT_TIMEOUT,
            Self::CdpStrokeTimeout { .. } => error_codes::A11Y_CDP_STROKE_TIMEOUT,
            Self::UiaWorkerTimeout { .. } => error_codes::A11Y_UIA_WORKER_TIMEOUT,
            Self::ForegroundActivationRefused { .. } => error_codes::FOREGROUND_ACTIVATION_REFUSED,
            Self::InvalidElementId { .. }
            | Self::UiaWorkerReplyLost { .. }
            | Self::Internal { .. } => error_codes::OBSERVE_INTERNAL,
        }
    }

    #[must_use]
    pub fn not_available(detail: impl Into<String>) -> Self {
        Self::NotAvailable {
            detail: detail.into(),
        }
    }

    #[must_use]
    pub fn internal(detail: impl Into<String>) -> Self {
        Self::Internal {
            detail: detail.into(),
        }
    }

    #[must_use]
    pub fn foreground_activation_refused(detail: impl Into<String>) -> Self {
        Self::ForegroundActivationRefused {
            detail: detail.into(),
        }
    }

    #[must_use]
    pub fn uia_worker_timeout(phase: UiaWorkerPhase, detail: impl Into<String>) -> Self {
        Self::UiaWorkerTimeout {
            phase,
            detail: detail.into(),
        }
    }
}
