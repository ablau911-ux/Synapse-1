//! DYN1 clip metadata, atomic publication, and the active recording registry.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use fs2::FileExt;
use serde::{Deserialize, Serialize};

pub(super) const CLIP_SCHEMA_VERSION: u32 = 3;
pub(super) const CLIP_JSON_FILE: &str = "clip.json";
pub(super) const FRAMES_FILE: &str = "frames.jsonl";
pub(super) const VIEWS_DIR: &str = "views";
/// Lists clips whose folder is not under the default clips folder; entries
/// stay after the clip is ready or failed.
const REGISTRY_FILE: &str = "active-clips.json";
const LOCK_SUFFIX: &str = ".lock";
/// Held by the recorder for the life of a recording (`RecorderLock`).
const RECORDER_LOCK_FILE: &str = "recorder.lock";
const TEMP_SUFFIX: &str = ".tmp";
/// A rename of `clip.json`, a segment `.part` file or a waiting frame, and
/// a read of `clip.json`, is tried this many times in all while another
/// program holds the file for a moment.
const BRIEF_HOLD_ATTEMPTS: u32 = 5;
/// The sleep between two such attempts.
const BRIEF_HOLD_RETRY_SLEEP_MS: u64 = 100;
/// Windows errors of a file another program holds for a moment: access
/// denied (5), sharing violation (32), lock violation (33).
const BRIEF_HOLD_OS_ERRORS: [i32; 3] = [5, 32, 33];
/// Default parent of clip folders.
pub(super) fn clips_root() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map_or_else(std::env::temp_dir, PathBuf::from)
        .join("synapse")
        .join("clips")
}

/// How far the clip got (C10).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ClipState {
    Recording,
    Ready,
    Failed,
}

impl ClipState {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Recording => "recording",
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
    Disk,
    LoadShedding,
    ServiceRestart,
    MovingBehind,
    MovingFailed,
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
    #[serde(default, rename = "x", skip_serializing_if = "Option::is_none")]
    pub(super) exact: Option<super::clip_exact::ExactRef>,
    #[serde(default, rename = "mv", skip_serializing_if = "Option::is_none")]
    pub(super) moving: Option<super::clip_moving::MovingRef>,
    #[serde(default, rename = "mv_rect", skip_serializing_if = "Option::is_none")]
    pub(super) moving_rect: Option<super::clip_motion::PixelRect>,
}

impl FrameLine {
    pub(super) fn shows_new_picture(&self) -> bool { self.new || self.exact.is_some() }
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

/// Session settings Windows granted (O1). It has the fields of
/// `synapse_capture::ObserverSettings` but stays its own stored type: that
/// type exists only on Windows while `clip.json` is read on every platform,
/// and the names stored in `clip.json` must not change with the capture
/// crate. The recorder copies the grant once, when frame zero is written.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub(super) struct ObserverGrant {
    pub(super) cursor_in_picture: bool,
    pub(super) border_hidden: bool,
    pub(super) secondary_windows: bool,
    pub(super) requested_interval_ms: Option<u64>,
    pub(super) dirty_regions: bool,
}

/// A moving segment verified against its actual file before publication.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct CommittedSegment {
    #[serde(flatten)]
    pub(super) segment: super::clip_moving::MovingSegment,
    pub(super) bytes: u64,
    pub(super) sha256: String,
}
impl CommittedSegment {
    pub(super) fn read_back(folder: &Path, segment: super::clip_moving::MovingSegment) -> Result<Self, String> {
        let (bytes, sha256) = file_size_and_sha256(&folder.join(&segment.file)).map_err(|e| e.to_string())?;
        Ok(Self { segment, bytes, sha256 })
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct ExactInfo {
    pub(super) level: i32,
    pub(super) window_log: u32,
    pub(super) group_pictures: u32,
    #[serde(flatten)]
    pub(super) totals: super::clip_exact::ExactSummary,
}
impl From<super::clip_exact::ExactSummary> for ExactInfo {
    fn from(totals: super::clip_exact::ExactSummary) -> Self {
        Self { level: super::clip_exact::EXACT_ZSTD_LEVEL, window_log: super::clip_exact::EXACT_WINDOW_LOG,
            group_pictures: super::clip_exact::EXACT_GROUP_PICTURES, totals }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct MovingInfo {
    pub(super) status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) encoder: Option<super::clip_encoder::EncoderGrant>,
    pub(super) segments: Vec<CommittedSegment>,
}
impl Default for MovingInfo {
    fn default() -> Self { Self { status: "not_used".into(), reason: None, encoder: None, segments: Vec::new() } }
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
    pub(super) way: String,
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
    pub(super) moving: MovingInfo,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) exact: Option<ExactInfo>,
    #[serde(default)]
    pub(super) conditions: Conditions,
}

impl ClipJson {
    /// Include only format files, including the final metadata's own byte length.
    pub(super) fn set_total_bytes(&mut self, folder: &Path) -> Result<(), String> {
        let mut other = std::fs::metadata(folder.join(FRAMES_FILE)).map_err(|e| e.to_string())?.len();
        for directory in ["frames", "moving"] {
            if let Ok(entries) = std::fs::read_dir(folder.join(directory)) {
                for entry in entries {
                    let entry = entry.map_err(|e| e.to_string())?;
                    if entry.file_type().map_err(|e| e.to_string())?.is_file() {
                        other += entry.metadata().map_err(|e| e.to_string())?.len();
                    }
                }
            }
        }
        loop {
            let bytes = other + serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?.len() as u64;
            let detail = self.conditions.t.as_mut().and_then(|t| t.detail.as_mut()).ok_or("missing T detail")?;
            if detail["bytes"].as_u64() == Some(bytes) { return Ok(()); }
            detail["bytes"] = serde_json::json!(bytes);
        }
    }

    /// Reads `clip.json` of a clip folder.
    pub(super) fn read(folder: &Path) -> Result<Self, String> {
        let path = folder.join(CLIP_JSON_FILE);
        let text = retry_brief_hold(|| std::fs::read_to_string(&path))
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

/// Opens a lock file for writing: creates it when missing and never
/// truncates it, so the lock can be taken on it.
pub(super) fn open_lock_file(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
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
        let file = open_lock_file(path)
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
    retry_brief_hold(|| std::fs::rename(&temp, path))
        .map_err(|error| format!("could not replace {}: {error}", path.display()))
}

/// Runs a file operation (a rename that replaces, a read). When another
/// program holds the file for a moment (antivirus, indexer, or another
/// process replacing it: `BRIEF_HOLD_OS_ERRORS`), the operation is tried
/// again, up to `BRIEF_HOLD_ATTEMPTS` times in all and
/// `BRIEF_HOLD_RETRY_SLEEP_MS` apart; any other error fails at once.
pub(super) fn retry_brief_hold<T>(
    mut operation: impl FnMut() -> std::io::Result<T>,
) -> std::io::Result<T> {
    let mut attempt = 1;
    loop {
        match operation() {
            Err(error)
                if attempt < BRIEF_HOLD_ATTEMPTS
                    && cfg!(windows)
                    && error
                        .raw_os_error()
                        .is_some_and(|code| BRIEF_HOLD_OS_ERRORS.contains(&code)) =>
            {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(BRIEF_HOLD_RETRY_SLEEP_MS));
            }
            outcome => return outcome,
        }
    }
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
        let file = open_lock_file(&path)
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
