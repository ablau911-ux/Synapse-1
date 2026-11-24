//! Instant-clock recording (plan 5.0): one AV1 video stream of the whole
//! window plus a lossless still each time it settles (`clip_stream`).
//! Privacy gates and the capture observer remain on the recorder path.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use rmcp::{RoleServer, service::RequestContext};
use serde_json::json;
use synapse_capture::ClipSource;
use synapse_core::error_codes;

use super::clip_folder::{
    CLIP_JSON_FILE, ClipJson, ClipRegistry, ClipState, FRAMES_FILE, RECORD_FPS, StopReason,
    clips_root, file_size_and_sha256,
};
use super::{
    CaptureClipParams, CaptureClipResponse, ClipStatusResponse, ErrorData, SynapseService,
};
use crate::m1::{ClipSegmentInfo, mcp_error};
use crate::m3::SharedM3State;
use crate::m3::timeline_control::{RecorderControl, SuppressReason};

/// f_r: the reference screen; T_r = 1 / f_r is the hold-time bound (C17).
pub(super) const REFERENCE_REFRESH_HZ: u32 = 60;
/// t_def: length of a recording unless the call gives one.
pub(super) const DEFAULT_DURATION_MS: u64 = 60_000;
/// Shortest recording a call may ask for.
pub(super) const MIN_DURATION_MS: u64 = 100;
/// How often the free space of the clip volume is measured (A11, F7).
const DISK_CHECK_MS: u64 = 1_000;
/// How often the windows on a recorded monitor are checked for an excluded app (E6).
const EXCLUDED_APP_CHECK_MS: u64 = 1_000;
/// How long to wait for the first frame (kept from version 1).
const FIRST_FRAME_TIMEOUT_MS: u64 = 3_000;
const CLIP_SOURCE_OF_TRUTH: &str =
    "clip folder files (clip.json, frames.jsonl, video/, stills/) + target metadata";

/// The target id in error data: `window:0x…` or `monitor:N`.
fn target_id(target: ClipSource) -> String {
    match target {
        ClipSource::Window { hwnd } => format!("window:{hwnd:#x}"),
        ClipSource::Monitor { monitor_index } => format!("monitor:{monitor_index}"),
    }
}

impl StopReason {
    const fn from_privacy(reason: SuppressReason) -> Self {
        match reason {
            SuppressReason::Paused => Self::PrivacyPaused,
            SuppressReason::ExcludedApp => Self::PrivacyExcludedApp,
        }
    }
}

/// The shared controls of the running recording: stop, the planned end, and
/// the length recorded so far (published on every instant, not written to disk).
struct RecordingControl {
    stop: AtomicBool,
    end_ms: AtomicU64,
    recorded_ms: AtomicU64,
}

/// The one running recording of the service (A2: only one recorder, so the
/// 10 % rule holds).
struct ActiveClip {
    clip_id: String,
    folder: PathBuf,
    control: Arc<RecordingControl>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// Owns the rule "one recording at a time" for the whole service.
struct Recorders;

static ACTIVE: Mutex<Option<ActiveClip>> = Mutex::new(None);

impl Recorders {
    fn active() -> std::sync::MutexGuard<'static, Option<ActiveClip>> {
        ACTIVE.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Claims the recorder for a clip; refused while another recording runs.
    fn claim(
        clip_id: &str,
        folder: &std::path::Path,
        end_ms: u64,
    ) -> Result<Arc<RecordingControl>, ErrorData> {
        let mut active = Self::active();
        if let Some(running) = active.as_ref() {
            return Err(mcp_error(
                error_codes::CAPTURE_TARGET_INVALID,
                format!(
                    "screenshot operation=record refused: clip {} is recording into {}",
                    running.clip_id,
                    running.folder.display()
                ),
            ));
        }
        let control = Arc::new(RecordingControl {
            stop: AtomicBool::new(false),
            end_ms: AtomicU64::new(end_ms),
            recorded_ms: AtomicU64::new(0),
        });
        *active = Some(ActiveClip {
            clip_id: clip_id.to_owned(),
            folder: folder.to_path_buf(),
            control: Arc::clone(&control),
            thread: None,
        });
        Ok(control)
    }

    fn attach_thread(clip_id: &str, thread: std::thread::JoinHandle<()>) {
        if let Some(active) = Self::active().as_mut()
            && active.clip_id == clip_id
        {
            active.thread = Some(thread);
        }
    }

    fn release(clip_id: &str) {
        let mut active = Self::active();
        if active
            .as_ref()
            .is_some_and(|running| running.clip_id == clip_id)
        {
            *active = None;
        }
    }

    /// The controls and folder of the running clip with this id.
    fn find(clip_id: &str) -> Option<(Arc<RecordingControl>, PathBuf)> {
        Self::active()
            .as_ref()
            .filter(|running| running.clip_id == clip_id)
            .map(|running| (Arc::clone(&running.control), running.folder.clone()))
    }

    /// Asks the running clip to stop and hands back its thread to join.
    fn stop(clip_id: &str) -> Option<std::thread::JoinHandle<()>> {
        let mut active = Self::active();
        let running = active
            .as_mut()
            .filter(|running| running.clip_id == clip_id)?;
        running.control.stop.store(true, Ordering::Relaxed);
        running.thread.take()
    }
}

/// A validated recording request.
struct ClipPlan {
    clip_id: String,
    target: ClipSource,
    /// The window's process name for the privacy gate; monitors read the
    /// foreground app on every instant and every visible window each second.
    app: Option<String>,
    folder: PathBuf,
    end_ms: u64,
    privacy: Arc<RecorderControl>,
}

impl ClipPlan {
    /// The privacy gate's verdict for what the clip shows right now.
    fn privacy_stop(&self) -> Option<StopReason> {
        let foreground_app;
        let app = match self.target {
            ClipSource::Window { .. } => self.app.as_deref(),
            ClipSource::Monitor { .. } => {
                foreground_app = synapse_a11y::current_foreground_context()
                    .ok()
                    .map(|context| context.process_name);
                foreground_app.as_deref()
            }
        };
        self.privacy
            .suppress_reason(app)
            .map(StopReason::from_privacy)
    }

    /// E6: for a monitor, an excluded app visible anywhere on it stops the clip,
    /// even when it is not in front.
    fn excluded_app_visible(&self) -> bool {
        let ClipSource::Monitor { monitor_index } = self.target else {
            return false;
        };
        synapse_a11y::visible_top_level_window_contexts()
            .unwrap_or_default()
            .iter()
            .filter(|context| context.monitor_index == monitor_index)
            .any(|context| {
                self.privacy.suppress_reason(Some(&context.process_name))
                    == Some(SuppressReason::ExcludedApp)
            })
    }
}

impl SynapseService {
    /// `record`: starts a recording and returns once `clip.json` and the first
    /// frame are written. The gates and the recording run on their own named
    /// thread, never on a tokio worker; the recording outlives this call.
    pub(super) async fn capture_clip(
        &self,
        params: CaptureClipParams,
        request_context: &RequestContext<RoleServer>,
    ) -> Result<CaptureClipResponse, ErrorData> {
        let target = self
            .capture_clip_target(&params, request_context)
            .map_err(|error| with_clip_context(error, "session_target"))?;
        let target_id = target_id(target);
        let m3_state = self.m3_state_handle();
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let thread = std::thread::Builder::new()
            .name("synapse-clip-recorder".to_owned())
            .spawn(move || run_recorder_thread(&m3_state, target, &params, sender))
            .map_err(|error| thread_failed(format!("could not start the recorder thread: {error}")))
            .map_err(|error| with_clip_context(error, &target_id))?;
        let started = receiver.await.unwrap_or_else(|_| {
            Err(thread_failed(
                "the recorder thread ended without a result (panicked)".to_owned(),
            ))
        });
        match started {
            Ok(response) => {
                Recorders::attach_thread(&response.clip_id, thread);
                Ok(response)
            }
            Err(error) => {
                let _ = tokio::task::spawn_blocking(move || thread.join()).await;
                Err(with_clip_context(error, &target_id))
            }
        }
    }

    /// An explicit monitor or window wins; otherwise the session's window
    /// target, resolved like `screenshot operation=capture`.
    fn capture_clip_target(
        &self,
        params: &CaptureClipParams,
        request_context: &RequestContext<RoleServer>,
    ) -> Result<ClipSource, ErrorData> {
        if let Some(monitor_index) = params.monitor_index {
            return Ok(ClipSource::Monitor { monitor_index });
        }
        let session_target = self.request_session_target(request_context)?;
        let hwnd = super::m1_tools::perception_window_hwnd(
            "screenshot operation=record",
            &session_target,
            params.window_hwnd,
        )?
        .ok_or_else(|| {
            mcp_error(
                error_codes::CAPTURE_TARGET_INVALID,
                "screenshot operation=record requires window_hwnd, monitor_index, or a bound session window target (set_target)",
            )
        })?;
        Ok(ClipSource::Window { hwnd })
    }
}

/// `record_status`: the state of a clip, read from its `clip.json`.
pub(super) fn clip_status(clip_id: &str) -> Result<ClipStatusResponse, ErrorData> {
    if let Some((control, folder)) = Recorders::find(clip_id) {
        return status_of(&folder, Some(&control));
    }
    let folder = ClipRegistry::folder_of(clip_id).ok_or_else(|| {
        mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "no clip with id {clip_id} in the active-clip registry or the default clips folder"
            ),
        )
    })?;
    status_of(&folder, None)
}

/// `record_stop`: ends the recording now; idempotent by clip id.
pub(super) async fn clip_stop(clip_id: &str) -> Result<ClipStatusResponse, ErrorData> {
    if let Some(thread) = Recorders::stop(clip_id) {
        let _ = tokio::task::spawn_blocking(move || thread.join()).await;
    }
    clip_status(clip_id)
}

/// `record_extend`: moves the end of a running recording by `add_ms`.
pub(super) fn clip_extend(clip_id: &str, add_ms: u64) -> Result<ClipStatusResponse, ErrorData> {
    let (control, folder) = Recorders::find(clip_id).ok_or_else(|| {
        mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("clip {clip_id} is not recording; only a running recording can be extended"),
        )
    })?;
    let end_ms = control.end_ms.fetch_add(add_ms, Ordering::Relaxed) + add_ms;
    ClipJson::update(&folder, |clip| clip.end_ms = clip.end_ms.max(end_ms))
        .map_err(internal_error)?;
    status_of(&folder, Some(&control))
}

/// The status from `clip.json`; while the clip is still recording, the length
/// is the one the recorder publishes on every instant (`live`).
fn status_of(
    folder: &std::path::Path,
    live: Option<&RecordingControl>,
) -> Result<ClipStatusResponse, ErrorData> {
    let clip = ClipJson::read(folder).map_err(internal_error)?;
    let recorded_ms = match live {
        Some(control) if clip.state == ClipState::Recording => clip
            .recorded_ms
            .max(control.recorded_ms.load(Ordering::Relaxed)),
        _ => clip.recorded_ms,
    };
    Ok(ClipStatusResponse {
        clip_id: clip.clip_id.clone(),
        folder: folder.to_string_lossy().into_owned(),
        state: clip.state.as_str().to_owned(),
        stop_reason: clip.stop_reason.map(|reason| reason.as_str().to_owned()),
        failure: clip.failure.clone(),
        seconds_recorded: recorded_ms as f64 / 1_000.0,
        end_ms: clip.end_ms,
        // The exact pixels of a clip are its lossless stills.
        exact_bytes: clip.stills.iter().map(|still| still.bytes).sum(),
        // One segment per complete video part, covering the whole window.
        segments: clip
            .parts
            .iter()
            .map(|part| ClipSegmentInfo {
                file: part.file.clone(),
                bytes: part.bytes,
                sha256: part.sha256.clone(),
                start_ms: part.first_instant * 1_000 / u64::from(clip.fps),
                end_ms: (part.last_instant + 1) * 1_000 / u64::from(clip.fps),
                // Each part holds pictures of its own size (a resize starts a new part).
                rect: [0, 0, part.width, part.height],
                frames: part.pictures,
            })
            .collect(),
        conditions: serde_json::to_value(&clip.conditions).unwrap_or_default(),
    })
}

type StartSender = tokio::sync::oneshot::Sender<Result<CaptureClipResponse, ErrorData>>;

/// The recorder thread: gates, `clip.json`, the observer, then every instant
/// until a stop. The start result goes to the `record` call through `started`.
fn run_recorder_thread(
    m3_state: &SharedM3State,
    target: ClipSource,
    params: &CaptureClipParams,
    started: StartSender,
) {
    let plan = match capture_clip_plan(m3_state, target, params) {
        Ok(plan) => plan,
        Err(error) => {
            let _ = started.send(Err(error));
            return;
        }
    };
    let control = match Recorders::claim(&plan.clip_id, &plan.folder, plan.end_ms) {
        Ok(control) => control,
        Err(error) => {
            let _ = started.send(Err(error));
            return;
        }
    };
    let mut claim = RecorderClaim {
        clip_id: &plan.clip_id,
        folder: &plan.folder,
        returned: false,
    };
    record(&plan, &control, started);
    claim.returned = true;
}

/// Ends the recorder's claim when the recorder thread ends, also by a panic:
/// the live share is cleared and the one-recording claim released, so a later
/// `record` is not refused until the service restarts. A recorder that did
/// not return (a panic; only dev builds unwind) leaves a clip still
/// `recording` as `failed`, so the worker does not wait for it forever.
struct RecorderClaim<'a> {
    clip_id: &'a str,
    folder: &'a Path,
    returned: bool,
}

impl Drop for RecorderClaim<'_> {
    fn drop(&mut self) {
        if !self.returned && self.folder.join(CLIP_JSON_FILE).is_file() {
            let _ = ClipJson::update(self.folder, |clip| {
                if clip.state == ClipState::Recording {
                    clip.state = ClipState::Failed;
                    clip.failure = Some("the recorder thread ended without finishing".to_owned());
                }
            });
        }
        Recorders::release(self.clip_id);
    }
}

/// A2: applies the start gates (storage pressure on the clip volume, privacy,
/// Windows build, folder) and fixes the folder. Runs on the recorder thread:
/// opening storage may block.
fn capture_clip_plan(
    m3_state: &SharedM3State,
    target: ClipSource,
    params: &CaptureClipParams,
) -> Result<ClipPlan, ErrorData> {
    let privacy = {
        let mut guard = m3_state.lock().map_err(|_poisoned| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                "M3 service state lock poisoned",
            )
        })?;
        guard
            .ensure_recorder_control()
            .map_err(|error| mcp_error(error_codes::TOOL_INTERNAL_ERROR, format!("{error:#}")))?
    };
    let clip_id = uuid::Uuid::new_v4().to_string();
    let folder = params
        .folder
        .as_ref()
        .map_or_else(|| clips_root().join(&clip_id), PathBuf::from);
    if folder.exists() {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "screenshot operation=record folder already exists: {}",
                folder.display()
            ),
        ));
    }
    let volume = folder
        .ancestors()
        .find(|ancestor| ancestor.exists())
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(clips_root);
    let free = synapse_storage::volume_free_bytes(&volume).map_err(|error| {
        internal_error(format!(
            "could not read the free space of {}: {error}",
            volume.display()
        ))
    })?;
    let level = synapse_storage::level_for_free_bytes(free);
    if level != synapse_storage::DiskPressureLevel::Normal {
        let code = level
            .code()
            .unwrap_or(error_codes::STORAGE_DISK_PRESSURE_LEVEL_1);
        return Err(mcp_error(
            code,
            format!(
                "screenshot operation=record refused: the clip volume {} has {free} bytes free ({code})",
                volume.display()
            ),
        ));
    }
    let app = match target {
        ClipSource::Window { hwnd } => synapse_a11y::foreground_context(hwnd)
            .ok()
            .map(|context| context.process_name),
        ClipSource::Monitor { .. } => None,
    };
    let plan = ClipPlan {
        clip_id,
        target,
        app,
        folder,
        end_ms: params.duration_ms,
        privacy,
    };
    if let Some(reason) = plan.privacy_stop() {
        return Err(stop_error(reason));
    }
    if plan.excluded_app_visible() {
        return Err(stop_error(StopReason::PrivacyExcludedApp));
    }
    Ok(plan)
}

#[cfg(windows)]
fn record(plan: &ClipPlan, control: &RecordingControl, started: StartSender) {
    live::record(plan, control, started);
}

#[cfg(not(windows))]
fn record(_plan: &ClipPlan, _control: &RecordingControl, started: StartSender) {
    let _ = started.send(Err(mcp_error(
        error_codes::CAPTURE_GRAPHICS_API_UNSUPPORTED,
        "screenshot operation=record requires Windows (Windows.Graphics.Capture)",
    )));
}

/// Adds operation, source of truth, target id and a remediation to an error's
/// data (keeping any values already there).
fn with_clip_context(mut error: ErrorData, target_id: &str) -> ErrorData {
    let mut data = match error.data.take() {
        Some(serde_json::Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    };
    let code = data
        .get("code")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(error_codes::TOOL_INTERNAL_ERROR)
        .to_owned();
    let remediation = match code.as_str() {
        error_codes::CAPTURE_TARGET_LOST => {
            "the target window closed; bind a live window and record again"
        }
        error_codes::CAPTURE_GRAPHICS_API_UNSUPPORTED => {
            "Windows.Graphics.Capture is unavailable for this target or this Windows is older than 10 1903; record a monitor or another window"
        }
        error_codes::CAPTURE_TARGET_INVALID => {
            "bind a live, visible window (or pass a valid monitor_index), check the timeline privacy gate and that no other recording runs, and retry"
        }
        error_codes::STORAGE_DISK_PRESSURE_LEVEL_1
        | error_codes::STORAGE_DISK_PRESSURE_LEVEL_2
        | error_codes::STORAGE_DISK_PRESSURE_LEVEL_3
        | error_codes::STORAGE_DISK_PRESSURE_LEVEL_4 => {
            "free disk space on the clip volume (at least 2 GB), then retry"
        }
        _ => "retry; if it repeats, check the daemon log for CAPTURE_CLIP entries",
    };
    let defaults = [
        ("code", json!(code)),
        ("tool", json!("screenshot")),
        ("operation", json!("record")),
        ("source_of_truth", json!(CLIP_SOURCE_OF_TRUTH)),
        ("target", json!(target_id)),
        ("remediation", json!(remediation)),
    ];
    for (key, value) in defaults {
        data.entry(key).or_insert(value);
    }
    error.data = Some(serde_json::Value::Object(data));
    error
}

fn internal_error(message: String) -> ErrorData {
    mcp_error(
        error_codes::TOOL_INTERNAL_ERROR,
        format!("screenshot operation=record: {message}"),
    )
}

/// Refusal for a stop reason that applies before the first frame is written.
fn stop_error(reason: StopReason) -> ErrorData {
    match reason {
        StopReason::TargetLost => mcp_error(
            error_codes::CAPTURE_TARGET_LOST,
            "screenshot operation=record: the target window closed before the first frame",
        ),
        _ => mcp_error(
            error_codes::CAPTURE_TARGET_INVALID,
            format!(
                "screenshot operation=record refused by the timeline privacy gate: {}",
                reason.as_str()
            ),
        ),
    }
}

/// `CAPTURE_THREAD_FAILED`, with the code owned by `synapse-capture`.
fn thread_failed(detail: String) -> ErrorData {
    let error = synapse_capture::CaptureError::ThreadFailed { detail };
    mcp_error(
        error.code(),
        format!("screenshot operation=record: {error}"),
    )
}

#[cfg(windows)]
mod live {
    use std::path::Path;
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    use synapse_capture::{
        ClipObserver, ClipSource, HNS_PER_MS, ObserverStats, current_process_cpu_hns,
    };
    use synapse_core::error_codes;

    use super::super::clip_folder::{
        CLIP_METHOD, CLIP_SCHEMA_VERSION, ClipClock, ClipGeometry, ClipJson, ClipRegistry,
        ClipState, ClipTargetInfo, ConditionStatus, Conditions, LOST_CAPTURE_FAILED,
        LOST_SLOT_OVERFLOW, ObserverGrant, RecorderLock, StopReason,
    };
    use super::super::clip_stream::{
        ClipStream, SETTLE_INSTANTS, StageTime, StreamInput, StreamTimes,
    };
    use super::super::{CaptureClipResponse, ErrorData};
    use super::{
        CLIP_JSON_FILE, ClipPlan, DISK_CHECK_MS, EXCLUDED_APP_CHECK_MS, FIRST_FRAME_TIMEOUT_MS,
        FRAMES_FILE, RECORD_FPS, REFERENCE_REFRESH_HZ, RecordingControl, StartSender,
        file_size_and_sha256, internal_error, stop_error, thread_failed,
    };
    use crate::m1::mcp_error;

    const HNS_PER_SECOND: i64 = 10_000_000;
    const PIXEL_SCALE: u32 = 1;
    const BGRA_BLACK: [u8; 3] = [0, 0, 0];
    const BGRA_PIXEL_BYTES: usize = 4;

    /// 1 / f in 100 ns units.
    const fn instant_hns() -> i64 {
        HNS_PER_SECOND / RECORD_FPS as i64
    }

    /// k / f in milliseconds.
    const fn instant_ms(instant: u64) -> u64 {
        instant * 1_000 / RECORD_FPS as u64
    }

    /// One clip. Sends the start result as soon as `clip.json` and instant
    /// zero are written.
    pub(super) fn record(plan: &ClipPlan, control: &RecordingControl, started: StartSender) {
        let mut recording = match Recording::start(plan) {
            Ok(recording) => recording,
            Err(error) => {
                discard_folder(plan);
                let _ = started.send(Err(error));
                return;
            }
        };
        match recording.response() {
            Ok(response) => {
                let _ = started.send(Ok(response));
            }
            Err(error) => {
                let _ = started.send(Err(error));
                recording.finish(StopReason::WriteFailed);
                return;
            }
        }
        let reason = recording.run(control);
        recording.finish(reason);
    }

    /// A `record` that fails before its first picture removes the folder it
    /// created (plan §3; #290). `capture_clip_plan` refuses a folder that
    /// existed before the call, so this never touches one.
    fn discard_folder(plan: &ClipPlan) {
        match std::fs::remove_dir_all(&plan.folder) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => tracing::warn!(
                code = "CAPTURE_CLIP_DISCARD_FAILED",
                folder = %plan.folder.display(),
                error = %error
            ),
        }
        if let Err(error) = ClipRegistry::remove(&plan.clip_id) {
            tracing::warn!(code = "CAPTURE_CLIP_DISCARD_FAILED", error = %error);
        }
    }

    /// Free space on the clip volume, including writes since the last measurement.
    struct DiskGuard {
        measured_free: u64,
        written_since: u64,
        measured_at: Instant,
    }
    impl DiskGuard {
        fn new(folder: &Path) -> Result<Self, String> {
            Ok(Self {
                measured_free: Self::measure(folder)?,
                written_since: 0,
                measured_at: Instant::now(),
            })
        }
        fn measure(folder: &Path) -> Result<u64, String> {
            synapse_storage::volume_free_bytes(folder).map_err(|e| e.to_string())
        }
        fn wrote(&mut self, bytes: u64) {
            self.written_since += bytes;
        }
        fn permits(&mut self, folder: &Path) -> Result<bool, String> {
            if self.measured_at.elapsed() >= Duration::from_millis(DISK_CHECK_MS) {
                self.measured_free = Self::measure(folder)?;
                self.written_since = 0;
                self.measured_at = Instant::now();
            }
            Ok(synapse_storage::level_for_free_bytes(
                self.measured_free.saturating_sub(self.written_since),
            ) == synapse_storage::DiskPressureLevel::Normal)
        }
    }

    /// What one recording cost as a whole: wall time, the process's processor
    /// time and the `clip.json` writes; the stages report their own totals.
    #[derive(Clone, Copy)]
    struct RecordingTimes {
        started: Instant,
        process_cpu_at_start_hns: i64,
        clip_json: StageTime,
    }

    impl RecordingTimes {
        fn begin() -> Self {
            Self {
                started: Instant::now(),
                process_cpu_at_start_hns: current_process_cpu_hns(),
                clip_json: StageTime::default(),
            }
        }

        /// The one `CLIP_RECORD_TIMING` line of a recording: totals of every
        /// stage, so its processor cost is explained by measurement.
        fn log(
            &self,
            clip_id: &str,
            observer: &ObserverStats,
            instants: u64,
            stream: &StreamTimes,
        ) {
            let video = &stream.video;
            let wall_ms = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
            let process_cpu_ms =
                (current_process_cpu_hns() - self.process_cpu_at_start_hns) / HNS_PER_MS;
            tracing::info!(
                code = "CLIP_RECORD_TIMING",
                clip_id,
                instants,
                wall_ms,
                process_cpu_ms,
                frames_delivered = observer.frames_delivered,
                readbacks = observer.readbacks,
                readback_ms = observer.readback_hns / HNS_PER_MS,
                readback_thread_cpu_ms = observer.readback_cpu_hns / HNS_PER_MS,
                compares = stream.compare.count,
                compare_ms = stream.compare.total_ms(),
                conversions = video.convert.count,
                convert_ms = video.convert.total_ms(),
                submits = video.submit.count,
                submit_ms = video.submit.total_ms(),
                max_queue_depth = video.max_queue_depth,
                encodes = video.encode.count,
                encode_ms = video.encode.total_ms(),
                slowest_encode_ms = video.encode.slowest_ms(),
                key_encodes = video.key_encode.count,
                key_encode_ms = video.key_encode.total_ms(),
                stills = stream.stills_written,
                stills_reused = stream.stills_reused,
                stills_skipped_budget = stream.stills_skipped_budget,
                stills_ms = stream.stills.total_ms(),
                index_writes = stream.index.count,
                index_ms = stream.index.total_ms(),
                clip_json_writes = self.clip_json.count,
                clip_json_ms = self.clip_json.total_ms(),
                "clip recording cost by stage"
            );
        }
    }

    /// The live part of one clip: the observer hands each instant's frame
    /// to the stream.
    struct Recording<'a> {
        plan: &'a ClipPlan,
        observer: ClipObserver,
        stream: ClipStream,
        disk: DiskGuard,
        origin_hns: i64,
        geometry: ClipGeometry,
        /// Composition time of the frame last handed to the stream; `None`
        /// after a lost instant, so the next frame is read back again.
        last_composed: Option<i64>,
        next_instant: u64,
        excluded_checked_at: Instant,
        times: RecordingTimes,
        /// Held for the life of the recording; declared last, so it is
        /// released after everything else, once `finish` wrote the stop.
        _recorder_lock: RecorderLock,
    }

    impl<'a> Recording<'a> {
        /// Folder, lock, `clip.json`, observer, first frame and instant zero.
        fn start(plan: &'a ClipPlan) -> Result<Self, ErrorData> {
            let times = RecordingTimes::begin();
            // The observer owns the Windows build rule; applied before the
            // clip folder exists, so an old Windows leaves nothing behind.
            synapse_capture::check_windows_build().map_err(|error| {
                mcp_error(
                    error.code(),
                    format!("screenshot operation=record: {error}"),
                )
            })?;
            std::fs::create_dir_all(&plan.folder).map_err(|error| {
                internal_error(format!(
                    "could not create {}: {error}",
                    plan.folder.display()
                ))
            })?;
            // Taken before `clip.json` exists, so recovery never sees this
            // clip `recording` without a live recorder.
            let recorder_lock = RecorderLock::acquire(&plan.folder).map_err(internal_error)?;
            initial_clip_json(plan)
                .create(&plan.folder)
                .map_err(internal_error)?;
            ClipRegistry::add(&plan.clip_id, &plan.folder).map_err(internal_error)?;
            let observer = ClipObserver::start(plan.target, instant_hns()).map_err(|error| {
                mcp_error(
                    error.code(),
                    format!("screenshot operation=record: {error}"),
                )
            })?;
            let origin_hns = wait_for_first_frame(&observer, plan)?;
            let stream = ClipStream::create(&plan.folder).map_err(internal_error)?;
            let disk = DiskGuard::new(&plan.folder).map_err(internal_error)?;
            let mut recording = Self {
                plan,
                observer,
                stream,
                disk,
                origin_hns,
                geometry: ClipGeometry::default(),
                last_composed: None,
                next_instant: 0,
                excluded_checked_at: Instant::now(),
                times,
                _recorder_lock: recorder_lock,
            };
            recording.write_instant_zero()?;
            Ok(recording)
        }

        /// Instant zero passes the privacy gate before it is written; an
        /// all-black first frame is an error.
        fn write_instant_zero(&mut self) -> Result<(), ErrorData> {
            if let Some(reason) = self.plan.privacy_stop() {
                return Err(stop_error(reason));
            }
            let frame = self.observer.frame_for_instant(0).ok_or_else(|| {
                thread_failed("the first frame is no longer held by the observer".to_owned())
            })?;
            let pixels = self.observer.read_back(&frame).map_err(|error| {
                mcp_error(
                    error.code(),
                    format!("screenshot operation=record: {error}"),
                )
            })?;
            if pixels
                .bgra
                .chunks_exact(BGRA_PIXEL_BYTES)
                .all(|pixel| pixel[..BGRA_BLACK.len()] == BGRA_BLACK)
            {
                return Err(mcp_error(
                    error_codes::CAPTURE_TARGET_INVALID,
                    "screenshot operation=record: the first frame is all black (hidden or secure desktop, or a protected window); no PrintWindow fallback is used",
                ));
            }
            self.geometry = ClipGeometry {
                window_width: pixels.width,
                window_height: pixels.height,
                scale: PIXEL_SCALE,
                recorded_width: pixels.width * PIXEL_SCALE,
                recorded_height: pixels.height * PIXEL_SCALE,
            };
            let written = self
                .stream
                .instant(
                    0,
                    StreamInput::Picture {
                        bgra: &pixels.bgra,
                        width: pixels.width,
                        height: pixels.height,
                    },
                )
                .map_err(internal_error)?;
            self.disk.wrote(written.bytes);
            self.last_composed = Some(frame.composed_hns);
            self.next_instant = 1;
            let clock = ClipClock {
                monotonic_ms_at_s0: monotonic_ms(),
                wall_clock_unix_ms_at_s0: wall_clock_ms(),
                composed_hns_at_s0: self.origin_hns,
            };
            let geometry = self.geometry;
            let settings = self.observer.settings();
            let encoder = self.stream.encoder_info();
            let folder = &self.plan.folder;
            self.times
                .clip_json
                .time(|| {
                    ClipJson::update(folder, |clip| {
                        clip.clock = clock;
                        clip.geometry = geometry;
                        clip.observer = ObserverGrant {
                            cursor_in_picture: settings.cursor_in_picture,
                            border_hidden: settings.border_hidden,
                            secondary_windows: settings.secondary_windows,
                            requested_interval_ms: settings.requested_interval_ms,
                            dirty_regions: settings.dirty_regions,
                        };
                        clip.encoder = Some(encoder);
                        clip.instants = 1;
                        clip.recorded_ms = instant_ms(1);
                    })
                })
                .map_err(internal_error)?;
            Ok(())
        }

        /// The `record` response: `clip.json` read back from disk.
        fn response(&self) -> Result<CaptureClipResponse, ErrorData> {
            let path = self.plan.folder.join(CLIP_JSON_FILE);
            let clip = ClipJson::read(&self.plan.folder).map_err(internal_error)?;
            let (bytes, sha256) = file_size_and_sha256(&path).map_err(|error| {
                internal_error(format!("could not read back clip.json: {error}"))
            })?;
            tracing::info!(
                code = "CAPTURE_CLIP_STARTED",
                clip_id = %clip.clip_id,
                folder = %self.plan.folder.display(),
                width = clip.geometry.window_width,
                height = clip.geometry.window_height,
                "readback=screenshot_record outcome=recording"
            );
            Ok(CaptureClipResponse {
                clip_id: clip.clip_id,
                folder: self.plan.folder.to_string_lossy().into_owned(),
                state: clip.state.as_str().to_owned(),
                clip_json_path: path.to_string_lossy().into_owned(),
                bytes,
                sha256,
                frames_path: self
                    .plan
                    .folder
                    .join(FRAMES_FILE)
                    .to_string_lossy()
                    .into_owned(),
                end_ms: clip.end_ms,
                fps: clip.fps,
                window_width: clip.geometry.window_width,
                window_height: clip.geometry.window_height,
                scale: clip.geometry.scale,
                recorded_width: clip.geometry.recorded_width,
                recorded_height: clip.geometry.recorded_height,
                cursor_in_picture: clip.observer.cursor_in_picture,
                border_hidden: clip.observer.border_hidden,
            })
        }

        /// One instant every 1 / f until a stop. Returns why it stopped.
        fn run(&mut self, control: &RecordingControl) -> StopReason {
            control
                .recorded_ms
                .store(instant_ms(self.next_instant), Ordering::Relaxed);
            loop {
                let k = self.next_instant;
                if control.stop.load(Ordering::Relaxed) {
                    return StopReason::StopCall;
                }
                if instant_ms(k) >= control.end_ms.load(Ordering::Relaxed) {
                    return StopReason::Completed;
                }
                // Instant k is closed once s_{k+1} begins: every frame composed
                // at or before s_k has then arrived.
                sleep_until(self.origin_hns + (to_i64(k) + 1) * instant_hns(), control);
                if control.stop.load(Ordering::Relaxed) {
                    return StopReason::StopCall;
                }
                if let Some(reason) = self.stop_condition() {
                    return reason;
                }
                if let Err(reason) = self.handle_instant(k) {
                    return reason;
                }
                self.next_instant = k + 1;
                // `record_status` reads the live length from here (no disk write).
                control
                    .recorded_ms
                    .store(instant_ms(self.next_instant), Ordering::Relaxed);
            }
        }

        /// The target closed, the privacy gate pauses, an excluded app shows on
        /// the monitor, or the disk rule stops the clip.
        fn stop_condition(&mut self) -> Option<StopReason> {
            if self.observer.closed() || self.window_closed() {
                return Some(StopReason::TargetLost);
            }
            if let Some(reason) = self.plan.privacy_stop() {
                return Some(reason);
            }
            if self.excluded_checked_at.elapsed() >= Duration::from_millis(EXCLUDED_APP_CHECK_MS) {
                self.excluded_checked_at = Instant::now();
                if self.plan.excluded_app_visible() {
                    return Some(StopReason::PrivacyExcludedApp);
                }
            }
            match self.disk.permits(&self.plan.folder) {
                Ok(true) => None,
                Ok(false) => Some(StopReason::DiskPressure),
                Err(error) => {
                    tracing::warn!(code = "CAPTURE_CLIP_DISK_CHECK_FAILED", error = %error);
                    Some(StopReason::WriteFailed)
                }
            }
        }

        /// Instant k: the frame composed in it is read back and handed to the
        /// stream; a frame already handed over is `Unchanged`; a frame the
        /// observer dropped (no free slot) or could not read is a named loss.
        fn handle_instant(&mut self, k: u64) -> Result<(), StopReason> {
            let frame = self.observer.frame_for_instant(to_i64(k));
            let overflowed = self
                .observer
                .stats()
                .slot_overflow_instants
                .contains(&to_i64(k));
            let frame = match frame {
                Some(frame) if !overflowed => frame,
                _ => {
                    self.last_composed = None;
                    return self.feed(k, StreamInput::Lost(LOST_SLOT_OVERFLOW));
                }
            };
            if self.last_composed == Some(frame.composed_hns) {
                return self.feed(k, StreamInput::Unchanged);
            }
            let pixels = match self.observer.read_back(&frame) {
                Ok(pixels) => pixels,
                Err(error) => {
                    tracing::warn!(code = "CAPTURE_CLIP_READBACK_FAILED", error = %error);
                    self.last_composed = None;
                    self.feed(k, StreamInput::Lost(LOST_CAPTURE_FAILED))?;
                    return Err(StopReason::CaptureFailed);
                }
            };
            self.feed(
                k,
                StreamInput::Picture {
                    bgra: &pixels.bgra,
                    width: pixels.width,
                    height: pixels.height,
                },
            )?;
            self.last_composed = Some(frame.composed_hns);
            Ok(())
        }

        /// Hands instant k to the stream; a closed video part is published
        /// in `clip.json` at once.
        fn feed(&mut self, k: u64, input: StreamInput) -> Result<(), StopReason> {
            let written = self.stream.instant(k, input).map_err(|error| {
                tracing::warn!(code = "CAPTURE_CLIP_WRITE_FAILED", instant = k, error = %error);
                StopReason::WriteFailed
            })?;
            self.disk.wrote(written.bytes);
            if !written.closed_parts.is_empty() {
                let parts = written.closed_parts;
                let folder = &self.plan.folder;
                self.times
                    .clip_json
                    .time(|| ClipJson::update(folder, |clip| clip.parts.extend(parts)))
                    .map_err(|error| {
                        tracing::warn!(code = "CAPTURE_CLIP_WRITE_FAILED", error = %error);
                        StopReason::WriteFailed
                    })?;
            }
            Ok(())
        }

        fn window_closed(&self) -> bool {
            match self.plan.target {
                // SAFETY: IsWindow accepts any handle value.
                ClipSource::Window { hwnd } => !unsafe {
                    windows::Win32::UI::WindowsAndMessaging::IsWindow(Some(window_handle(hwnd)))
                }
                .as_bool(),
                ClipSource::Monitor { .. } => false,
            }
        }

        /// Ends the stream (waiting index lines, last part) and publishes the
        /// clip `ready` with every count of plan §3, then logs what each stage
        /// cost (`CLIP_RECORD_TIMING`).
        fn finish(mut self, reason: StopReason) {
            let folder = &self.plan.folder;
            let summary = match self.stream.finish() {
                Ok(summary) => summary,
                Err(error) => {
                    tracing::warn!(code = "CAPTURE_CLIP_FINISH_FAILED", error = %error);
                    let _ = self.times.clip_json.time(|| {
                        ClipJson::update(folder, |clip| {
                            clip.state = ClipState::Failed;
                            clip.failure = Some(error);
                        })
                    });
                    let stats = self.observer.stop();
                    // The stream's own totals ended with it; the observer's and the
                    // process's are still told.
                    self.times.log(
                        &self.plan.clip_id,
                        &stats,
                        self.next_instant,
                        &StreamTimes::default(),
                    );
                    return;
                }
            };
            let stats = self.observer.stop();
            let geometry = self.geometry;
            let recorded_ms = instant_ms(summary.instants);
            let (instants, stream_times) = (summary.instants, summary.times);
            let reference_hns = HNS_PER_SECOND / i64::from(REFERENCE_REFRESH_HZ);
            let updated = self.times.clip_json.time(|| {
                ClipJson::update(folder, |clip| -> Result<(), String> {
                    clip.parts.extend(summary.parts);
                    clip.stills = summary.stills;
                    clip.encoder = Some(summary.encoder);
                    clip.instants = summary.instants;
                    clip.recorded_ms = recorded_ms;
                    clip.stop_reason = Some(reason);
                    clip.conditions = Conditions {
                        capture: Some(ConditionStatus::held(serde_json::json!({
                            "max_hold_ms": stats.max_hold_hns as f64 / HNS_PER_MS as f64,
                            "reference_period_ms": reference_hns as f64 / HNS_PER_MS as f64,
                            "hold_within_reference": stats.max_hold_hns <= reference_hns,
                            "frames_delivered": stats.frames_delivered
                        }))),
                        geometry: Some(ConditionStatus::held(serde_json::json!({
                            "window_width": geometry.window_width,
                            "window_height": geometry.window_height,
                            "scale": PIXEL_SCALE
                        }))),
                        quality: Some(ConditionStatus::held(serde_json::json!({
                            "encoder": clip.encoder.as_ref().map(|encoder| encoder.name.clone()),
                            "parts": clip.parts.len(),
                            "stills": clip.stills.len(),
                            "settle_instants": SETTLE_INSTANTS
                        }))),
                        totals: Some(ConditionStatus::held(serde_json::json!({
                            "length_ms": recorded_ms,
                            "instants": summary.instants,
                            "encoded": summary.encoded,
                            "same": summary.same,
                            "lost": summary.lost.len(),
                            "stop_reason": reason.as_str()
                        }))),
                    };
                    clip.add_lost(summary.lost);
                    if clip.state == ClipState::Recording {
                        clip.state = ClipState::Ready;
                    }
                    clip.set_total_bytes(folder)
                })
                .and_then(|(_, result)| result)
            });
            if let Err(error) = updated {
                tracing::warn!(code = "CAPTURE_CLIP_FINISH_FAILED", error = %error);
                let _ = self.times.clip_json.time(|| {
                    ClipJson::update(folder, |clip| {
                        clip.state = ClipState::Failed;
                        clip.failure = Some(error);
                    })
                });
            }
            self.times
                .log(&self.plan.clip_id, &stats, instants, &stream_times);
        }
    }

    /// `clip.json` at start: state `recording`.
    fn initial_clip_json(plan: &ClipPlan) -> ClipJson {
        ClipJson {
            schema_version: CLIP_SCHEMA_VERSION,
            method: CLIP_METHOD.to_owned(),
            clip_id: plan.clip_id.clone(),
            synapse_version: env!("CARGO_PKG_VERSION").to_owned(),
            state: ClipState::Recording,
            failure: None,
            stop_reason: None,
            fps: RECORD_FPS,
            end_ms: plan.end_ms,
            recorded_ms: 0,
            instants: 0,
            target: target_info(plan.target),
            clock: ClipClock::default(),
            geometry: ClipGeometry::default(),
            observer: ObserverGrant::default(),
            encoder: None,
            parts: Vec::new(),
            stills: Vec::new(),
            lost: Vec::new(),
            bytes: 0,
            conditions: Conditions::default(),
        }
    }

    fn target_info(target: ClipSource) -> ClipTargetInfo {
        match target {
            ClipSource::Window { hwnd } => {
                let context = synapse_a11y::foreground_context(hwnd).ok();
                ClipTargetInfo {
                    kind: "window".to_owned(),
                    hwnd: Some(hwnd),
                    monitor_index: context.as_ref().map(|c| c.monitor_index),
                    process_id: context.as_ref().map(|c| c.pid),
                    exe: context.as_ref().map(|c| c.process_path.clone()),
                    title: context.map(|c| c.window_title),
                }
            }
            ClipSource::Monitor { monitor_index } => ClipTargetInfo {
                kind: "monitor".to_owned(),
                monitor_index: Some(monitor_index),
                ..ClipTargetInfo::default()
            },
        }
    }

    /// Waits for the first delivered frame; it is s_0.
    fn wait_for_first_frame(observer: &ClipObserver, plan: &ClipPlan) -> Result<i64, ErrorData> {
        let deadline = Instant::now() + Duration::from_millis(FIRST_FRAME_TIMEOUT_MS);
        while Instant::now() < deadline {
            if let Some(origin) = observer.origin() {
                return Ok(origin);
            }
            if observer.closed() {
                return Err(stop_error(StopReason::TargetLost));
            }
            std::thread::sleep(Duration::from_millis(instant_ms(1)));
        }
        if let ClipSource::Window { hwnd } = plan.target
            // SAFETY: IsWindow accepts any handle value.
            && !unsafe { windows::Win32::UI::WindowsAndMessaging::IsWindow(Some(window_handle(hwnd))) }
                .as_bool()
        {
            return Err(stop_error(StopReason::TargetLost));
        }
        Err(mcp_error(
            error_codes::CAPTURE_TARGET_INVALID,
            format!(
                "screenshot operation=record got no frame within {FIRST_FRAME_TIMEOUT_MS} ms; a minimized or non-rendering target cannot start a clip (it is never restored)"
            ),
        ))
    }

    /// Sleeps until the QPC time `deadline_hns`, waking early on a stop call.
    fn sleep_until(deadline_hns: i64, control: &RecordingControl) {
        loop {
            let left = deadline_hns - synapse_capture::now_hns();
            if left <= 0 || control.stop.load(Ordering::Relaxed) {
                return;
            }
            let left_ns = u64::try_from(left.min(instant_hns())).unwrap_or(0) * 100;
            std::thread::sleep(Duration::from_nanos(left_ns));
        }
    }

    fn to_i64(value: u64) -> i64 {
        i64::try_from(value).unwrap_or(i64::MAX)
    }

    fn window_handle(hwnd: i64) -> windows::Win32::Foundation::HWND {
        windows::Win32::Foundation::HWND(hwnd as *mut core::ffi::c_void)
    }

    fn monotonic_ms() -> u64 {
        // SAFETY: no arguments.
        unsafe { windows::Win32::System::SystemInformation::GetTickCount64() }
    }

    fn wall_clock_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| {
                u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
            })
    }
}
