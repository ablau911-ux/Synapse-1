#![deny(unsafe_code)]

pub mod camera;
pub mod config;
pub mod error;
pub mod history;
pub mod tiles;

pub use camera::{
    Camera, CameraStatus, ChangeRegion, Changes, Look, ScreenState, Watch, WatchFrame, Zoom,
};
pub use config::{
    CameraConfig, DEFAULT_CUT_RATIO_PERCENT, DEFAULT_FPS, DEFAULT_HISTORY_MAX_BYTES,
    DEFAULT_HISTORY_MS, DEFAULT_JPEG_QUALITY, DEFAULT_LOOK_LONG_EDGE, DEFAULT_MAX_CHANGE_CROPS,
    DEFAULT_START_TIMEOUT_MS, DEFAULT_TILE_PX, DEFAULT_WAIT_MAX_MS, DEFAULT_WATCH_CELL_LONG_EDGE,
    DEFAULT_WATCH_DEFAULT_MS, DEFAULT_WATCH_MAX_FRAMES,
};
pub use error::{
    CameraError, LIVEFEED_ALREADY_RUNNING, LIVEFEED_BAD_CONFIG, LIVEFEED_BAD_REGION,
    LIVEFEED_CAPTURE_FAILED, LIVEFEED_NOT_RUNNING, LivefeedError, Result,
};
pub use history::{ChangeRecord, History};
pub use synapse_core::Rect;
pub use tiles::diff;
