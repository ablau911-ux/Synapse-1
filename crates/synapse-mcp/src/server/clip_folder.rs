//! The clip folder (docs/clip-recorder-plan.md, version 4.1, "Clip folder"):
//! its file names, `clip.json` with the clip state, `frames.jsonl` lines, the
//! raw frames waiting in `waiting/`, and the active-clip registry.
//!
//! The live part (`capture_clip`), the clip worker and the viewer all read and
//! write a clip only through this module. `clip.json` is always replaced
//! atomically (temporary name, then rename) under an exclusive lock file, so the
//! service and the worker process never lose each other's updates.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use fs2::FileExt;
use serde::{Deserialize, Serialize};

pub(super) const CLIP_SCHEMA_VERSION: u32 = 2;
pub(super) const CLIP_JSON_FILE: &str = "clip.json";
pub(super) const FRAMES_FILE: &str = "frames.jsonl";
pub(super) const WAITING_DIR: &str = "waiting";
pub(super) const VIEWS_DIR: &str = "views";
/// Lists clips whose folder is not under the default clips folder; entries
/// stay after the clip is ready or failed.
const REGISTRY_FILE: &str = "active-clips.json";
const LOCK_SUFFIX: &str = ".lock";
/// Held by the recorder for the life of a recording (`RecorderLock`).
const RECORDER_LOCK_FILE: &str = "recorder.lock";
const TEMP_SUFFIX: &str = ".tmp";
const PART_SUFFIX: &str = ".part";
/// Extension of a waiting frame: B, G, R rows compressed with zstd (A8).
const WAITING_FRAME_EXTENSION: &str = "zst";
/// zstd level of the waiting frames (package 0b full comparison, `ca46079d`).
const WAITING_ZSTD_LEVEL: i32 = -3;
/// A waiting frame keeps B, G and R; the alpha byte is dropped (A8).
pub(super) const WAITING_BYTES_PER_PIXEL: u64 = 3;
/// The observer reads frames back as B, G, R, A.
const BGRA_BYTES_PER_PIXEL: usize = 4;

/// Default parent of clip folders.
pub(super) fn clips_root() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map_or_else(std::env::temp_dir, PathBuf::from)
        .join("synapse")
        .join("clips")
}

/// `segment-NNNN.mp4`.
pub(super) fn segment_file_name(index: u32) -> String {
    format!("segment-{index:04}.mp4")
}

/// `segment-NNNN.mp4.part`: the segment being encoded.
pub(super) fn segment_part_name(index: u32) -> String {
    format!("{}{PART_SUFFIX}", segment_file_name(index))
}

pub(super) fn is_part_file(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension == "part")
}

/// The raw frame of instant k: tightly packed B, G, R rows at the captured
/// size (the size is in its `frames.jsonl` line), compressed whole with zstd
/// (A8). Each file stands alone.
pub(super) fn raw_frame_path(folder: &Path, instant: u64) -> PathBuf {
    folder
        .join(WAITING_DIR)
        .join(format!("frame-{instant:08}.{WAITING_FRAME_EXTENSION}"))
}

/// `true` for a finished waiting frame (not a `.tmp` still being written).
pub(super) fn is_waiting_frame(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension == WAITING_FRAME_EXTENSION)
}

/// A8: writes the waiting frames of one recording through one reused zstd
/// context.
pub(super) struct WaitingFrameWriter {
    compressor: zstd::bulk::Compressor<'static>,
    bgr: Vec<u8>,
    compressed: Vec<u8>,
}

impl WaitingFrameWriter {
    pub(super) fn new() -> Result<Self, String> {
        let compressor = zstd::bulk::Compressor::new(WAITING_ZSTD_LEVEL)
            .map_err(|error| format!("could not open the zstd compressor: {error}"))?;
        Ok(Self {
            compressor,
            bgr: Vec::new(),
            compressed: Vec::new(),
        })
    }

    /// Drops the alpha byte of a BGRA frame (B, G, R stay), compresses the
    /// whole frame with zstd at `WAITING_ZSTD_LEVEL`, and writes it as the
    /// waiting frame of instant k (`.tmp`, then rename). Returns the bytes
    /// written to disk.
    pub(super) fn write(
        &mut self,
        folder: &Path,
        instant: u64,
        bgra: &[u8],
    ) -> Result<u64, String> {
        self.bgr.clear();
        self.bgr.extend(
            bgra.chunks_exact(BGRA_BYTES_PER_PIXEL)
                .flat_map(|pixel| [pixel[0], pixel[1], pixel[2]]),
        );
        self.compressed.clear();
        self.compressed
            .reserve(zstd::zstd_safe::compress_bound(self.bgr.len()));
        self.compressor
            .compress_to_buffer(&self.bgr, &mut self.compressed)
            .map_err(|error| format!("could not compress waiting frame {instant}: {error}"))?;
        let path = raw_frame_path(folder, instant);
        let temp = path.with_extension(format!("{WAITING_FRAME_EXTENSION}{TEMP_SUFFIX}"));
        std::fs::write(&temp, &self.compressed)
            .map_err(|error| format!("could not write {}: {error}", temp.display()))?;
        std::fs::rename(&temp, &path)
            .map_err(|error| format!("could not rename {}: {error}", temp.display()))?;
        Ok(self.compressed.len() as u64)
    }
}

/// Why a waiting frame could not be read back: the loss cause its instants
/// get, and the detail for the log.
pub(super) struct WaitingReadError {
    pub(super) cause: LossCause,
    pub(super) message: String,
}

/// Reads waiting frames back through one reused zstd context.
pub(super) struct WaitingFrameReader {
    decompressor: zstd::bulk::Decompressor<'static>,
}

impl WaitingFrameReader {
    pub(super) fn new() -> Result<Self, String> {
        let decompressor = zstd::bulk::Decompressor::new()
            .map_err(|error| format!("could not open the zstd decompressor: {error}"))?;
        Ok(Self { decompressor })
    }

    /// A waiting frame as B, G, R rows of `width` × `height`. A missing file
    /// is `waiting_missing`; a short or damaged file, or one of another size,
    /// is `waiting_unreadable`.
    pub(super) fn read(
        &mut self,
        path: &Path,
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>, WaitingReadError> {
        let bytes = std::fs::read(path).map_err(|error| WaitingReadError {
            cause: if error.kind() == std::io::ErrorKind::NotFound {
                LossCause::WaitingMissing
            } else {
                LossCause::WaitingUnreadable
            },
            message: format!("could not read {}: {error}", path.display()),
        })?;
        self.decode(path, &bytes, width, height)
            .map_err(|message| WaitingReadError {
                cause: LossCause::WaitingUnreadable,
                message,
            })
    }

    fn decode(
        &mut self,
        path: &Path,
        bytes: &[u8],
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>, String> {
        let expected =
            usize::try_from(u64::from(width) * u64::from(height) * WAITING_BYTES_PER_PIXEL)
                .map_err(|_| format!("{} is too large for this machine", path.display()))?;
        // The content size in the zstd frame header is checked first, so a
        // damaged header never sizes an allocation.
        let stored = zstd::zstd_safe::get_frame_content_size(bytes)
            .ok()
            .flatten()
            .and_then(|size| usize::try_from(size).ok());
        if stored != Some(expected) {
            return Err(format!(
                "{} holds {stored:?} bytes, not the {expected} of a {width}x{height} frame",
                path.display()
            ));
        }
        let bgr = self
            .decompressor
            .decompress(bytes, expected)
            .map_err(|error| format!("could not decompress {}: {error}", path.display()))?;
        if bgr.len() != expected {
            return Err(format!(
                "{} decompressed to {} bytes, not {expected}",
                path.display(),
                bgr.len()
            ));
        }
        Ok(bgr)
    }
}

/// How far the clip got (C10).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ClipState {
    Recording,
    Encoding,
    Ready,
    Failed,
}

impl ClipState {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Recording => "recording",
            Self::Encoding => "encoding",
            Self::Ready => "ready",
            Self::Failed => "failed",
        }
    }
}

/// Why the recording ended (C14). Committed segments always stay.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum StopReason {
    Completed,
    StopCall,
    TargetLost,
    PrivacyPaused,
    PrivacyExcludedApp,
    DiskPressure,
    WriteFailed,
    CaptureFailed,
    ServiceRestart,
}

impl StopReason {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::StopCall => "stop_call",
            Self::TargetLost => "target_lost",
            Self::PrivacyPaused => "privacy_paused",
            Self::PrivacyExcludedApp => "privacy_excluded_app",
            Self::DiskPressure => "disk_pressure",
            Self::WriteFailed => "write_failed",
            Self::CaptureFailed => "capture_failed",
            Self::ServiceRestart => "service_restart",
        }
    }
}

/// Why a frame for an instant is missing from the clip (E1, condition C).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum LossCause {
    SlotOverflow,
    PrivacyDiscard,
    Disk,
    LoadShedding,
    ServiceRestart,
    /// The clip worker could not read the waiting frame back (damaged or
    /// short file); the picture before it was kept in the segment.
    WaitingUnreadable,
    /// The waiting frame of a new picture was not on disk when the clip
    /// worker encoded its segment; the picture before it was kept.
    WaitingMissing,
}

/// One line of `frames.jsonl`: one per instant s_k.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct FrameLine {
    pub(super) instant: u64,
    /// k / f in milliseconds from s_0.
    pub(super) t_ms: u64,
    /// `true` when a new frame was written to `waiting/` for this instant.
    pub(super) new: bool,
    /// Captured size of the frame shown at this instant.
    pub(super) width: u32,
    pub(super) height: u32,
    /// Cursor position in frame pixels, logged even though it is not drawn (E9).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) cursor: Option<[i32; 2]>,
    /// Composition time of the frame shown (100 ns, QPC clock).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) composed_hns: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) lost: Option<LossCause>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) minimized: bool,
}

/// Reads every complete line of `frames.jsonl` (a torn last line is skipped).
pub(super) fn read_frame_lines(folder: &Path) -> std::io::Result<Vec<FrameLine>> {
    let text = match std::fs::read_to_string(folder.join(FRAMES_FILE)) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    Ok(text
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect())
}

/// The clock of a clip: s_0 is the first delivered frame.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct ClipClock {
    pub(super) monotonic_ms_at_s0: u64,
    pub(super) wall_clock_unix_ms_at_s0: u64,
    pub(super) composed_hns_at_s0: i64,
}

/// What is recorded.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct ClipTargetInfo {
    pub(super) kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) hwnd: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) monitor_index: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) process_id: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) exe: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) title: Option<String>,
}

/// Window size, σ and recorded size (A3, condition G).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub(super) struct ClipGeometry {
    pub(super) window_width: u32,
    pub(super) window_height: u32,
    pub(super) scale: u32,
    pub(super) recorded_width: u32,
    pub(super) recorded_height: u32,
}

/// Session settings Windows granted (O1).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub(super) struct ObserverGrant {
    pub(super) cursor_in_picture: bool,
    pub(super) border_hidden: bool,
    pub(super) secondary_windows: bool,
    pub(super) requested_interval_ms: Option<u64>,
    pub(super) dirty_regions: bool,
}

/// A committed segment (B8).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct SegmentCheckpoint {
    pub(super) index: u32,
    pub(super) file: String,
    pub(super) bytes: u64,
    pub(super) sha256: String,
    pub(super) first_instant: u64,
    pub(super) last_instant: u64,
    /// Segment-local time 0 is `start_ms` on the clip timeline.
    pub(super) start_ms: u64,
    pub(super) end_ms: u64,
    pub(super) width: u32,
    pub(super) height: u32,
    /// `true` for a transition segment formed during a drag (E4).
    #[serde(default)]
    pub(super) padded: bool,
    /// `false` when the recorded size had to stay within the encoder bound
    /// below σ · window size (B3).
    #[serde(default = "bound_held_default")]
    pub(super) bound_held: bool,
    pub(super) keyframe_instants: Vec<u64>,
    pub(super) encoder: String,
    pub(super) hardware: bool,
    pub(super) quality: QualityReport,
}

const fn bound_held_default() -> bool {
    true
}

/// The rate control the encoder accepted and read back (B7, condition Q).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct QualityReport {
    /// `quality` or `bitrate`.
    pub(super) mode: String,
    /// Quality level q, or bits per second.
    pub(super) value: u32,
    pub(super) gop_frames: u32,
    pub(super) b_frames: u32,
    pub(super) screen_content_hint: bool,
    /// `held`, `unverified` or `not_held` (C6).
    pub(super) status: String,
}

/// One condition: its status and the instants where it failed.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct ConditionStatus {
    pub(super) status: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) failed_instants: Vec<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) detail: Option<serde_json::Value>,
}

/// The `conditions` block of `clip.json` (plan table C, G, Q, B, T).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct Conditions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) c: Option<ConditionStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) g: Option<ConditionStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) q: Option<ConditionStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) b: Option<ConditionStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) t: Option<ConditionStatus>,
}

/// One part's figures for condition B.
pub(super) enum BReport {
    /// The live part, once when the recording stops (A9, A10).
    Live {
        threads: u32,
        max_live_share_percent: f64,
        max_worker_cap_given_percent: f64,
        live_seconds_at_limit: u64,
    },
    /// The host, after every clip worker run.
    Worker {
        max_worker_share_percent: f64,
        max_sum_percent: f64,
        seconds_over_limit: u64,
        worker_seconds: f64,
        limit_percent: f64,
    },
}

/// The verdict of condition B; `clip.json` keeps it as its status string.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BStatus {
    Held,
    NotHeld,
    /// The live part reported; the clip worker has not yet.
    PendingWorker,
    /// The clip worker reported without a live figure (the service ended
    /// during the recording).
    Unverified,
}

impl BStatus {
    fn parse(status: &str) -> Option<Self> {
        match status {
            "held" => Some(Self::Held),
            "not_held" => Some(Self::NotHeld),
            "pending_worker" => Some(Self::PendingWorker),
            "unverified" => Some(Self::Unverified),
            _ => None,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Held => "held",
            Self::NotHeld => "not_held",
            Self::PendingWorker => "pending_worker",
            Self::Unverified => "unverified",
        }
    }
}

impl Conditions {
    /// The one merge of condition B. Two parts measure it at different times:
    /// the live part when the recording stops, and the host after every clip
    /// worker run (possibly before the recording stops). Each report adds its
    /// figures to the detail; a `not_held` verdict and the other part's figures
    /// are never erased. The worker's figures cover all of its runs: the
    /// maxima and the worker time over every run, the seconds over the limit
    /// summed.
    pub(super) fn merge_b(&mut self, report: BReport) {
        let stored = self.b.take();
        let stored_status = stored
            .as_ref()
            .and_then(|condition| BStatus::parse(&condition.status));
        let mut detail = stored
            .and_then(|condition| condition.detail)
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default();
        let live_reported = detail.contains_key("max_live_share_percent");
        let worker_reported = detail.contains_key("seconds_over_limit");
        let was_not_held = stored_status == Some(BStatus::NotHeld);
        let status = match report {
            BReport::Live {
                threads,
                max_live_share_percent,
                max_worker_cap_given_percent,
                live_seconds_at_limit,
            } => {
                detail.insert("threads".to_owned(), serde_json::json!(threads));
                detail.insert(
                    "max_live_share_percent".to_owned(),
                    serde_json::json!(max_live_share_percent),
                );
                detail.insert(
                    "max_worker_cap_given_percent".to_owned(),
                    serde_json::json!(max_worker_cap_given_percent),
                );
                detail.insert(
                    "live_seconds_at_limit".to_owned(),
                    serde_json::json!(live_seconds_at_limit),
                );
                if was_not_held || live_seconds_at_limit > 0 {
                    BStatus::NotHeld
                } else if worker_reported {
                    stored_status.unwrap_or(BStatus::PendingWorker)
                } else {
                    BStatus::PendingWorker
                }
            }
            BReport::Worker {
                max_worker_share_percent,
                max_sum_percent,
                seconds_over_limit,
                worker_seconds,
                limit_percent,
            } => {
                let previous = |key: &str| {
                    detail
                        .get(key)
                        .and_then(serde_json::Value::as_f64)
                        .unwrap_or(0.0)
                };
                let over = detail
                    .get("seconds_over_limit")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0)
                    + seconds_over_limit;
                let worker_share =
                    max_worker_share_percent.max(previous("max_worker_share_percent"));
                let max_sum = max_sum_percent.max(previous("max_sum_percent"));
                let seconds = worker_seconds + previous("worker_seconds");
                detail.insert(
                    "max_worker_share_percent".to_owned(),
                    serde_json::json!(worker_share),
                );
                detail.insert("max_sum_percent".to_owned(), serde_json::json!(max_sum));
                detail.insert("seconds_over_limit".to_owned(), serde_json::json!(over));
                detail.insert("worker_seconds".to_owned(), serde_json::json!(seconds));
                detail.insert("limit_percent".to_owned(), serde_json::json!(limit_percent));
                if was_not_held || over > 0 {
                    BStatus::NotHeld
                } else if live_reported {
                    BStatus::Held
                } else {
                    // No live figure: the sum cannot be shown to stay within
                    // the limit.
                    BStatus::Unverified
                }
            }
        };
        self.b = Some(ConditionStatus {
            status: status.as_str().to_owned(),
            failed_instants: Vec::new(),
            detail: Some(serde_json::Value::Object(detail)),
        });
    }
}

/// A lost instant with its cause.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(super) struct LostInstant {
    pub(super) instant: u64,
    pub(super) cause: LossCause,
}

/// `clip.json`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct ClipJson {
    pub(super) schema_version: u32,
    pub(super) clip_id: String,
    pub(super) synapse_version: String,
    pub(super) state: ClipState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) failure: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) stop_reason: Option<StopReason>,
    pub(super) fps: u32,
    /// Planned end in ms from s_0; `record_extend` moves it.
    pub(super) end_ms: u64,
    /// Instants handled so far, as milliseconds (k / f).
    pub(super) recorded_ms: u64,
    pub(super) target: ClipTargetInfo,
    pub(super) clock: ClipClock,
    pub(super) geometry: ClipGeometry,
    pub(super) observer: ObserverGrant,
    #[serde(default)]
    pub(super) lost: Vec<LostInstant>,
    #[serde(default)]
    pub(super) segments: Vec<SegmentCheckpoint>,
    #[serde(default)]
    pub(super) conditions: Conditions,
}

impl ClipJson {
    /// Reads `clip.json` of a clip folder.
    pub(super) fn read(folder: &Path) -> Result<Self, String> {
        let path = folder.join(CLIP_JSON_FILE);
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("could not read {}: {error}", path.display()))?;
        let clip: Self = serde_json::from_str(&text)
            .map_err(|error| format!("{} is not valid: {error}", path.display()))?;
        if clip.schema_version != CLIP_SCHEMA_VERSION {
            return Err(format!(
                "{} schema_version {} is not supported (expected {CLIP_SCHEMA_VERSION})",
                path.display(),
                clip.schema_version
            ));
        }
        Ok(clip)
    }

    /// Writes `clip.json` for a new clip (temporary name, then rename).
    pub(super) fn create(&self, folder: &Path) -> Result<(), String> {
        let _lock = FolderLock::acquire(folder)?;
        write_atomically(&folder.join(CLIP_JSON_FILE), self)
    }

    /// Reads, changes and replaces `clip.json` under the folder lock.
    pub(super) fn update<T>(
        folder: &Path,
        change: impl FnOnce(&mut Self) -> T,
    ) -> Result<(Self, T), String> {
        let _lock = FolderLock::acquire(folder)?;
        let mut clip = Self::read(folder)?;
        let result = change(&mut clip);
        write_atomically(&folder.join(CLIP_JSON_FILE), &clip)?;
        Ok((clip, result))
    }

    /// Adds lost instants (an instant already listed keeps its first cause)
    /// and carries the whole list into conditions C and T where they exist:
    /// their failed instants, and `not_held` once anything is lost. Both the
    /// recorder and the clip worker name losses, so this is the one place
    /// that merges them.
    pub(super) fn add_lost(&mut self, more: impl IntoIterator<Item = LostInstant>) {
        for lost in more {
            if !self.lost.iter().any(|known| known.instant == lost.instant) {
                self.lost.push(lost);
            }
        }
        self.lost.sort_by_key(|lost| lost.instant);
        if self.lost.is_empty() {
            return;
        }
        let failed: Vec<u64> = self.lost.iter().map(|lost| lost.instant).collect();
        for condition in [self.conditions.c.as_mut(), self.conditions.t.as_mut()]
            .into_iter()
            .flatten()
        {
            condition.status = "not_held".to_owned();
            condition.failed_instants.clone_from(&failed);
            if let Some(serde_json::Value::Object(detail)) = condition.detail.as_mut()
                && detail.contains_key("instants_lost")
            {
                detail.insert("instants_lost".to_owned(), serde_json::json!(failed.len()));
            }
        }
    }
}

/// An exclusive lock on `<file>.lock`, held across one read-modify-write.
struct FolderLock {
    file: std::fs::File,
}

impl FolderLock {
    fn acquire(folder: &Path) -> Result<Self, String> {
        let path = folder.join(format!("{CLIP_JSON_FILE}{LOCK_SUFFIX}"));
        Self::on(&path)
    }

    fn on(path: &Path) -> Result<Self, String> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
            .map_err(|error| format!("could not open {}: {error}", path.display()))?;
        file.lock_exclusive()
            .map_err(|error| format!("could not lock {}: {error}", path.display()))?;
        Ok(Self { file })
    }
}

impl Drop for FolderLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

/// Serialises `value`, writes it to `<path>.tmp`, flushes it to disk and
/// renames it over `path`.
pub(super) fn write_atomically(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("could not serialise {}: {error}", path.display()))?;
    let temp = path.with_extension(format!(
        "{}{TEMP_SUFFIX}",
        path.extension()
            .map_or_else(String::new, |extension| extension
                .to_string_lossy()
                .into_owned())
    ));
    let mut file = std::fs::File::create(&temp)
        .map_err(|error| format!("could not create {}: {error}", temp.display()))?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("could not write {}: {error}", temp.display()))?;
    drop(file);
    std::fs::rename(&temp, path)
        .map_err(|error| format!("could not replace {}: {error}", path.display()))
}

/// The recorder's exclusive `recorder.lock` in the clip folder, held for the
/// life of a recording (the pattern of the clip worker's `worker.lock`).
/// Windows releases it when the recorder's process ends, also by a crash, so
/// recovery (B10) in any service process can tell a live clip from one whose
/// service ended.
pub(super) struct RecorderLock {
    file: std::fs::File,
}

impl RecorderLock {
    /// Takes the lock of a new clip folder.
    pub(super) fn acquire(folder: &Path) -> Result<Self, String> {
        let path = folder.join(RECORDER_LOCK_FILE);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(|error| format!("could not open {}: {error}", path.display()))?;
        file.try_lock_exclusive()
            .map_err(|error| format!("could not lock {}: {error}", path.display()))?;
        Ok(Self { file })
    }

    /// `true` while a recorder holds the lock of this clip folder. Without
    /// the file (a clip from before the lock) no recorder is alive; when the
    /// file cannot be opened or locked, the recorder counts as alive, so a
    /// live clip is never taken for a dead one.
    pub(super) fn held(folder: &Path) -> bool {
        let file = match std::fs::OpenOptions::new()
            .write(true)
            .open(folder.join(RECORDER_LOCK_FILE))
        {
            Ok(file) => file,
            Err(error) => return error.kind() != std::io::ErrorKind::NotFound,
        };
        if file.try_lock_exclusive().is_err() {
            return true;
        }
        let _ = FileExt::unlock(&file);
        false
    }
}

impl Drop for RecorderLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

/// Size and SHA-256 (lowercase hex) of a file.
pub(super) fn file_size_and_sha256(path: &Path) -> std::io::Result<(u64, String)> {
    use sha2::Digest as _;
    use std::io::Read as _;

    let mut file = std::fs::File::open(path)?;
    let mut hasher = sha2::Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    let mut bytes = 0_u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        bytes += read as u64;
        hasher.update(&buffer[..read]);
    }
    let digest = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok((bytes, digest))
}

/// One clip in the active-clip registry.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct RegistryEntry {
    pub(super) clip_id: String,
    pub(super) folder: PathBuf,
}

/// The active-clip registry under the default clips folder (A4, B10). An
/// entry stays after the clip is ready or failed, so `record_status` and
/// `record_stop` still find a clip in another folder by its id; recovery
/// skips clips in a terminal state.
pub(super) struct ClipRegistry;

impl ClipRegistry {
    fn path() -> PathBuf {
        clips_root().join(REGISTRY_FILE)
    }

    /// Every registered clip folder outside the default folder.
    pub(super) fn entries() -> Vec<RegistryEntry> {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Lists a clip whose folder is not under the default clips folder.
    pub(super) fn add(clip_id: &str, folder: &Path) -> Result<(), String> {
        if folder.parent() == Some(clips_root().as_path()) {
            return Ok(());
        }
        Self::change(|entries| {
            entries.retain(|entry| entry.clip_id != clip_id);
            entries.push(RegistryEntry {
                clip_id: clip_id.to_owned(),
                folder: folder.to_path_buf(),
            });
        })
    }

    fn change(edit: impl FnOnce(&mut Vec<RegistryEntry>)) -> Result<(), String> {
        let root = clips_root();
        std::fs::create_dir_all(&root)
            .map_err(|error| format!("could not create {}: {error}", root.display()))?;
        let path = Self::path();
        let _lock = FolderLock::on(&path.with_extension(format!("json{LOCK_SUFFIX}")))?;
        let mut entries = Self::entries();
        edit(&mut entries);
        write_atomically(&path, &entries)
    }

    /// The folder of a clip id: the registry, then the default folder. A
    /// registered folder that no longer holds a `clip.json` is skipped.
    pub(super) fn folder_of(clip_id: &str) -> Option<PathBuf> {
        Self::entries()
            .into_iter()
            .find(|entry| entry.clip_id == clip_id && entry.folder.join(CLIP_JSON_FILE).is_file())
            .map(|entry| entry.folder)
            .or_else(|| {
                let folder = clips_root().join(clip_id);
                folder.join(CLIP_JSON_FILE).is_file().then_some(folder)
            })
    }

    /// Every clip folder that may need work: the registry plus every folder
    /// under the default clips folder.
    pub(super) fn all_folders() -> Vec<PathBuf> {
        let mut folders: Vec<PathBuf> = Self::entries()
            .into_iter()
            .map(|entry| entry.folder)
            .collect();
        if let Ok(read) = std::fs::read_dir(clips_root()) {
            folders.extend(
                read.filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .filter(|path| path.join(CLIP_JSON_FILE).is_file()),
            );
        }
        folders.sort();
        folders.dedup();
        folders
    }
}
