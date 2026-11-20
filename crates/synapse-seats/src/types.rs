use std::{
    fmt,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

pub const MAX_TEXT_BYTES: usize = 65_536;
pub const MAX_EVALUATE_BYTES: usize = 65_536;
pub const MAX_SCREENSHOT_BYTES: usize = 12 * 1024 * 1024;
pub const MAX_VIEWPORT_WIDTH: u32 = 3_840;
pub const MAX_VIEWPORT_HEIGHT: u32 = 2_160;

#[derive(
    Clone, Debug, Deserialize, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize,
)]
#[serde(transparent)]
pub struct OwnerId(String);

impl OwnerId {
    #[must_use]
    pub fn new(trusted_session: impl Into<String>) -> Self {
        Self(trusted_session.into())
    }
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, JsonSchema, PartialEq, Serialize)]
#[serde(transparent)]
pub struct SeatId(#[schemars(with = "String")] pub Uuid);

impl SeatId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}
impl Default for SeatId {
    fn default() -> Self {
        Self::new()
    }
}
impl fmt::Display for SeatId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, JsonSchema, PartialEq, Serialize)]
#[serde(transparent)]
pub struct SeatGeneration(pub u64);

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    Browser,
    #[cfg(any(test, feature = "test-support"))]
    Mock,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "backend", rename_all = "snake_case", deny_unknown_fields)]
pub enum SeatConfig {
    Browser(BrowserSeatConfig),
    #[cfg(any(test, feature = "test-support"))]
    Mock(MockSeatConfig),
}

impl SeatConfig {
    #[must_use]
    pub const fn backend_kind(&self) -> BackendKind {
        match self {
            Self::Browser(_) => BackendKind::Browser,
            #[cfg(any(test, feature = "test-support"))]
            Self::Mock(_) => BackendKind::Mock,
        }
    }
    pub fn validate(&self) -> Result<(), SeatError> {
        match self {
            Self::Browser(config) => config.validate(),
            #[cfg(any(test, feature = "test-support"))]
            Self::Mock(_) => Ok(()),
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MockSeatConfig {}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserSeatConfig {
    #[serde(default)]
    pub initial_url: Option<String>,
    #[serde(default)]
    pub viewport: Viewport,
}
impl BrowserSeatConfig {
    pub fn validate(&self) -> Result<(), SeatError> {
        self.viewport.validate()?;
        if let Some(url) = &self.initial_url {
            validate_url(url)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Viewport {
    #[serde(default = "default_width")]
    pub width: u32,
    #[serde(default = "default_height")]
    pub height: u32,
}
const fn default_width() -> u32 {
    1280
}
const fn default_height() -> u32 {
    720
}
impl Default for Viewport {
    fn default() -> Self {
        Self {
            width: default_width(),
            height: default_height(),
        }
    }
}
impl Viewport {
    pub fn validate(self) -> Result<(), SeatError> {
        if self.width == 0
            || self.height == 0
            || self.width > MAX_VIEWPORT_WIDTH
            || self.height > MAX_VIEWPORT_HEIGHT
        {
            return Err(SeatError::InvalidConfig(format!(
                "viewport must be 1..={MAX_VIEWPORT_WIDTH} by 1..={MAX_VIEWPORT_HEIGHT}"
            )));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum SeatAction {
    Navigate { url: String },
    TypeText { text: String },
    KeyDown { key: String },
    KeyUp { key: String },
    PointerMove { x: f64, y: f64 },
    PointerDown { button: PointerButton },
    PointerUp { button: PointerButton },
    Scroll { delta_x: f64, delta_y: f64 },
    Resize { viewport: Viewport },
    Evaluate { expression: String },
}
impl SeatAction {
    pub fn validate(&self) -> Result<(), SeatError> {
        match self {
            Self::Navigate { url } => validate_url(url),
            Self::TypeText { text } if text.is_empty() || text.len() > MAX_TEXT_BYTES => Err(
                SeatError::InvalidAction(format!("text must be 1..={MAX_TEXT_BYTES} bytes")),
            ),
            Self::KeyDown { key } | Self::KeyUp { key } if key.is_empty() || key.len() > 128 => {
                Err(SeatError::InvalidAction(
                    "key must be 1..=128 bytes".to_owned(),
                ))
            }
            Self::PointerMove { x, y } if !x.is_finite() || !y.is_finite() => Err(
                SeatError::InvalidAction("pointer coordinates must be finite".to_owned()),
            ),
            Self::Scroll { delta_x, delta_y } if !delta_x.is_finite() || !delta_y.is_finite() => {
                Err(SeatError::InvalidAction(
                    "scroll deltas must be finite".to_owned(),
                ))
            }
            Self::Resize { viewport } => viewport.validate(),
            Self::Evaluate { expression }
                if expression.is_empty() || expression.len() > MAX_EVALUATE_BYTES =>
            {
                Err(SeatError::InvalidAction(format!(
                    "expression must be 1..={MAX_EVALUATE_BYTES} bytes"
                )))
            }
            _ => Ok(()),
        }
    }
}

fn validate_url(url: &str) -> Result<(), SeatError> {
    if url.is_empty() || url.len() > 8_192 {
        return Err(SeatError::InvalidAction(
            "URL must be 1..=8192 bytes".to_owned(),
        ));
    }
    if !(url.starts_with("https://") || url.starts_with("http://") || url == "about:blank") {
        return Err(SeatError::InvalidAction(
            "URL scheme must be http, https, or exactly about:blank".to_owned(),
        ));
    }
    Ok(())
}

#[derive(
    Clone, Copy, Debug, Deserialize, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize,
)]
#[serde(rename_all = "snake_case")]
pub enum PointerButton {
    Left,
    Middle,
    Right,
    Back,
    Forward,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SeatState {
    Starting,
    Ready,
    Paused,
    Failed,
    Closed,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BackendIdentity {
    pub kind: BackendKind,
    pub target: String,
    pub process_id: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActionReceipt {
    pub seat_id: SeatId,
    pub generation: SeatGeneration,
    pub sequence: u64,
    pub timestamp_ms: u64,
    pub detail: ActionDetail,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ActionDetail {
    Navigated {
        frame_id: Option<String>,
    },
    TextInserted {
        byte_count: u32,
    },
    Key {
        key: String,
        pressed: bool,
    },
    PointerMoved {
        x: f64,
        y: f64,
    },
    PointerButton {
        button: PointerButton,
        pressed: bool,
    },
    Scrolled {
        delta_x: f64,
        delta_y: f64,
    },
    Resized {
        viewport: Viewport,
    },
    Evaluated {
        result_type: String,
        value_json: String,
        description: Option<String>,
    },
}
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub timestamp_ms: u64,
    pub mime_type: String,
    pub image_base64: String,
}
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureReceipt {
    pub seat_id: SeatId,
    pub generation: SeatGeneration,
    pub sequence: u64,
    pub timestamp_ms: u64,
    pub frame: Frame,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FrameMetadata {
    pub sequence: u64,
    pub width: u32,
    pub height: u32,
    pub timestamp_ms: u64,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SeatView {
    pub id: SeatId,
    pub owner: OwnerId,
    pub generation: SeatGeneration,
    pub backend: BackendIdentity,
    pub state: SeatState,
    pub latest_receipt: Option<ActionReceipt>,
    pub latest_frame: Option<FrameMetadata>,
}

#[derive(Clone, Debug)]
pub struct BrowserRuntimeConfig {
    pub chromium_executable: PathBuf,
    pub runtime_root: PathBuf,
    pub startup_timeout_ms: u64,
}
#[derive(Clone, Debug)]
pub struct SeatSupervisorConfig {
    pub browser: BrowserRuntimeConfig,
    pub max_seats: usize,
    pub per_seat_queue_capacity: usize,
    pub request_timeout_ms: u64,
}
impl SeatSupervisorConfig {
    pub fn validate(&self) -> Result<(), SeatError> {
        if self.max_seats == 0
            || self.per_seat_queue_capacity == 0
            || self.request_timeout_ms == 0
            || self.browser.startup_timeout_ms == 0
        {
            return Err(SeatError::InvalidConfig(
                "seat capacities and timeouts must be non-zero".to_owned(),
            ));
        }
        Ok(())
    }

    /// C5 (#73): total time one seat creation may take, including teardown and
    /// readback of a failed or abandoned startup: startup + request timeout.
    #[must_use]
    pub const fn startup_budget_ms(&self) -> u64 {
        self.browser
            .startup_timeout_ms
            .saturating_add(self.request_timeout_ms)
    }

    /// Part of the request timeout a backend may spend tearing down its own
    /// failed startup (half). It ends before the supervisor's creation cutoff.
    #[must_use]
    pub const fn startup_teardown_allowance(&self) -> Duration {
        Duration::from_millis(self.request_timeout_ms / 2)
    }

    /// Final part of the C5 budget kept for the supervisor's abandoned-startup
    /// readback (a quarter of the request timeout). The quarter between the
    /// backend teardown allowance and this reserve is slack, so the supervisor
    /// cutoff never races an orderly backend teardown.
    #[must_use]
    pub const fn startup_backstop_reserve(&self) -> Duration {
        Duration::from_millis(self.request_timeout_ms / 4)
    }
}

/// Physical readback of a partial backend torn down after a failed or abandoned
/// startup (#73). Only `Confirmed` may be reported as "torn down".
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TeardownStatus {
    /// Child exit and private-state removal were both read back.
    Confirmed,
    /// Teardown did not finish inside its allowance; names what may remain.
    Pending(String),
}

impl fmt::Display for TeardownStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Confirmed => {
                formatter.write_str("teardown confirmed: child exited, private state removed")
            }
            Self::Pending(detail) => write!(formatter, "teardown pending: {detail}"),
        }
    }
}

#[derive(Debug, Error)]
pub enum SeatError {
    #[error("seat configuration is invalid: {0}")]
    InvalidConfig(String),
    #[error("seat action is invalid: {0}")]
    InvalidAction(String),
    #[error("seat {0} does not exist")]
    NotFound(SeatId),
    #[error("seat {0} is not owned by this trusted session")]
    NotOwner(SeatId),
    #[error("seat {id} generation {provided} is stale; current generation is {current}")]
    StaleGeneration {
        id: SeatId,
        provided: u64,
        current: u64,
    },
    #[error("seat {0} is paused")]
    Paused(SeatId),
    #[error("seat {0} is closed")]
    Closed(SeatId),
    #[error("seat {0} failed: {1}")]
    Failed(SeatId, String),
    #[error("seat capacity is exhausted")]
    CapacityExhausted,
    #[error("seat {0} queue is full")]
    QueueFull(SeatId),
    #[error("seat {0} request timed out")]
    Timeout(SeatId),
    /// Backend startup (process launch, CDP connection, initial commands) missed its
    /// deadline. No seat was registered and the capacity slot was returned;
    /// `teardown` is the physical readback of the partial backend (#73).
    #[error("seat backend startup did not complete within {timeout_ms}ms; {teardown}")]
    StartupTimeout {
        timeout_ms: u64,
        teardown: TeardownStatus,
    },
    #[error("backend {0:?} is unavailable")]
    BackendUnavailable(BackendKind),
    #[error("backend error: {0}")]
    Backend(String),
    #[error("capture exceeded {MAX_SCREENSHOT_BYTES} bytes")]
    FrameTooLarge,
    #[error("backend {0:?} is already registered")]
    BackendAlreadyRegistered(BackendKind),
}

impl SeatError {
    #[must_use]
    pub fn backend(detail: impl AsRef<str>) -> Self {
        const MAX_DETAIL_BYTES: usize = 4_096;
        let text = detail.as_ref();
        let bounded = if text.len() <= MAX_DETAIL_BYTES {
            text.to_owned()
        } else {
            format!("{}… [truncated]", &text[..MAX_DETAIL_BYTES])
        };
        Self::Backend(bounded)
    }
}

#[cfg(test)]
#[path = "browser_types_tests.rs"]
mod tests;

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}
