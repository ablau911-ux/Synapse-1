//! The automatic, best-effort Windows-playable copy of a finished clip.
//! The recording stays ready regardless of this independent conversion's outcome.

use std::fs::File;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use fs2::FileExt;
use serde::{Deserialize, Serialize};

use super::clip_folder::{
    ClipJson, ClipState, StopReason, open_lock_file, part_path, read_index, retry_brief_hold,
    write_atomically,
};

const STATUS_FILE: &str = "playable_video.json";
const LOCK_FILE: &str = "playable_video.lock";
const VIDEO_FILE: &str = "clip.mp4";
const TEMP_FILE: &str = ".clip-playable.tmp.mp4";
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const TIMEOUT_BASE_MS: u64 = 60_000;
const OUTPUT_FPS: u32 = 20;

/// Persisted separately so conversion never rewrites the recording's final metadata.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct ClipPlayableVideo {
    state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

impl ClipPlayableVideo {
    fn outcome(state: &str, reason: Option<String>) -> Self {
        Self {
            state: state.to_owned(),
            path: None,
            bytes: None,
            duration_ms: None,
            reason,
        }
    }

    /// Called only after clip.json is final. Only the small status write precedes
    /// spawning; all video work runs independently of the recorder.
    pub(super) fn start(folder: &Path) {
        if let Err(error) = Self::try_start(folder) {
            tracing::warn!(code = "CLIP_PLAYABLE_START_FAILED", error = %error);
        }
    }

    fn try_start(folder: &Path) -> Result<(), String> {
        let Some(lock) = Self::lock(folder)? else {
            return Ok(());
        };
        let clip = ClipJson::read(folder)?;
        if clip.state != ClipState::Ready {
            return Ok(());
        }
        if let Some(status) = Self::read(folder)?
            && status.state != "converting"
        {
            return Ok(());
        }
        Self::outcome("converting", None).save(folder)?;
        let folder = folder.to_path_buf();
        let worker_folder = folder.clone();
        let spawned = std::thread::Builder::new()
            .name("synapse-clip-playable".to_owned())
            .spawn(move || {
                // This OS lock also identifies interrupted conversions after restart.
                let _lock = lock;
                let status = match Self::convert(&worker_folder, &clip) {
                    Ok(status) => status,
                    Err(reason) => Self::outcome("failed", Some(reason)),
                };
                if let Err(error) =
                    retry_brief_hold(|| std::fs::remove_file(worker_folder.join(TEMP_FILE)))
                    && error.kind() != std::io::ErrorKind::NotFound
                {
                    tracing::warn!(code = "CLIP_PLAYABLE_TEMP_CLEANUP_FAILED", error = %error);
                }
                if let Err(error) = status.save(&worker_folder) {
                    tracing::warn!(code = "CLIP_PLAYABLE_STATUS_FAILED", error = %error);
                }
            });
        if spawned.is_err() {
            Self::outcome("failed", Some("conversion_thread_failed".to_owned())).save(&folder)?;
        }
        Ok(())
    }

    /// A persisted in-progress result without its OS lock means the service died.
    /// Re-read under the lock so a worker finishing concurrently is never overwritten.
    pub(super) fn status(folder: &Path) -> Option<serde_json::Value> {
        let result = (|| -> Result<Option<Self>, String> {
            let Some(mut status) = Self::read(folder)? else {
                return Ok(None);
            };
            if status.state == "converting"
                && let Some(_lock) = Self::lock(folder)?
            {
                status = Self::read(folder)?.unwrap_or(status);
                if status.state == "converting" {
                    status = Self::outcome("failed", Some("conversion_interrupted".to_owned()));
                    status.save(folder)?;
                }
            }
            Ok(Some(status))
        })();
        let status = match result {
            Ok(status) => status?,
            Err(error) => {
                tracing::warn!(code = "CLIP_PLAYABLE_STATUS_FAILED", error = %error);
                Self::outcome("failed", Some("playable_status_unreadable".to_owned()))
            }
        };
        serde_json::to_value(status).ok()
    }

    fn read(folder: &Path) -> Result<Option<Self>, String> {
        let path = folder.join(STATUS_FILE);
        match retry_brief_hold(|| std::fs::read(&path)) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|error| format!("could not read playable status: {error}")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("could not read playable status: {error}")),
        }
    }

    fn save(&self, folder: &Path) -> Result<(), String> {
        write_atomically(&folder.join(STATUS_FILE), self)
    }

    /// The file stays open for the worker's lifetime; closing it releases the lock.
    fn lock(folder: &Path) -> Result<Option<File>, String> {
        let file = open_lock_file(&folder.join(LOCK_FILE)).map_err(|error| error.to_string())?;
        match file.try_lock_exclusive() {
            Ok(()) => Ok(Some(file)),
            Err(error) if crate::single_instance::file_lock_error_is_contention(&error) => Ok(None),
            Err(error) => Err(format!("could not lock playable conversion: {error}")),
        }
    }

    fn convert(folder: &Path, clip: &ClipJson) -> Result<Self, String> {
        if clip.parts.is_empty() {
            return Ok(Self::outcome("skipped", Some("no_video_parts".to_owned())));
        }
        // Finalized status and totals both use recorded_ms, not the planned end.
        // A completed fractional instant may extend beyond that end.
        let duration_ms = if clip.stop_reason == Some(StopReason::ServiceRestart) {
            // Recovery pads the index with lost instants and includes them in
            // recorded_ms. Only surviving pictures, repeats, and stills prove
            // how far recording reached; an unchanged tail remains recorded.
            read_index(folder)
                .map_err(|error| format!("recorded_length_unreadable: {error}"))?
                .iter()
                .rev()
                .find(|line| {
                    line.enc.is_some() || line.same.is_some() || line.still_file().is_some()
                })
                .map_or(0, |line| {
                    line.instant.saturating_add(1).saturating_mul(1_000) / u64::from(clip.fps)
                })
        } else {
            clip.recorded_ms
        }
        .min(clip.end_ms);
        if duration_ms == 0 {
            return Ok(Self::outcome("skipped", Some("zero_duration".to_owned())));
        }
        let mut parts: Vec<_> = clip.parts.iter().collect();
        parts.sort_by_key(|part| (part.first_instant, part.part));
        let width = parts[0].width.div_ceil(2) * 2;
        let height = parts[0].height.div_ceil(2) * 2;
        if width == 0 || height == 0 {
            return Err("invalid_part_dimensions".to_owned());
        }
        // Encode through the final 20 FPS instant, then shorten that packet in
        // the MP4 time base when duration_ms falls between instants.
        let frame_ms = 1_000 / u64::from(OUTPUT_FPS);
        let encoded_end_ms = duration_ms.div_ceil(frame_ms) * frame_ms;
        let length = seconds(encoded_end_ms);
        let mut command =
            Command::new(std::env::var_os("SYNAPSE_FFMPEG").unwrap_or_else(|| "ffmpeg".into()));
        command.args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y"]);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;
            command.creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS);
        }
        let mut filters = Vec::new();
        for (index, part) in parts.iter().enumerate() {
            let start_ms = part.first_instant.saturating_mul(1_000) / u64::from(clip.fps);
            let end_ms = parts
                .get(index + 1)
                .map_or(encoded_end_ms, |next| {
                    next.first_instant.saturating_mul(1_000) / u64::from(clip.fps)
                })
                .min(encoded_end_ms);
            let segment_start = if index == 0 { 0 } else { start_ms };
            if start_ms >= duration_ms || end_ms <= start_ms || end_ms <= segment_start {
                return Err("invalid_part_timeline".to_owned());
            }
            command
                .args(["-threads", "2", "-i"])
                .arg(part_path(folder, part.part));
            // IVF timestamps are global recorder instants. Rebase each part, keep
            // gaps between its pictures, then hold its last picture until the next
            // part's first instant (or duration_ms). A missing initial picture is black.
            let prefix = if index == 0 && start_ms > 0 {
                format!(":start_mode=add:start_duration={}", seconds(start_ms))
            } else {
                String::new()
            };
            filters.push(format!(
                "[{index}:v]setpts=PTS-STARTPTS,scale={width}:{height}:force_original_aspect_ratio=decrease:force_divisible_by=2,pad={width}:{height}:(ow-iw)/2:(oh-ih)/2,setsar=1,tpad=stop_mode=clone:stop_duration={length}{prefix},fps={OUTPUT_FPS},trim=duration={},setpts=PTS-STARTPTS[v{index}]",
                seconds(end_ms - segment_start)
            ));
        }
        let inputs: String = (0..parts.len())
            .map(|index| format!("[v{index}]"))
            .collect();
        filters.push(format!("{inputs}concat=n={}:v=1:a=0[out]", parts.len()));
        command
            .args(["-filter_complex_threads", "1", "-filter_complex"])
            .arg(filters.join(";"))
            .args([
                "-map",
                "[out]",
                "-an",
                "-t",
                &length,
                "-c:v",
                "libx264",
                "-threads",
                "2",
                "-pix_fmt",
                "yuv420p",
                "-crf",
                "18",
                "-movflags",
                "+faststart",
                "-f",
                "mp4",
            ]);
        if encoded_end_ms != duration_ms {
            // Without B pictures, packet decode and display order agree. Rescale
            // packet times to milliseconds and shorten only the final duration.
            command
                .args(["-bf", "0", "-bsf:v"])
                .arg(format!(
                    "setts=time_base=1/1000:prescale=1:duration='min(DURATION,{duration_ms}-PTS)'"
                ))
                .args(["-video_track_timescale", "1000"]);
        }
        command.arg(folder.join(TEMP_FILE));
        Self::run(&mut command, duration_ms)?;
        let temp = folder.join(TEMP_FILE);
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&temp)
            .map_err(|_| "output_missing".to_owned())?;
        let bytes = file
            .metadata()
            .map_err(|_| "output_metadata_failed".to_owned())?
            .len();
        if bytes == 0 {
            return Err("output_empty".to_owned());
        }
        file.sync_all()
            .map_err(|_| "output_sync_failed".to_owned())?;
        drop(file);
        retry_brief_hold(|| std::fs::rename(&temp, folder.join(VIDEO_FILE)))
            .map_err(|_| "output_publish_failed".to_owned())?;
        Ok(Self {
            state: "ready".to_owned(),
            path: Some(folder.join(VIDEO_FILE).to_string_lossy().into_owned()),
            bytes: Some(bytes),
            duration_ms: Some(duration_ms),
            reason: None,
        })
    }

    fn run(command: &mut Command, length_ms: u64) -> Result<(), String> {
        let mut child = command.spawn().map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                "ffmpeg_not_found".to_owned()
            } else {
                format!("ffmpeg_start_failed: {error}")
            }
        })?;
        let timeout =
            Duration::from_millis(TIMEOUT_BASE_MS.saturating_add(length_ms.saturating_mul(2)));
        let started = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => return Ok(()),
                Ok(Some(status)) => return Err(format!("ffmpeg_exit: {status}")),
                Ok(None) if started.elapsed() < timeout => std::thread::sleep(POLL_INTERVAL),
                result => {
                    // Every timeout/wait failure reaps the process before returning.
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(if result.is_err() {
                        "ffmpeg_wait_failed".to_owned()
                    } else {
                        "ffmpeg_timeout".to_owned()
                    });
                }
            }
        }
    }
}

fn seconds(milliseconds: u64) -> String {
    format!("{}.{:03}", milliseconds / 1_000, milliseconds % 1_000)
}
