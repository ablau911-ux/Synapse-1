//! The clip folder of plan 5.0 §3 and everything that reads or writes its
//! format: the folder layout, `frames.jsonl` lines, the IVF container of the
//! video parts, `clip.json` with its atomic publication, the recorder lock and
//! the active-clip registry.
//!
//!   video/part-NNNN.ivf          AV1; a new part at each key picture
//!   stills/still-NNNNNNNN.webp   lossless WebP; the number is the instant it belongs to
//!   frames.jsonl                 one line per instant, in order
//!   clip.json                    the clip's description, replaced atomically

use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};

use fs2::FileExt;
use serde::{Deserialize, Serialize};

/// Version 4: one AV1 video stream plus settled stills (plan 5.0).
pub(super) const CLIP_SCHEMA_VERSION: u32 = 4;
/// The method id stored in `clip.json`.
pub(super) const CLIP_METHOD: &str = "av1-video-and-stills";
pub(super) const CLIP_JSON_FILE: &str = "clip.json";
pub(super) const FRAMES_FILE: &str = "frames.jsonl";
pub(super) const VIDEO_DIR: &str = "video";
pub(super) const STILLS_DIR: &str = "stills";
pub(super) const VIEWS_DIR: &str = "views";
/// Lists clips whose folder is not under the default clips folder; entries
/// stay after the clip is ready or failed.
const REGISTRY_FILE: &str = "active-clips.json";
const LOCK_SUFFIX: &str = ".lock";
/// Held by the recorder for the life of a recording (`RecorderLock`).
const RECORDER_LOCK_FILE: &str = "recorder.lock";
const TEMP_SUFFIX: &str = ".tmp";
/// A rename of `clip.json` and a read of `clip.json` are tried this many
/// times in all while another program holds the file for a moment.
const BRIEF_HOLD_ATTEMPTS: u32 = 5;
/// The sleep between two such attempts.
const BRIEF_HOLD_RETRY_SLEEP_MS: u64 = 100;
/// Windows errors of a file another program holds for a moment: access
/// denied (5), sharing violation (32), lock violation (33).
const BRIEF_HOLD_OS_ERRORS: [i32; 3] = [5, 32, 33];
const HASH_BUFFER_BYTES: usize = 64 * 1024;

/// Loss causes named in `frames.jsonl` and `clip.json`.
pub(super) const LOST_NO_PICTURE_YET: &str = "no_picture_yet";
pub(super) const LOST_SLOT_OVERFLOW: &str = "slot_overflow";
/// The encoder queue was full: the changed picture could not be queued.
pub(super) const LOST_ENCODER_BEHIND: &str = "encoder_behind";
pub(super) const LOST_CAPTURE_FAILED: &str = "capture_failed";
pub(super) const LOST_SERVICE_RESTART: &str = "service_restart";
/// The encoder failed: the instant's picture never reached the video.
pub(super) const LOST_ENCODER_FAILED: &str = "encoder_failed";
/// `frames.jsonl` could not be written at the end of the recording (the
/// video finished): the clip failed.
pub(super) const LOST_INDEX_WRITE_FAILED: &str = "index_write_failed";
/// The recorder thread ended without finishing (a panic): the instant's
/// picture never reached the video.
pub(super) const LOST_RECORDER_ENDED: &str = "recorder_ended";

/// IVF container (the AV1 parts): a 32-byte file header, then per picture a
/// 12-byte header (size, timestamp) and the picture's bytes.
const IVF_SIGNATURE: &[u8; 4] = b"DKIF";
const IVF_VERSION: u16 = 0;
const IVF_HEADER_BYTES: u16 = 32;
const IVF_AV1_FOURCC: &[u8; 4] = b"AV01";
const IVF_TIME_SCALE: u32 = 1;
const IVF_PICTURE_COUNT_OFFSET: u64 = 24;
/// Where the IVF header holds the part's picture width and height (u16 each).
const IVF_WIDTH_OFFSET: usize = 12;
const IVF_HEIGHT_OFFSET: usize = 14;
const IVF_PICTURE_HEADER_BYTES: usize = 12;
const IVF_SIZE_FIELD_BYTES: usize = 4;

/// f: recorded instants per second (base rule). Owned by the clip format:
/// it sets every index line's `t_ms` and the video's time base.
pub(super) const RECORD_FPS: u32 = 20;

/// Longest recording, in milliseconds: 10 minutes (owner's decision of
/// 2026-11-24). `record` and `record_extend` refuse a longer clip, and
/// recovery caps a `clip.json` that plans more at this length, so no number in
/// a call or in a clip folder makes recovery write lost lines without limit.
pub(super) const MAX_DURATION_MS: u64 = 600_000;

/// Default parent of clip folders.
pub(super) fn clips_root() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map_or_else(std::env::temp_dir, PathBuf::from)
        .join("synapse")
        .join("clips")
}

/// The file of video part `part`: `video/part-NNNN.ivf`.
pub fn part_path(folder: &Path, part: u32) -> PathBuf {
    folder.join(VIDEO_DIR).join(format!("part-{part:04}.ivf"))
}

/// The still that belongs to `instant`: `stills/still-NNNNNNNN.webp`.
pub fn still_path(folder: &Path, instant: u64) -> PathBuf {
    folder
        .join(STILLS_DIR)
        .join(format!("still-{instant:08}.webp"))
}

/// One line of `frames.jsonl`: one per instant, in order. Exactly one of
/// `enc` (the picture's place in the video: part, index in the part),
/// `same` (unchanged: shows the picture before it) and `lost` (with its
/// cause) is present; `still` marks the instant whose lossless still exists,
/// `still_of: k` an instant whose settled picture equals the still of instant
/// k (no second file is written). A `lost` line can keep its still mark:
/// recovery found its video picture missing, but its complete still shows the
/// instant's exact picture.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexLine {
    pub instant: u64,
    pub t_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enc: Option<(u32, u32)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub same: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lost: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub still: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub still_of: Option<u64>,
}

impl IndexLine {
    /// The instant whose still file holds this instant's settled picture:
    /// its own (`still`) or the one it repeats (`still_of`); `None` when the
    /// instant is no settle point with a still.
    pub(super) fn still_file(&self) -> Option<u64> {
        if self.still == Some(true) {
            Some(self.instant)
        } else {
            self.still_of
        }
    }

    /// The line as written to `frames.jsonl`, newline included.
    pub(super) fn text(&self) -> Result<String, String> {
        serde_json::to_string(self)
            .map(|line| format!("{line}\n"))
            .map_err(|error| format!("could not serialise index line {}: {error}", self.instant))
    }
}

/// Reads every complete line of `frames.jsonl` (a torn last line is skipped;
/// a missing file is an empty index).
pub fn read_index(folder: &Path) -> std::io::Result<Vec<IndexLine>> {
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

/// One IVF video part being written. Pictures go straight to the file, so a
/// killed recorder keeps every picture written before it; the picture count
/// in the header is set at close (or by `repair_ivf` on recovery).
pub(super) struct IvfWriter {
    file: std::fs::File,
    pictures: u32,
    bytes: u64,
}

impl IvfWriter {
    pub(super) fn create(
        path: &Path,
        width: u32,
        height: u32,
        ticks_per_second: u32,
    ) -> Result<Self, String> {
        let narrow = |value: u32| {
            u16::try_from(value).map_err(|_| format!("IVF cannot hold a size of {value}"))
        };
        let mut header = Vec::with_capacity(usize::from(IVF_HEADER_BYTES));
        header.extend_from_slice(IVF_SIGNATURE);
        header.extend_from_slice(&IVF_VERSION.to_le_bytes());
        header.extend_from_slice(&IVF_HEADER_BYTES.to_le_bytes());
        header.extend_from_slice(IVF_AV1_FOURCC);
        header.extend_from_slice(&narrow(width)?.to_le_bytes());
        header.extend_from_slice(&narrow(height)?.to_le_bytes());
        header.extend_from_slice(&ticks_per_second.to_le_bytes());
        header.extend_from_slice(&IVF_TIME_SCALE.to_le_bytes());
        header.extend_from_slice(&0_u32.to_le_bytes()); // picture count, set at close
        header.extend_from_slice(&0_u32.to_le_bytes()); // unused
        let mut file = std::fs::File::create(path)
            .map_err(|error| format!("could not create {}: {error}", path.display()))?;
        file.write_all(&header)
            .map_err(|error| format!("could not write {}: {error}", path.display()))?;
        Ok(Self {
            file,
            pictures: 0,
            bytes: header.len() as u64,
        })
    }

    /// Pictures appended so far; the next picture gets this index.
    pub(super) const fn pictures(&self) -> u32 {
        self.pictures
    }

    /// Appends one picture; returns the bytes written.
    pub(super) fn append(&mut self, timestamp: u64, data: &[u8]) -> Result<u64, String> {
        let size = u32::try_from(data.len()).map_err(|_| "picture too large for IVF".to_owned())?;
        let mut record = Vec::with_capacity(IVF_PICTURE_HEADER_BYTES + data.len());
        record.extend_from_slice(&size.to_le_bytes());
        record.extend_from_slice(&timestamp.to_le_bytes());
        record.extend_from_slice(data);
        self.file
            .write_all(&record)
            .map_err(|error| format!("could not write an IVF picture: {error}"))?;
        self.pictures += 1;
        self.bytes += record.len() as u64;
        Ok(record.len() as u64)
    }

    /// Writes the picture count into the header; returns the file's size.
    pub(super) fn close(mut self) -> Result<u64, String> {
        self.file
            .seek(SeekFrom::Start(IVF_PICTURE_COUNT_OFFSET))
            .and_then(|_| self.file.write_all(&self.pictures.to_le_bytes()))
            .and_then(|()| self.file.flush())
            .map_err(|error| format!("could not write the IVF picture count: {error}"))?;
        Ok(self.bytes)
    }
}

/// One complete picture of an IVF part.
pub(super) struct IvfPicture {
    pub(super) data: Vec<u8>,
}

/// Every complete picture of an IVF part, in order; a torn last picture is
/// left out.
pub(super) fn read_ivf(path: &Path) -> Result<Vec<IvfPicture>, String> {
    let bytes = std::fs::read(path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    let (pictures, _) = scan_ivf(&bytes, path)?;
    Ok(pictures)
}

/// Makes an IVF part whose writer died readable: cuts a torn last picture
/// and writes the count of complete pictures into the header. Returns that
/// count and the file's new size. A part shorter than its header (its writer
/// died while writing the header) holds no picture: it is left as it is and
/// counts 0, so recovery removes it.
pub(super) fn repair_ivf(path: &Path) -> Result<(u32, u64), String> {
    let bytes = std::fs::read(path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    if bytes.len() < usize::from(IVF_HEADER_BYTES) {
        return Ok((0, bytes.len() as u64));
    }
    let (pictures, end) = scan_ivf(&bytes, path)?;
    let count = u32::try_from(pictures.len())
        .map_err(|_| format!("{} holds too many pictures", path.display()))?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|error| format!("could not open {}: {error}", path.display()))?;
    file.set_len(end)
        .and_then(|()| file.seek(SeekFrom::Start(IVF_PICTURE_COUNT_OFFSET)))
        .and_then(|_| file.write_all(&count.to_le_bytes()))
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("could not repair {}: {error}", path.display()))?;
    Ok((count, end))
}

/// The complete pictures of IVF bytes and the offset where the last one ends.
fn scan_ivf(bytes: &[u8], path: &Path) -> Result<(Vec<IvfPicture>, u64), String> {
    let header = usize::from(IVF_HEADER_BYTES);
    if bytes.len() < header || &bytes[..IVF_SIGNATURE.len()] != IVF_SIGNATURE {
        return Err(format!("{} is not an IVF file", path.display()));
    }
    let mut pictures = Vec::new();
    let mut at = header;
    while bytes.len() >= at + IVF_PICTURE_HEADER_BYTES {
        let mut size = [0_u8; IVF_SIZE_FIELD_BYTES];
        size.copy_from_slice(&bytes[at..at + IVF_SIZE_FIELD_BYTES]);
        let start = at + IVF_PICTURE_HEADER_BYTES;
        let end = start + u32::from_le_bytes(size) as usize;
        if end > bytes.len() {
            break;
        }
        pictures.push(IvfPicture {
            data: bytes[start..end].to_vec(),
        });
        at = end;
    }
    Ok((pictures, at as u64))
}

/// How far the clip got.
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

/// Why the recording ended. Complete parts and stills always stay.
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

/// Width and height of the window and of the recorded picture (one recorded
/// pixel per window pixel).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub(super) struct ClipGeometry {
    pub(super) window_width: u32,
    pub(super) window_height: u32,
    pub(super) scale: u32,
    pub(super) recorded_width: u32,
    pub(super) recorded_height: u32,
}

/// Session settings Windows granted. It has the fields of
/// `synapse_capture::ObserverSettings` but stays its own stored type: that
/// type exists only on Windows while `clip.json` is read on every platform,
/// and the names stored in `clip.json` must not change with the capture
/// crate. The recorder copies the grant once, at instant zero.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub(super) struct ObserverGrant {
    pub(super) cursor_in_picture: bool,
    pub(super) border_hidden: bool,
    pub(super) secondary_windows: bool,
    pub(super) requested_interval_ms: Option<u64>,
    pub(super) dirty_regions: bool,
}

/// The encoder's name and the settings it ran with.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct EncoderInfo {
    pub(super) name: String,
    pub(super) settings: serde_json::Value,
}

/// One complete video part, read back after it was closed. A window can
/// change size while it is recorded; every part holds pictures of one size,
/// and a new size starts a new part.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct PartInfo {
    pub(super) part: u32,
    /// Path relative to the clip folder.
    pub(super) file: String,
    pub(super) first_instant: u64,
    pub(super) last_instant: u64,
    pub(super) pictures: u32,
    /// The part's picture size. Absent in a `clip.json` written before parts
    /// had their own size: `ClipJson::read` fills in the clip's geometry.
    #[serde(default)]
    pub(super) width: u32,
    #[serde(default)]
    pub(super) height: u32,
    pub(super) bytes: u64,
    pub(super) sha256: String,
}

impl PartInfo {
    /// Reads a closed part back: its picture size (from its IVF header), its
    /// size on disk and SHA-256.
    pub(super) fn read_back(
        folder: &Path,
        part: u32,
        first_instant: u64,
        last_instant: u64,
        pictures: u32,
    ) -> Result<Self, String> {
        let path = part_path(folder, part);
        let (width, height) = ivf_picture_size(&path)?;
        let (bytes, sha256) = file_size_and_sha256(&path)
            .map_err(|error| format!("could not read back {}: {error}", path.display()))?;
        Ok(Self {
            part,
            file: relative_name(folder, &path),
            first_instant,
            last_instant,
            pictures,
            width,
            height,
            bytes,
            sha256,
        })
    }
}

/// The picture width and height an IVF part's header names.
fn ivf_picture_size(path: &Path) -> Result<(u32, u32), String> {
    let mut header = [0_u8; IVF_HEADER_BYTES as usize];
    std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut header))
        .map_err(|error| {
            format!(
                "could not read the IVF header of {}: {error}",
                path.display()
            )
        })?;
    let field = |at: usize| u32::from(u16::from_le_bytes([header[at], header[at + 1]]));
    Ok((field(IVF_WIDTH_OFFSET), field(IVF_HEIGHT_OFFSET)))
}

/// One lossless still.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct StillInfo {
    pub(super) instant: u64,
    /// Path relative to the clip folder.
    pub(super) file: String,
    pub(super) bytes: u64,
}

impl StillInfo {
    pub(super) fn new(folder: &Path, instant: u64, bytes: u64) -> Self {
        Self {
            instant,
            file: relative_name(folder, &still_path(folder, instant)),
            bytes,
        }
    }
}

/// `path` relative to `folder`, with forward slashes.
fn relative_name(folder: &Path, path: &Path) -> String {
    path.strip_prefix(folder)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// A lost instant with its cause.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct LostInstant {
    pub(super) instant: u64,
    pub(super) cause: String,
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

impl ConditionStatus {
    pub(super) fn held(detail: serde_json::Value) -> Self {
        Self {
            status: "held".to_owned(),
            failed_instants: Vec::new(),
            detail: Some(detail),
        }
    }
}

/// The `conditions` block of `clip.json`, with worded keys.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct Conditions {
    /// Every instant has a picture, `same`, or a named loss.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) capture: Option<ConditionStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) geometry: Option<ConditionStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) quality: Option<ConditionStatus>,
    /// N = N_enc + N_same + N_lost and the clip's length.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) totals: Option<ConditionStatus>,
}

/// `clip.json` (plan 5.0 §3).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct ClipJson {
    pub(super) schema_version: u32,
    pub(super) method: String,
    pub(super) clip_id: String,
    pub(super) synapse_version: String,
    pub(super) state: ClipState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) failure: Option<String>,
    /// A failed clip: the cause (`LOST_*`) its lost instants get when its
    /// index is made to agree with the pictures on disk; read through
    /// `failed_lost_cause`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) lost_cause: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) stop_reason: Option<StopReason>,
    pub(super) fps: u32,
    /// Planned end in ms from s_0; `record_extend` moves it.
    pub(super) end_ms: u64,
    /// Instants handled so far, as milliseconds (k / f).
    pub(super) recorded_ms: u64,
    /// Lines in `frames.jsonl`: N = N_enc + N_same + N_lost.
    #[serde(default)]
    pub(super) instants: u64,
    pub(super) target: ClipTargetInfo,
    pub(super) clock: ClipClock,
    /// Width and height of the clip.
    pub(super) geometry: ClipGeometry,
    pub(super) observer: ObserverGrant,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) encoder: Option<EncoderInfo>,
    #[serde(default)]
    pub(super) parts: Vec<PartInfo>,
    #[serde(default)]
    pub(super) stills: Vec<StillInfo>,
    /// Each lost instant exactly once, in order.
    #[serde(default)]
    pub(super) lost: Vec<LostInstant>,
    /// B = Σ video parts + Σ stills + frames.jsonl + clip.json.
    #[serde(default)]
    pub(super) bytes: u64,
    #[serde(default)]
    pub(super) conditions: Conditions,
}

impl ClipJson {
    /// Sets `bytes` to B, including this `clip.json`'s own final length.
    pub(super) fn set_total_bytes(&mut self, folder: &Path) -> Result<(), String> {
        let mut files = std::fs::metadata(folder.join(FRAMES_FILE))
            .map_err(|error| format!("could not read the size of {FRAMES_FILE}: {error}"))?
            .len();
        for directory in [VIDEO_DIR, STILLS_DIR] {
            files += directory_bytes(&folder.join(directory))?;
        }
        loop {
            let own = serde_json::to_vec_pretty(self)
                .map_err(|error| format!("could not serialise clip.json: {error}"))?
                .len() as u64;
            if self.bytes == files + own {
                return Ok(());
            }
            self.bytes = files + own;
        }
    }

    /// Reads `clip.json` of a clip folder.
    pub(super) fn read(folder: &Path) -> Result<Self, String> {
        let path = folder.join(CLIP_JSON_FILE);
        let text = retry_brief_hold(|| std::fs::read_to_string(&path))
            .map_err(|error| format!("could not read {}: {error}", path.display()))?;
        let mut clip: Self = serde_json::from_str(&text)
            .map_err(|error| format!("{} is not valid: {error}", path.display()))?;
        // Parts written before parts had their own size hold the clip's size.
        let geometry = (clip.geometry.recorded_width, clip.geometry.recorded_height);
        for part in clip.parts.iter_mut().filter(|part| part.width == 0) {
            (part.width, part.height) = geometry;
        }
        if clip.schema_version != CLIP_SCHEMA_VERSION {
            return Err(format!(
                "{} schema_version {} is not supported (expected {CLIP_SCHEMA_VERSION})",
                path.display(),
                clip.schema_version
            ));
        }
        // Every reader divides by fps (index times, part times): a clip that
        // says 0 is refused here, once, for recovery, status and the viewer.
        if clip.fps == 0 {
            return Err(format!("{} has zero fps", path.display()));
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

    /// The cause the lost instants of a failed clip get: its `lost_cause`;
    /// a clip that failed before that field existed failed in the encoder.
    pub(super) fn failed_lost_cause(&self) -> &str {
        self.lost_cause.as_deref().unwrap_or(LOST_ENCODER_FAILED)
    }

    /// Adds lost instants (an instant already listed keeps its first cause),
    /// so each lost instant is listed exactly once, and carries the list
    /// into the `capture` and `totals` conditions where they exist.
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
        for condition in [
            self.conditions.capture.as_mut(),
            self.conditions.totals.as_mut(),
        ]
        .into_iter()
        .flatten()
        {
            condition.status = "not_held".to_owned();
            condition.failed_instants.clone_from(&failed);
        }
    }
}

/// Total size of the files directly inside `directory` (0 when it is missing).
fn directory_bytes(directory: &Path) -> Result<u64, String> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(format!("could not list {}: {error}", directory.display())),
    };
    let mut bytes = 0;
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let metadata = entry.metadata().map_err(|error| error.to_string())?;
        if metadata.is_file() {
            bytes += metadata.len();
        }
    }
    Ok(bytes)
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
    write_bytes_atomically(path, &bytes)
}

/// Writes `bytes` to `<path>.tmp`, flushes it to disk and renames it over `path`.
pub(super) fn write_bytes_atomically(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temp = path.with_extension(format!(
        "{}{TEMP_SUFFIX}",
        path.extension()
            .map_or_else(String::new, |extension| extension
                .to_string_lossy()
                .into_owned())
    ));
    let mut file = std::fs::File::create(&temp)
        .map_err(|error| format!("could not create {}: {error}", temp.display()))?;
    file.write_all(bytes)
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
/// life of a recording. Windows releases it when the recorder's process
/// ends, also by a crash, so recovery in any service process can tell a live
/// clip from one whose service ended.
pub(super) struct RecorderLock {
    file: std::fs::File,
}

impl RecorderLock {
    /// Takes the lock of a clip folder.
    pub(super) fn acquire(folder: &Path) -> Result<Self, String> {
        let path = folder.join(RECORDER_LOCK_FILE);
        let file = open_lock_file(&path)
            .map_err(|error| format!("could not open {}: {error}", path.display()))?;
        file.try_lock_exclusive()
            .map_err(|error| format!("could not lock {}: {error}", path.display()))?;
        Ok(Self { file })
    }

    /// `true` while a recorder holds the lock of this clip folder. Without
    /// the file no recorder is alive; when the file cannot be opened or
    /// locked, the recorder counts as alive, so a live clip is never taken
    /// for a dead one.
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

    let mut file = std::fs::File::open(path)?;
    let mut hasher = sha2::Sha256::new();
    let mut buffer = vec![0_u8; HASH_BUFFER_BYTES];
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

/// The active-clip registry under the default clips folder. An entry stays
/// after the clip is ready or failed, so `record_status` and `record_stop`
/// still find a clip in another folder by its id; recovery skips clips in a
/// terminal state.
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

    /// Drops a clip from the registry (a `record` that failed before its
    /// first picture removed its folder).
    pub(super) fn remove(clip_id: &str) -> Result<(), String> {
        if !Self::entries().iter().any(|entry| entry.clip_id == clip_id) {
            return Ok(());
        }
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
