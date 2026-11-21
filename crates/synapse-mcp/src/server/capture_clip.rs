//! `screenshot operation=record`: the bug-clip recorder ("Bug clip video"
//! research, order of work step 2: recording on request).
//!
//! One synchronous call records one window or one monitor for `duration_ms`
//! into a clip folder:
//!
//! - `clip.mp4`: H.264 at a constant frame rate (`ClipEncoder`).
//! - `frames.jsonl`: one line per encoded frame
//!   (`{"frame","t_ms","new","key"}`).
//! - `clip.json`: identity, clock, target, encode settings actually granted,
//!   and health (`ClipManifest`).
//!
//! Frames come from the existing `synapse-capture` loop (Windows.Graphics.
//! Capture, cursor on). The recorder owns the timeline: a ticker at
//! `1000 / fps` ms encodes the latest frame on every tick and repeats the last
//! one when Windows delivered nothing new, recording per frame which it was.
//! The frame size is fixed at clip start (even dimensions); later frames of a
//! different size are scaled and letterboxed into it. A minimized window
//! delivers no frames: those ticks repeat and form `target_minimized`
//! intervals. The recorder never restores or focuses a window.
//!
//! Privacy: the timeline recorder gate (`RecorderControl::suppress_reason`,
//! the gate the activity recorder uses) is checked before start and on every
//! tick. Paused or excluded before start: refused. During recording: the clip
//! stops, keeps what it has, and records the reason. A closed window stops
//! the clip the same way (`target_lost`). Disk-pressure level 3/4 refuses.
//!
//! The clip is written into a hidden temp folder next to the target folder and
//! renamed into place at the end; on error the temp folder is removed. The
//! capture loop (this call's own `CaptureHandle`, never the shared M1
//! controller) is always stopped.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use rmcp::{RoleServer, service::RequestContext};
use serde::Serialize;
use serde_json::json;
use synapse_core::error_codes;

use super::{CaptureClipParams, CaptureClipResponse, ErrorData, SynapseService};
use crate::m1::mcp_error;
use crate::m3::timeline_control::{RecorderControl, SuppressReason};

pub(super) const MIN_DURATION_MS: u64 = 100;
pub(super) const MAX_DURATION_MS: u64 = 120_000;
pub(super) const DEFAULT_FPS: u32 = 20;
pub(super) const MAX_FPS: u32 = 30;
/// How long to wait for the first frame, which fixes the clip size.
const FIRST_FRAME_TIMEOUT_MS: u64 = 3_000;
pub(super) const CLIP_SCHEMA_VERSION: u32 = 1;
pub(super) const VIDEO_FILE: &str = "clip.mp4";
pub(super) const FRAMES_FILE: &str = "frames.jsonl";
pub(super) const CLIP_JSON_FILE: &str = "clip.json";
const CLIP_SOURCE_OF_TRUTH: &str =
    "clip folder files (clip.mp4, frames.jsonl, clip.json) + target metadata";

/// What to record.
#[derive(Clone, Copy, Debug)]
enum ClipTarget {
    Window { hwnd: i64 },
    Monitor { monitor_index: u32 },
}

impl ClipTarget {
    fn id(self) -> String {
        match self {
            Self::Window { hwnd } => format!("window:{hwnd:#x}"),
            Self::Monitor { monitor_index } => format!("monitor:{monitor_index}"),
        }
    }
}

/// Why the recording ended.
#[derive(Clone, Copy, Debug, Default, Serialize)]
#[serde(rename_all = "snake_case")]
enum StopReason {
    #[default]
    Completed,
    TargetLost,
    PrivacyPaused,
    PrivacyExcludedApp,
}

impl StopReason {
    const fn from_privacy(reason: SuppressReason) -> Self {
        match reason {
            SuppressReason::Paused => Self::PrivacyPaused,
            SuppressReason::ExcludedApp => Self::PrivacyExcludedApp,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::TargetLost => "target_lost",
            Self::PrivacyPaused => "privacy_paused",
            Self::PrivacyExcludedApp => "privacy_excluded_app",
        }
    }
}

/// A validated recording request.
struct ClipPlan {
    clip_id: String,
    target: ClipTarget,
    /// The window's process name for the privacy gate; monitors read the
    /// foreground app on every tick instead.
    app: Option<String>,
    duration_ms: u64,
    fps: u32,
    folder: PathBuf,
    privacy: Arc<RecorderControl>,
}

impl ClipPlan {
    /// The privacy gate's verdict for what the clip shows right now.
    fn privacy_stop(&self) -> Option<StopReason> {
        let foreground_app;
        let app = match self.target {
            ClipTarget::Window { .. } => self.app.as_deref(),
            ClipTarget::Monitor { .. } => {
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
}

impl SynapseService {
    /// Records a clip folder. Parameters are bounds-checked by the
    /// `screenshot` facade before this runs. The recording runs on its own
    /// named thread, never on a tokio worker; this call only awaits its result.
    pub(super) async fn capture_clip(
        &self,
        params: CaptureClipParams,
        request_context: &RequestContext<RoleServer>,
    ) -> Result<CaptureClipResponse, ErrorData> {
        let target = self
            .capture_clip_target(&params, request_context)
            .map_err(|error| with_clip_context(error, "session_target"))?;
        let target_id = target.id();
        let plan = self
            .capture_clip_plan(target, &params)
            .map_err(|error| with_clip_context(error, &target_id))?;
        let (sender, receiver) = tokio::sync::oneshot::channel();
        std::thread::Builder::new()
            .name("synapse-clip-recorder".to_owned())
            .spawn(move || {
                let _ = sender.send(record_clip(&plan));
            })
            .map_err(|error| thread_failed(format!("could not start the recorder thread: {error}")))
            .map_err(|error| with_clip_context(error, &target_id))?;
        receiver
            .await
            .unwrap_or_else(|_| {
                Err(thread_failed(
                    "the recorder thread ended without a result (panicked)".to_owned(),
                ))
            })
            .map_err(|error| with_clip_context(error, &target_id))
    }

    /// An explicit monitor or window wins; otherwise the session's window
    /// target, resolved like `screenshot operation=capture`.
    fn capture_clip_target(
        &self,
        params: &CaptureClipParams,
        request_context: &RequestContext<RoleServer>,
    ) -> Result<ClipTarget, ErrorData> {
        if let Some(monitor_index) = params.monitor_index {
            return Ok(ClipTarget::Monitor { monitor_index });
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
        Ok(ClipTarget::Window { hwnd })
    }

    /// Applies the start gates (disk pressure, privacy) and fixes the folder.
    fn capture_clip_plan(
        &self,
        target: ClipTarget,
        params: &CaptureClipParams,
    ) -> Result<ClipPlan, ErrorData> {
        let (db, privacy) = {
            let m3_state = self.m3_state_handle();
            let mut guard = m3_state.lock().map_err(|_poisoned| {
                mcp_error(
                    error_codes::TOOL_INTERNAL_ERROR,
                    "M3 service state lock poisoned",
                )
            })?;
            let db = guard.ensure_storage().map_err(|error| {
                mcp_error(
                    error_codes::TOOL_INTERNAL_ERROR,
                    format!("open storage for the clip recorder gates: {error}"),
                )
            })?;
            let privacy = guard.ensure_recorder_control().map_err(|error| {
                mcp_error(error_codes::TOOL_INTERNAL_ERROR, format!("{error:#}"))
            })?;
            (db, privacy)
        };
        let pressure_code = match db.pressure_level() {
            synapse_storage::DiskPressureLevel::Level3 => {
                Some(error_codes::STORAGE_DISK_PRESSURE_LEVEL_3)
            }
            synapse_storage::DiskPressureLevel::Level4 => {
                Some(error_codes::STORAGE_DISK_PRESSURE_LEVEL_4)
            }
            _ => None,
        };
        if let Some(code) = pressure_code {
            return Err(mcp_error(
                code,
                format!("screenshot operation=record refused: disk pressure is critical ({code})"),
            ));
        }

        let clip_id = uuid::Uuid::new_v4().to_string();
        let app = match target {
            ClipTarget::Window { hwnd } => synapse_a11y::foreground_context(hwnd)
                .ok()
                .map(|context| context.process_name),
            ClipTarget::Monitor { .. } => None,
        };
        let plan = ClipPlan {
            folder: params
                .folder
                .as_ref()
                .map_or_else(|| clips_root().join(&clip_id), PathBuf::from),
            clip_id,
            target,
            app,
            duration_ms: params.duration_ms,
            fps: params.fps.unwrap_or(DEFAULT_FPS),
            privacy,
        };
        if let Some(reason) = plan.privacy_stop() {
            return Err(mcp_error(
                error_codes::CAPTURE_TARGET_INVALID,
                format!(
                    "screenshot operation=record refused by the timeline privacy gate: {}",
                    reason.as_str()
                ),
            ));
        }
        Ok(plan)
    }
}

/// Default parent of clip folders (same root as `replay_root()`).
fn clips_root() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map_or_else(std::env::temp_dir, PathBuf::from)
        .join("synapse")
        .join("clips")
}

/// Records into a temp folder next to `plan.folder`, then renames it into place.
fn record_clip(plan: &ClipPlan) -> Result<CaptureClipResponse, ErrorData> {
    let temp = temp_folder_for(&plan.folder)?;
    std::fs::create_dir_all(&temp).map_err(|error| {
        internal_error(format!(
            "could not create temp clip folder {temp:?}: {error}"
        ))
    })?;
    let recorded = record_into(plan, &temp).and_then(|summary| {
        std::fs::rename(&temp, &plan.folder).map_err(|error| {
            internal_error(format!(
                "could not move clip into {:?}: {error}",
                plan.folder
            ))
        })?;
        Ok(summary)
    });
    let summary = match recorded {
        Ok(summary) => summary,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&temp);
            return Err(error);
        }
    };
    let video_path = plan.folder.join(VIDEO_FILE);
    let bytes_written = std::fs::metadata(&video_path).map_or(0, |m| m.len());
    let sha256 = file_sha256(&video_path)
        .map_err(|error| internal_error(format!("could not hash clip.mp4: {error}")))?;
    tracing::info!(
        code = "CAPTURE_CLIP_RECORDED",
        folder = %plan.folder.display(),
        frames_new = summary.health.frames_new,
        frames_repeated = summary.health.frames_repeated,
        encoder = %summary.encode.encoder,
        hardware = summary.encode.hardware,
        stopped_reason = summary.health.stopped_reason.as_str(),
        bytes_written,
        "readback=screenshot_record outcome=encoded"
    );
    Ok(CaptureClipResponse {
        clip_id: summary.clip_id,
        folder: path_string(&plan.folder),
        video_path: path_string(&video_path),
        frames_path: path_string(&plan.folder.join(FRAMES_FILE)),
        clip_json_path: path_string(&plan.folder.join(CLIP_JSON_FILE)),
        bytes: bytes_written,
        sha256,
        duration_ms: summary.health.duration_ms,
        width: summary.encode.width,
        height: summary.encode.height,
        fps: summary.encode.fps,
        frames_total: summary.health.frames_total,
        frames_new: summary.health.frames_new,
        frames_repeated: summary.health.frames_repeated,
        encoder: summary.encode.encoder,
        hardware: summary.encode.hardware,
        container: summary.encode.container.to_owned(),
        stopped_reason: summary.health.stopped_reason.as_str().to_owned(),
    })
}

fn temp_folder_for(folder: &Path) -> Result<PathBuf, ErrorData> {
    let parent = folder
        .parent()
        .ok_or_else(|| internal_error(format!("clip folder {folder:?} has no parent directory")))?;
    let name = folder
        .file_name()
        .map_or_else(|| "clip".into(), |name| name.to_string_lossy());
    Ok(parent.join(format!(
        ".{name}.recording-{}",
        uuid::Uuid::new_v4().simple()
    )))
}

fn file_sha256(path: &Path) -> std::io::Result<String> {
    use sha2::Digest as _;
    use std::io::Read as _;

    let mut file = std::fs::File::open(path)?;
    let mut hasher = sha2::Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

#[cfg(windows)]
fn record_into(plan: &ClipPlan, temp: &Path) -> Result<ClipManifest, ErrorData> {
    let capture = synapse_capture::spawn_capture_loop(synapse_capture::CaptureConfig {
        target: match plan.target {
            ClipTarget::Window { hwnd } => synapse_capture::CaptureTarget::Window { hwnd },
            ClipTarget::Monitor { monitor_index } => {
                synapse_capture::CaptureTarget::Monitor { monitor_index }
            }
        },
        min_update_interval_ms: 1_000 / u64::from(plan.fps),
        cursor_visible: true,
        // Include menus and popups owned by the window: they matter in bug clips.
        secondary_windows: true,
        dirty_region_only: false,
        // Pinned: SYNAPSE_CAPTURE_FORCE_DXGI must not break window clips.
        backend_preference: synapse_capture::CaptureBackendPreference::GraphicsCaptureApi,
    })
    .map_err(|error| {
        mcp_error(
            error.code(),
            format!("screenshot operation=record: {error}"),
        )
    })?;
    let recorded = ClipRecorder::new(plan).run(&capture, temp);
    if let Err(error) = capture.stop() {
        tracing::warn!(
            code = "CAPTURE_CLIP_STOP_FAILED",
            error = %error,
            "capture loop reported an error while stopping"
        );
    }
    let manifest = recorded?;
    let json = serde_json::to_vec_pretty(&manifest)
        .map_err(|error| internal_error(format!("could not serialise clip.json: {error}")))?;
    std::fs::write(temp.join(CLIP_JSON_FILE), json)
        .map_err(|error| internal_error(format!("could not write clip.json: {error}")))?;
    Ok(manifest)
}

#[cfg(not(windows))]
fn record_into(_plan: &ClipPlan, _temp: &Path) -> Result<ClipManifest, ErrorData> {
    Err(mcp_error(
        error_codes::CAPTURE_GRAPHICS_API_UNSUPPORTED,
        "screenshot operation=record requires Windows (Windows.Graphics.Capture + Media Foundation)",
    ))
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
            "Windows.Graphics.Capture is unavailable for this target; record a monitor or another window"
        }
        error_codes::CAPTURE_TARGET_INVALID => {
            "bind a live, visible window (or pass a valid monitor_index), check the timeline privacy gate, and retry"
        }
        error_codes::STORAGE_DISK_PRESSURE_LEVEL_3 | error_codes::STORAGE_DISK_PRESSURE_LEVEL_4 => {
            "free disk space on the Synapse data volume, then retry"
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

/// Runs the constant-rate timeline and writes `clip.mp4` + `frames.jsonl`.
#[cfg(windows)]
struct ClipRecorder<'a> {
    plan: &'a ClipPlan,
    health: ClipHealth,
    minimized_since: Option<(u64, u64)>,
    source_size: (u32, u32),
}

#[cfg(windows)]
impl<'a> ClipRecorder<'a> {
    fn new(plan: &'a ClipPlan) -> Self {
        Self {
            plan,
            health: ClipHealth::default(),
            minimized_since: None,
            source_size: (0, 0),
        }
    }

    fn run(
        mut self,
        capture: &synapse_capture::CaptureHandle,
        temp: &Path,
    ) -> Result<ClipManifest, ErrorData> {
        use std::io::Write as _;
        use std::time::{Duration, Instant};

        let fps = self.plan.fps;
        let receiver = capture.receiver();
        let first = receiver
            .recv_timeout(Duration::from_millis(FIRST_FRAME_TIMEOUT_MS))
            .map_err(|_| {
                if self.target_closed() {
                    return mcp_error(
                        error_codes::CAPTURE_TARGET_LOST,
                        "screenshot operation=record: the target window closed before the first frame",
                    );
                }
                mcp_error(
                    error_codes::CAPTURE_TARGET_INVALID,
                    format!(
                        "screenshot operation=record got no frame within {FIRST_FRAME_TIMEOUT_MS} ms; a minimized or non-rendering target cannot start a clip (it is never restored)"
                    ),
                )
            })?;
        let started = Instant::now();
        let clock = ClipClock::now();
        let first = frame_to_bgra(&first)?;
        if is_all_black(&first.bytes) {
            return Err(mcp_error(
                error_codes::CAPTURE_TARGET_INVALID,
                "screenshot operation=record: the first frame is all black (hidden or secure desktop, or a protected window); no PrintWindow fallback is used",
            ));
        }
        self.source_size = (first.width, first.height);
        let mut canvas = ClipCanvas::new(first.width, first.height);
        canvas.draw(&first.bytes, first.width, first.height);

        let mut encoder = super::clip_encoder::ClipEncoder::create(
            &temp.join(VIDEO_FILE),
            canvas.width,
            canvas.height,
            fps,
        )
        .map_err(|error| internal_error(format!("H.264 encoder setup failed: {error}")))?;
        let keyframe_interval = encoder.readback().keyframe_interval_frames.max(1);
        let frames_file = std::fs::File::create(temp.join(FRAMES_FILE))
            .map_err(|error| internal_error(format!("could not create frames.jsonl: {error}")))?;
        let mut frames_out = std::io::BufWriter::new(frames_file);

        let planned_frames = (self.plan.duration_ms * u64::from(fps))
            .div_ceil(1_000)
            .max(1);
        let tick = Duration::from_secs(1) / fps;
        let mut total_frames = 0;
        for frame in 0..planned_frames {
            let t_ms = frame * 1_000 / u64::from(fps);
            let is_new = if frame == 0 {
                true
            } else {
                let deadline = started + tick * u32::try_from(frame).unwrap_or(u32::MAX);
                if let Some(wait) = deadline.checked_duration_since(Instant::now()) {
                    std::thread::sleep(wait);
                }
                if let Some(reason) = self.early_stop() {
                    self.health.stopped_reason = reason;
                    break;
                }
                self.take_latest(capture, &mut canvas, frame, t_ms)
            };
            if is_new {
                self.health.frames_new += 1;
            } else {
                self.health.frames_repeated += 1;
            }
            encoder
                .write_nv12(&canvas.nv12)
                .map_err(|error| internal_error(format!("H.264 encode failed: {error}")))?;
            let line = FrameLine {
                frame,
                t_ms,
                new: is_new,
                key: frame % u64::from(keyframe_interval) == 0,
            };
            serde_json::to_writer(&mut frames_out, &line)
                .map_err(|error| internal_error(format!("frames.jsonl write failed: {error}")))?;
            frames_out
                .write_all(b"\n")
                .map_err(|error| internal_error(format!("frames.jsonl write failed: {error}")))?;
            total_frames = frame + 1;
        }
        let end_ms = total_frames * 1_000 / u64::from(fps);
        self.close_minimized(total_frames, end_ms);
        frames_out
            .flush()
            .map_err(|error| internal_error(format!("frames.jsonl flush failed: {error}")))?;
        let readback = encoder
            .finish()
            .map_err(|error| internal_error(format!("H.264 finalize failed: {error}")))?;

        let wall_elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.health.frames_total = total_frames;
        self.health.frames_dropped_by_channel = capture.stats().frames_dropped();
        self.health.requested_duration_ms = self.plan.duration_ms;
        self.health.duration_ms = end_ms;
        self.health.wall_elapsed_ms = wall_elapsed_ms;
        let seconds = (wall_elapsed_ms.max(1) as f64) / 1_000.0;
        self.health.achieved_fps = round2(total_frames as f64 / seconds);
        self.health.new_frame_fps = round2(self.health.frames_new as f64 / seconds);

        Ok(ClipManifest {
            schema_version: CLIP_SCHEMA_VERSION,
            clip_id: self.plan.clip_id.clone(),
            synapse_version: env!("CARGO_PKG_VERSION"),
            clock,
            target: ClipTargetInfo::read(self.plan.target, first.width, first.height),
            encode: ClipEncode {
                codec: "h264",
                profile: "high",
                pixel_format: "yuv420p (nv12)",
                encoder: readback.encoder,
                hardware: readback.hardware,
                container: readback.container,
                width: canvas.width,
                height: canvas.height,
                fps,
                keyframe_interval_frames: readback.keyframe_interval_frames,
                keyframe_interval_ms: u64::from(readback.keyframe_interval_frames) * 1_000
                    / u64::from(fps),
                rate_control: RateControl {
                    mode: readback.rate_control_mode,
                    value: readback.rate_control_value,
                },
                cursor: true,
                resize: "letterbox",
            },
            health: self.health,
        })
    }

    /// Drains the capture channel, draws the newest frame and reports whether
    /// one arrived. With none, notes whether the window is minimized.
    fn take_latest(
        &mut self,
        capture: &synapse_capture::CaptureHandle,
        canvas: &mut ClipCanvas,
        frame: u64,
        t_ms: u64,
    ) -> bool {
        let receiver = capture.receiver();
        let mut latest = None;
        while let Ok(captured) = receiver.try_recv() {
            if latest.is_some() {
                self.health.frames_skipped += 1;
            }
            latest = Some(captured);
        }
        let bitmap = latest.and_then(|captured| match frame_to_bgra(&captured) {
            Ok(bitmap) => Some(bitmap),
            Err(error) => {
                self.health.frames_skipped += 1;
                tracing::warn!(code = "CAPTURE_CLIP_FRAME_DROPPED", error = %error.message);
                None
            }
        });
        let Some(bitmap) = bitmap else {
            if self.target_minimized() {
                self.minimized_since.get_or_insert((frame, t_ms));
            } else {
                self.close_minimized(frame, t_ms);
            }
            return false;
        };
        self.close_minimized(frame, t_ms);
        if (bitmap.width, bitmap.height) != self.source_size {
            self.source_size = (bitmap.width, bitmap.height);
            self.health.resizes.push(ClipResize {
                frame,
                t_ms,
                width: bitmap.width,
                height: bitmap.height,
            });
        }
        canvas.draw(&bitmap.bytes, bitmap.width, bitmap.height);
        true
    }

    /// Why the clip must stop before this tick, if it must: the target window
    /// closed, or the privacy gate now suppresses recording.
    fn early_stop(&self) -> Option<StopReason> {
        if self.target_closed() {
            return Some(StopReason::TargetLost);
        }
        self.plan.privacy_stop()
    }

    fn target_closed(&self) -> bool {
        match self.plan.target {
            ClipTarget::Window { hwnd } => !unsafe {
                windows::Win32::UI::WindowsAndMessaging::IsWindow(Some(window_handle(hwnd)))
            }
            .as_bool(),
            ClipTarget::Monitor { .. } => false,
        }
    }

    fn target_minimized(&self) -> bool {
        match self.plan.target {
            ClipTarget::Window { hwnd } => unsafe {
                windows::Win32::UI::WindowsAndMessaging::IsIconic(window_handle(hwnd)).as_bool()
            },
            ClipTarget::Monitor { .. } => false,
        }
    }

    fn close_minimized(&mut self, end_frame: u64, end_ms: u64) {
        if let Some((start_frame, start_ms)) = self.minimized_since.take() {
            self.health.minimized_intervals.push(MinimizedInterval {
                start_frame,
                end_frame,
                start_ms,
                end_ms,
            });
        }
    }
}

#[cfg(windows)]
fn window_handle(hwnd: i64) -> windows::Win32::Foundation::HWND {
    windows::Win32::Foundation::HWND(hwnd as *mut core::ffi::c_void)
}

/// `true` when every pixel of a BGRA frame is black (alpha ignored).
#[cfg(windows)]
fn is_all_black(bgra: &[u8]) -> bool {
    bgra.chunks_exact(4).all(|pixel| pixel[..3] == [0, 0, 0])
}

#[cfg(windows)]
fn frame_to_bgra(
    frame: &synapse_capture::CapturedFrame,
) -> Result<synapse_capture::CapturedBgraBitmap, ErrorData> {
    let region = synapse_core::Rect {
        x: 0,
        y: 0,
        w: i32::try_from(frame.width).unwrap_or(i32::MAX),
        h: i32::try_from(frame.height).unwrap_or(i32::MAX),
    };
    synapse_capture::captured_frame_region_to_bgra_bitmap(frame, region)
        .map_err(|error| mcp_error(error.code(), format!("frame conversion failed: {error}")))
}

/// The fixed-size picture the encoder sees, kept as NV12 (BT.709 limited range).
#[cfg(windows)]
struct ClipCanvas {
    width: u32,
    height: u32,
    nv12: Vec<u8>,
}

#[cfg(windows)]
impl ClipCanvas {
    /// Fixes the clip size from the first frame, rounded down to even numbers.
    fn new(first_width: u32, first_height: u32) -> Self {
        let width = (first_width & !1).max(2);
        let height = (first_height & !1).max(2);
        let pixels = width as usize * height as usize;
        Self {
            width,
            height,
            nv12: vec![0; pixels * 3 / 2],
        }
    }

    /// Draws a BGRA frame: as is when it matches (an odd extra row or column is
    /// cropped), otherwise scaled to fit and letterboxed with black bars.
    fn draw(&mut self, bgra: &[u8], width: u32, height: u32) {
        let fits = width >= self.width
            && height >= self.height
            && width - self.width <= 1
            && height - self.height <= 1;
        if fits {
            self.convert(bgra, width as usize);
            return;
        }
        let letterboxed = self.letterbox(bgra, width, height);
        self.convert(&letterboxed, self.width as usize);
    }

    fn letterbox(&self, bgra: &[u8], width: u32, height: u32) -> Vec<u8> {
        let (canvas_w, canvas_h) = (self.width as usize, self.height as usize);
        let mut out = [0_u8, 0, 0, 255].repeat(canvas_w * canvas_h);
        let Some(source) = image::RgbaImage::from_raw(
            width,
            height,
            bgra.get(..width as usize * height as usize * 4)
                .unwrap_or_default()
                .to_vec(),
        ) else {
            return out;
        };
        let scale = f64::min(
            f64::from(self.width) / f64::from(width.max(1)),
            f64::from(self.height) / f64::from(height.max(1)),
        );
        let fit_w = ((f64::from(width) * scale).round() as u32).clamp(1, self.width);
        let fit_h = ((f64::from(height) * scale).round() as u32).clamp(1, self.height);
        // Channel order is irrelevant to resizing, so BGRA passes through as is.
        let scaled =
            image::imageops::resize(&source, fit_w, fit_h, image::imageops::FilterType::Triangle);
        let x0 = (canvas_w - fit_w as usize) / 2;
        let y0 = (canvas_h - fit_h as usize) / 2;
        let row_bytes = fit_w as usize * 4;
        for (row, line) in scaled.as_raw().chunks_exact(row_bytes).enumerate() {
            let start = ((y0 + row) * canvas_w + x0) * 4;
            out[start..start + row_bytes].copy_from_slice(line);
        }
        out
    }

    /// BGRA (`stride_px` pixels per row) to NV12 over the canvas area,
    /// BT.709 limited range, chroma averaged over each 2x2 block.
    fn convert(&mut self, bgra: &[u8], stride_px: usize) {
        let (width, height) = (self.width as usize, self.height as usize);
        let rgb_at = |col: usize, row: usize| -> (i32, i32, i32) {
            let offset = (row * stride_px + col) * 4;
            bgra.get(offset..offset + 3).map_or((0, 0, 0), |px| {
                (i32::from(px[2]), i32::from(px[1]), i32::from(px[0]))
            })
        };
        let (luma, chroma) = self.nv12.split_at_mut(width * height);
        for row in 0..height {
            for col in 0..width {
                let (red, green, blue) = rgb_at(col, row);
                luma[row * width + col] =
                    clamp_u8(((47 * red + 157 * green + 16 * blue + 128) >> 8) + 16);
            }
        }
        for row in (0..height).step_by(2) {
            for col in (0..width).step_by(2) {
                let mut sum = (0, 0, 0);
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let (red, green, blue) = rgb_at(col + dx, row + dy);
                    sum = (sum.0 + red, sum.1 + green, sum.2 + blue);
                }
                let (red, green, blue) = (sum.0 / 4, sum.1 / 4, sum.2 / 4);
                let at = (row / 2) * width + col;
                chroma[at] = clamp_u8(((-26 * red - 87 * green + 112 * blue + 128) >> 8) + 128);
                chroma[at + 1] = clamp_u8(((112 * red - 102 * green - 10 * blue + 128) >> 8) + 128);
            }
        }
    }
}

#[cfg(windows)]
fn clamp_u8(value: i32) -> u8 {
    u8::try_from(value.clamp(0, 255)).unwrap_or(u8::MAX)
}

#[cfg(windows)]
fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn internal_error(message: String) -> ErrorData {
    mcp_error(
        synapse_core::error_codes::TOOL_INTERNAL_ERROR,
        format!("screenshot operation=record: {message}"),
    )
}

/// `CAPTURE_THREAD_FAILED`, with the code owned by `synapse-capture`.
fn thread_failed(detail: String) -> ErrorData {
    let error = synapse_capture::CaptureError::ThreadFailed { detail };
    mcp_error(
        error.code(),
        format!("screenshot operation=record: {error}"),
    )
}

/// One line of `frames.jsonl` (written here, read by the clip viewer).
#[derive(Serialize, serde::Deserialize)]
pub(super) struct FrameLine {
    pub(super) frame: u64,
    /// Milliseconds from frame 0 on the constant-rate timeline.
    pub(super) t_ms: u64,
    /// `true` when Windows delivered a new frame for this tick, `false` when
    /// the previous frame was repeated.
    pub(super) new: bool,
    /// Keyframe by the encoder's configured keyframe interval.
    pub(super) key: bool,
}

/// `clip.json`.
#[derive(Serialize)]
struct ClipManifest {
    schema_version: u32,
    clip_id: String,
    synapse_version: &'static str,
    clock: ClipClock,
    target: ClipTargetInfo,
    encode: ClipEncode,
    health: ClipHealth,
}

/// Frame 0 is the first frame Windows delivered; `t_ms` values count from it.
#[derive(Serialize)]
struct ClipClock {
    /// `GetTickCount64` (ms since boot, monotonic) at frame 0.
    monotonic_ms_at_frame0: u64,
    /// Wall clock (Unix ms) at frame 0, for joining log files.
    wall_clock_unix_ms_at_frame0: u64,
}

impl ClipClock {
    #[cfg(windows)]
    fn now() -> Self {
        Self {
            monotonic_ms_at_frame0: unsafe {
                windows::Win32::System::SystemInformation::GetTickCount64()
            },
            wall_clock_unix_ms_at_frame0: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| {
                    u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
                }),
        }
    }
}

#[derive(Serialize)]
struct ClipTargetInfo {
    /// `window` or `monitor`.
    kind: &'static str,
    hwnd: Option<i64>,
    monitor_index: Option<u32>,
    process_id: Option<u32>,
    exe: Option<String>,
    title: Option<String>,
    /// Size of the first captured frame.
    width: u32,
    height: u32,
}

impl ClipTargetInfo {
    #[cfg(windows)]
    fn read(target: ClipTarget, width: u32, height: u32) -> Self {
        match target {
            ClipTarget::Window { hwnd } => {
                let context = synapse_a11y::foreground_context(hwnd).ok();
                Self {
                    kind: "window",
                    hwnd: Some(hwnd),
                    monitor_index: context.as_ref().map(|c| c.monitor_index),
                    process_id: context.as_ref().map(|c| c.pid),
                    exe: context.as_ref().map(|c| c.process_path.clone()),
                    title: context.map(|c| c.window_title),
                    width,
                    height,
                }
            }
            ClipTarget::Monitor { monitor_index } => Self {
                kind: "monitor",
                hwnd: None,
                monitor_index: Some(monitor_index),
                process_id: None,
                exe: None,
                title: None,
                width,
                height,
            },
        }
    }
}

#[derive(Serialize)]
struct ClipEncode {
    codec: &'static str,
    profile: &'static str,
    pixel_format: &'static str,
    /// Friendly name of the encoder Media Foundation actually inserted.
    encoder: String,
    hardware: bool,
    /// `fragmented_mp4`, or `mp4` when fragmented MP4 was refused.
    container: &'static str,
    width: u32,
    height: u32,
    fps: u32,
    keyframe_interval_frames: u32,
    keyframe_interval_ms: u64,
    rate_control: RateControl,
    cursor: bool,
    /// How frames of another size are fitted into the fixed clip size.
    resize: &'static str,
}

#[derive(Serialize)]
struct RateControl {
    /// `quality` (value = quality level 0..=100) or `cbr_bitrate` (value = bits/s).
    mode: &'static str,
    value: u32,
}

#[derive(Default, Serialize)]
struct ClipHealth {
    frames_total: u64,
    frames_new: u64,
    frames_repeated: u64,
    /// Frames the capture channel (2 slots, drops the oldest) dropped before
    /// the recorder could drain them.
    frames_dropped_by_channel: u64,
    /// Frames drained but not encoded: superseded by a newer frame within the
    /// same tick, or failed conversion.
    frames_skipped: u64,
    requested_duration_ms: u64,
    /// Encoded duration (`frames_total / fps`).
    duration_ms: u64,
    wall_elapsed_ms: u64,
    /// Encoded frames per wall-clock second.
    achieved_fps: f64,
    /// New (non-repeated) frames per wall-clock second.
    new_frame_fps: f64,
    minimized_intervals: Vec<MinimizedInterval>,
    resizes: Vec<ClipResize>,
    /// `completed`, or why the clip stopped early.
    stopped_reason: StopReason,
}

/// A `target_minimized` interval: ticks that repeated because the window was
/// minimized. `end_frame` is exclusive.
#[derive(Serialize)]
struct MinimizedInterval {
    start_frame: u64,
    end_frame: u64,
    start_ms: u64,
    end_ms: u64,
}

/// The target's frame size changed at `frame`.
#[derive(Serialize)]
struct ClipResize {
    frame: u64,
    t_ms: u64,
    width: u32,
    height: u32,
}
