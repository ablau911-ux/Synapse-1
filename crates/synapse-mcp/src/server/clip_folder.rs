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
/// Lists active clips whose folder is not under the default clips folder.
const REGISTRY_FILE: &str = "active-clips.json";
const LOCK_SUFFIX: &str = ".lock";
const TEMP_SUFFIX: &str = ".tmp";
const PART_SUFFIX: &str = ".part";

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

/// The raw frame of instant k: tightly packed BGRA rows at the captured size
/// (the size is in its `frames.jsonl` line).
pub(super) fn raw_frame_path(folder: &Path, instant: u64) -> PathBuf {
    folder
        .join(WAITING_DIR)
        .join(format!("frame-{instant:08}.bgra"))
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
    pub(super) keyframe_instants: Vec<u64>,
    pub(super) encoder: String,
    pub(super) hardware: bool,
    pub(super) quality: QualityReport,
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

/// The active-clip registry under the default clips folder (A4, B10).
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

    /// Drops a clip once it is ready or failed.
    pub(super) fn remove(clip_id: &str) -> Result<(), String> {
        Self::change(|entries| entries.retain(|entry| entry.clip_id != clip_id))
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

    /// The folder of a clip id: the registry, then the default folder.
    pub(super) fn folder_of(clip_id: &str) -> Option<PathBuf> {
        Self::entries()
            .into_iter()
            .find(|entry| entry.clip_id == clip_id)
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
