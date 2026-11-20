use std::{collections::HashMap, env::VarError, str::FromStr};

use crate::error::{LivefeedError, Result};

pub const DEFAULT_FPS: u32 = 5;
pub const DEFAULT_TILE_PX: u32 = 64;
pub const DEFAULT_HISTORY_MS: u64 = 120_000;
pub const DEFAULT_HISTORY_MAX_BYTES: usize = 67_108_864;
pub const DEFAULT_LOOK_LONG_EDGE: u32 = 1280;
pub const DEFAULT_JPEG_QUALITY: u8 = 80;
pub const DEFAULT_MAX_CHANGE_CROPS: usize = 8;
pub const DEFAULT_WAIT_MAX_MS: u64 = 30_000;
pub const DEFAULT_START_TIMEOUT_MS: u64 = 3_000;
pub const DEFAULT_WATCH_DEFAULT_MS: u64 = 3_000;
pub const DEFAULT_WATCH_MAX_FRAMES: usize = 6;
pub const DEFAULT_WATCH_CELL_LONG_EDGE: u32 = 480;
pub const DEFAULT_CUT_RATIO_PERCENT: u64 = 20;

pub const ENV_FPS: &str = "SYNAPSE_LIVEFEED_FPS";
pub const ENV_TILE_PX: &str = "SYNAPSE_LIVEFEED_TILE_PX";
pub const ENV_HISTORY_MS: &str = "SYNAPSE_LIVEFEED_HISTORY_MS";
pub const ENV_HISTORY_MAX_BYTES: &str = "SYNAPSE_LIVEFEED_HISTORY_MAX_BYTES";
pub const ENV_LOOK_LONG_EDGE: &str = "SYNAPSE_LIVEFEED_LOOK_LONG_EDGE";
pub const ENV_JPEG_QUALITY: &str = "SYNAPSE_LIVEFEED_JPEG_QUALITY";
pub const ENV_MAX_CHANGE_CROPS: &str = "SYNAPSE_LIVEFEED_MAX_CHANGE_CROPS";
pub const ENV_WAIT_MAX_MS: &str = "SYNAPSE_LIVEFEED_WAIT_MAX_MS";
pub const ENV_START_TIMEOUT_MS: &str = "SYNAPSE_LIVEFEED_START_TIMEOUT_MS";
pub const ENV_WATCH_DEFAULT_MS: &str = "SYNAPSE_LIVEFEED_WATCH_DEFAULT_MS";
pub const ENV_WATCH_MAX_FRAMES: &str = "SYNAPSE_LIVEFEED_WATCH_MAX_FRAMES";
pub const ENV_WATCH_CELL_LONG_EDGE: &str = "SYNAPSE_LIVEFEED_WATCH_CELL_LONG_EDGE";
pub const ENV_CUT_RATIO_PERCENT: &str = "SYNAPSE_LIVEFEED_CUT_RATIO_PERCENT";

pub const MAX_JPEG_QUALITY: u8 = 100;
pub(crate) const ZERO_U8: u8 = 0;
pub(crate) const ZERO_U32: u32 = 0;
pub(crate) const ZERO_U64: u64 = 0;
pub(crate) const ZERO_USIZE: usize = 0;
pub(crate) const ZERO_I32: i32 = 0;
pub(crate) const ZERO_ISIZE: isize = 0;
pub(crate) const ONE_U32: u32 = 1;
pub(crate) const ONE_U64: u64 = 1;
pub(crate) const ONE_USIZE: usize = 1;
pub(crate) const ONE_F32: f32 = 1.0;
pub(crate) const BYTES_PER_PIXEL: usize = 4;
pub(crate) const BYTES_PER_PIXEL_U8: u8 = 4;
pub(crate) const BLUE_CHANNEL_INDEX: usize = 0;
pub(crate) const RED_CHANNEL_INDEX: usize = 2;
pub(crate) const MS_PER_SECOND: u64 = 1_000;
pub(crate) const POLL_INTERVAL_MS: u64 = 25;
pub(crate) const BLACK_CHANNEL_MAX: u8 = 8;
pub(crate) const BLACK_SAMPLE_STEP_PX: usize = 4;
pub(crate) const DESKTOP_READOBJECTS_ACCESS: u32 = 1;
pub(crate) const MONITORINFOF_PRIMARY: u32 = 1;
pub(crate) const WIN32_TRUE: i32 = 1;
pub(crate) const DPI_AWARENESS_PER_MONITOR_V2: isize = -4;
pub(crate) const WATCH_SEPARATOR_PX: u32 = 2;
pub(crate) const PERCENT_MAX: u64 = 100;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CameraConfig {
    pub fps: u32,
    pub tile_px: u32,
    pub history_ms: u64,
    pub history_max_bytes: usize,
    pub look_long_edge: u32,
    pub jpeg_quality: u8,
    pub max_change_crops: usize,
    pub wait_max_ms: u64,
    pub start_timeout_ms: u64,
    pub watch_default_ms: u64,
    pub watch_max_frames: usize,
    pub watch_cell_long_edge: u32,
    pub cut_ratio_percent: u64,
}

impl Default for CameraConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl CameraConfig {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            fps: DEFAULT_FPS,
            tile_px: DEFAULT_TILE_PX,
            history_ms: DEFAULT_HISTORY_MS,
            history_max_bytes: DEFAULT_HISTORY_MAX_BYTES,
            look_long_edge: DEFAULT_LOOK_LONG_EDGE,
            jpeg_quality: DEFAULT_JPEG_QUALITY,
            max_change_crops: DEFAULT_MAX_CHANGE_CROPS,
            wait_max_ms: DEFAULT_WAIT_MAX_MS,
            start_timeout_ms: DEFAULT_START_TIMEOUT_MS,
            watch_default_ms: DEFAULT_WATCH_DEFAULT_MS,
            watch_max_frames: DEFAULT_WATCH_MAX_FRAMES,
            watch_cell_long_edge: DEFAULT_WATCH_CELL_LONG_EDGE,
            cut_ratio_percent: DEFAULT_CUT_RATIO_PERCENT,
        }
    }

    /// Loads [`CameraConfig`] from process environment variables (`SYNAPSE_LIVEFEED_*`).
    ///
    /// # Errors
    ///
    /// Returns [`LivefeedError::BadConfig`] if any environment variable cannot be
    /// parsed or validates as zero / out of bounds.
    pub fn from_env() -> Result<Self> {
        let mut cfg = Self::default();
        cfg.fps = read_env_var(ENV_FPS, cfg.fps)?;
        cfg.tile_px = read_env_var(ENV_TILE_PX, cfg.tile_px)?;
        cfg.history_ms = read_env_var(ENV_HISTORY_MS, cfg.history_ms)?;
        cfg.history_max_bytes = read_env_var(ENV_HISTORY_MAX_BYTES, cfg.history_max_bytes)?;
        cfg.look_long_edge = read_env_var(ENV_LOOK_LONG_EDGE, cfg.look_long_edge)?;
        cfg.jpeg_quality = read_env_var(ENV_JPEG_QUALITY, cfg.jpeg_quality)?;
        cfg.max_change_crops = read_env_var(ENV_MAX_CHANGE_CROPS, cfg.max_change_crops)?;
        cfg.wait_max_ms = read_env_var(ENV_WAIT_MAX_MS, cfg.wait_max_ms)?;
        cfg.start_timeout_ms = read_env_var(ENV_START_TIMEOUT_MS, cfg.start_timeout_ms)?;
        cfg.watch_default_ms = read_env_var(ENV_WATCH_DEFAULT_MS, cfg.watch_default_ms)?;
        cfg.watch_max_frames = read_env_var(ENV_WATCH_MAX_FRAMES, cfg.watch_max_frames)?;
        cfg.watch_cell_long_edge =
            read_env_var(ENV_WATCH_CELL_LONG_EDGE, cfg.watch_cell_long_edge)?;
        cfg.cut_ratio_percent = read_env_var(ENV_CUT_RATIO_PERCENT, cfg.cut_ratio_percent)?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Loads [`CameraConfig`] using a caller-supplied environment lookup function.
    ///
    /// # Errors
    ///
    /// Returns [`LivefeedError::BadConfig`] if any provided value fails to parse or is zero.
    pub fn from_env_with<F>(get_var: F) -> Result<Self>
    where
        F: Fn(&str) -> Option<String>,
    {
        let mut cfg = Self::default();
        cfg.fps = parse_opt_var(ENV_FPS, get_var(ENV_FPS), cfg.fps)?;
        cfg.tile_px = parse_opt_var(ENV_TILE_PX, get_var(ENV_TILE_PX), cfg.tile_px)?;
        cfg.history_ms = parse_opt_var(ENV_HISTORY_MS, get_var(ENV_HISTORY_MS), cfg.history_ms)?;
        cfg.history_max_bytes = parse_opt_var(
            ENV_HISTORY_MAX_BYTES,
            get_var(ENV_HISTORY_MAX_BYTES),
            cfg.history_max_bytes,
        )?;
        cfg.look_long_edge = parse_opt_var(
            ENV_LOOK_LONG_EDGE,
            get_var(ENV_LOOK_LONG_EDGE),
            cfg.look_long_edge,
        )?;
        cfg.jpeg_quality = parse_opt_var(
            ENV_JPEG_QUALITY,
            get_var(ENV_JPEG_QUALITY),
            cfg.jpeg_quality,
        )?;
        cfg.max_change_crops = parse_opt_var(
            ENV_MAX_CHANGE_CROPS,
            get_var(ENV_MAX_CHANGE_CROPS),
            cfg.max_change_crops,
        )?;
        cfg.wait_max_ms =
            parse_opt_var(ENV_WAIT_MAX_MS, get_var(ENV_WAIT_MAX_MS), cfg.wait_max_ms)?;
        cfg.start_timeout_ms = parse_opt_var(
            ENV_START_TIMEOUT_MS,
            get_var(ENV_START_TIMEOUT_MS),
            cfg.start_timeout_ms,
        )?;
        cfg.watch_default_ms = parse_opt_var(
            ENV_WATCH_DEFAULT_MS,
            get_var(ENV_WATCH_DEFAULT_MS),
            cfg.watch_default_ms,
        )?;
        cfg.watch_max_frames = parse_opt_var(
            ENV_WATCH_MAX_FRAMES,
            get_var(ENV_WATCH_MAX_FRAMES),
            cfg.watch_max_frames,
        )?;
        cfg.watch_cell_long_edge = parse_opt_var(
            ENV_WATCH_CELL_LONG_EDGE,
            get_var(ENV_WATCH_CELL_LONG_EDGE),
            cfg.watch_cell_long_edge,
        )?;
        cfg.cut_ratio_percent = parse_opt_var(
            ENV_CUT_RATIO_PERCENT,
            get_var(ENV_CUT_RATIO_PERCENT),
            cfg.cut_ratio_percent,
        )?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Loads [`CameraConfig`] from an iterator of `(key, value)` environment pairs.
    ///
    /// # Errors
    ///
    /// Returns [`LivefeedError::BadConfig`] if any value fails to parse or is zero.
    pub fn from_vars<I, K, V>(vars: I) -> Result<Self>
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<str>,
        V: AsRef<str>,
    {
        let map: HashMap<String, String> = vars
            .into_iter()
            .map(|(k, v)| (k.as_ref().to_owned(), v.as_ref().to_owned()))
            .collect();
        Self::from_env_with(|key| map.get(key).cloned())
    }

    /// Validates that no configuration field is zero and `jpeg_quality` is within `1..=100`.
    ///
    /// # Errors
    ///
    /// Returns [`LivefeedError::BadConfig`] when any field is zero or out of range.
    pub fn validate(&self) -> Result<()> {
        if self.fps == ZERO_U32 {
            return Err(LivefeedError::bad_config("fps must be greater than zero"));
        }
        if self.tile_px == ZERO_U32 {
            return Err(LivefeedError::bad_config(
                "tile_px must be greater than zero",
            ));
        }
        if self.history_ms == ZERO_U64 {
            return Err(LivefeedError::bad_config(
                "history_ms must be greater than zero",
            ));
        }
        if self.history_max_bytes == ZERO_USIZE {
            return Err(LivefeedError::bad_config(
                "history_max_bytes must be greater than zero",
            ));
        }
        if self.look_long_edge == ZERO_U32 {
            return Err(LivefeedError::bad_config(
                "look_long_edge must be greater than zero",
            ));
        }
        if self.jpeg_quality == ZERO_U8 || self.jpeg_quality > MAX_JPEG_QUALITY {
            return Err(LivefeedError::bad_config("jpeg_quality must be in 1..=100"));
        }
        if self.max_change_crops == ZERO_USIZE {
            return Err(LivefeedError::bad_config(
                "max_change_crops must be greater than zero",
            ));
        }
        if self.wait_max_ms == ZERO_U64 {
            return Err(LivefeedError::bad_config(
                "wait_max_ms must be greater than zero",
            ));
        }
        if self.start_timeout_ms == ZERO_U64 {
            return Err(LivefeedError::bad_config(
                "start_timeout_ms must be greater than zero",
            ));
        }
        if self.watch_default_ms == ZERO_U64 {
            return Err(LivefeedError::bad_config(
                "watch_default_ms must be greater than zero",
            ));
        }
        if self.watch_max_frames == ZERO_USIZE {
            return Err(LivefeedError::bad_config(
                "watch_max_frames must be greater than zero",
            ));
        }
        if self.watch_cell_long_edge == ZERO_U32 {
            return Err(LivefeedError::bad_config(
                "watch_cell_long_edge must be greater than zero",
            ));
        }
        if self.cut_ratio_percent == ZERO_U64 || self.cut_ratio_percent > PERCENT_MAX {
            return Err(LivefeedError::bad_config(
                "cut_ratio_percent must be in 1..=100",
            ));
        }
        Ok(())
    }
}

fn read_env_var<T: FromStr>(name: &str, default: T) -> Result<T> {
    match std::env::var(name) {
        Ok(raw) => parse_raw_var(name, &raw),
        Err(VarError::NotPresent) => Ok(default),
        Err(VarError::NotUnicode(_)) => Err(LivefeedError::bad_config(format!(
            "{name} contains non-unicode characters"
        ))),
    }
}

fn parse_opt_var<T: FromStr>(name: &str, value: Option<String>, default: T) -> Result<T> {
    match value {
        Some(raw) => parse_raw_var(name, &raw),
        None => Ok(default),
    }
}

fn parse_raw_var<T: FromStr>(name: &str, raw: &str) -> Result<T> {
    raw.trim()
        .parse::<T>()
        .map_err(|_err| LivefeedError::bad_config(format!("invalid value for {name}: {raw:?}")))
}
