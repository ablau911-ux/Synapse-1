//! The clip worker (docs/clip-recorder-plan.md, version 4.1, section B):
//! a long-lived worker mode of the same executable that forms segments from
//! `frames.jsonl`, encodes them and commits them (packages 3a and 3b).
//!
//! - `ProcessorBudget` owns the 10 % rule (E3, E10): the live part reports its
//!   measured share every second (A9) and the worker's cap is what remains.
//! - `ClipWorkerHost` runs in the service: one worker at a time for the whole
//!   service, a first-in first-out queue that recovered clips also enter (C11),
//!   a job with a hard processor cap recomputed every second, and recovery at
//!   start (B10). It reuses only `current_exe` and the kill-on-close job of
//!   `desktop_worker` (F6, F10): no hidden desktop, no window, no 10 s wait.
//! - `ClipWorker` runs in the child process: B1, B2, B8 and B9.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::clip_folder::{
    BReport, ClipJson, ClipRegistry, ClipState, FrameLine, LossCause, LostInstant, RecorderLock,
    StopReason, read_frame_lines, waiting_frame_path,
};

/// U_max: the recorder never uses more than this share of the processor time
/// of the whole machine (E10).
pub(super) const CPU_LIMIT_PERCENT: f64 = 10.0;
/// A segment closes after this much clip time.
pub(super) const SEGMENT_MS: u64 = 60_000;
/// A segment closes when its retained waiting bytes reach this (package 0, M4/M5; S6).
/// Counted as the sizes of its waiting files on disk (compressed).
pub(super) const SEGMENT_RAW_BUDGET_BYTES: u64 = 1 << 30;
/// A new window size counts as settled once it held this long (E4).
pub(super) const RESIZE_SETTLE_MS: u64 = 500;
/// The worker cap is recomputed every second (E3); the worker looks for new
/// frames of a clip still recording as often.
const WORKER_TICK_MS: u64 = 1_000;
/// Worker exit codes.
const EXIT_READY: u8 = 0;
const EXIT_FAILED: u8 = 1;
const EXIT_BUSY: u8 = 2;
/// Held by the worker that owns a clip, so two workers never encode one clip.
const WORKER_LOCK_FILE: &str = "worker.lock";
/// The clip fails on this many worker crashes in a row without a newly
/// committed segment (B10: the segment is encoded again from its waiting
/// frames once).
const NO_PROGRESS_CRASH_LIMIT: u32 = 2;

/// The live share and the worker cap, in percent of the whole machine.
pub(super) struct ProcessorBudget {
    live_share_bits: AtomicU64,
    worker_cap_bits: AtomicU64,
}

static BUDGET: ProcessorBudget = ProcessorBudget {
    live_share_bits: AtomicU64::new(0),
    // 10.0_f64 as bits: the whole limit while nothing is recorded.
    worker_cap_bits: AtomicU64::new(0x4024_0000_0000_0000),
};

impl ProcessorBudget {
    pub(super) fn global() -> &'static Self {
        &BUDGET
    }

    /// Records the live part's share of the last second; the worker's cap
    /// becomes `CPU_LIMIT_PERCENT` minus it (never below zero). Returns the cap.
    pub(super) fn set_live_share(&self, percent: f64) -> f64 {
        let share = percent.max(0.0);
        let cap = (CPU_LIMIT_PERCENT - share).max(0.0);
        self.live_share_bits
            .store(share.to_bits(), Ordering::Relaxed);
        self.worker_cap_bits.store(cap.to_bits(), Ordering::Relaxed);
        cap
    }

    /// The live part ended: the worker may use the whole limit again.
    pub(super) fn clear_live_share(&self) {
        self.set_live_share(0.0);
    }

    pub(super) fn live_share(&self) -> f64 {
        f64::from_bits(self.live_share_bits.load(Ordering::Relaxed))
    }

    pub(super) fn worker_cap(&self) -> f64 {
        f64::from_bits(self.worker_cap_bits.load(Ordering::Relaxed))
    }

    /// `true` when `cpu_hns` of processor time within one second already
    /// reaches the whole limit on a machine with `threads` logical processors
    /// (A9: the live part then sheds the read-back of new frames).
    pub(super) fn live_alone_reached(cpu_hns: i64, threads: u32) -> bool {
        share_percent(cpu_hns, 10_000_000, threads) >= CPU_LIMIT_PERCENT
    }
}

/// Processor time `cpu_hns` over `wall_hns` as a share of the whole machine.
pub(super) fn share_percent(cpu_hns: i64, wall_hns: i64, threads: u32) -> f64 {
    cpu_hns as f64 * 100.0 / (wall_hns.max(1) as f64 * f64::from(threads.max(1)))
}

/// n: the machine's logical processors (A1), for every share of the whole
/// machine.
pub(super) fn logical_processors() -> u32 {
    std::thread::available_parallelism().map_or(1, |threads| {
        u32::try_from(threads.get()).unwrap_or(u32::MAX)
    })
}

// ---------------------------------------------------------------------------
// Segment forming (B2)
// ---------------------------------------------------------------------------

/// A range of `frames.jsonl` lines that forms one segment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SegmentRange {
    pub(super) first: usize,
    pub(super) last: usize,
    /// Captured size of the segment's canvas: the frame size, or for a
    /// transition segment the largest size seen during the drag (E4).
    pub(super) canvas: (u32, u32),
    pub(super) padded: bool,
}

/// B2: forms the next segment from the lines at and after `start`. `None`
/// while the clip is still recording and the next segment is not complete.
/// The raw budget counts the waiting files of `folder` as they are on disk
/// (compressed), read for each new frame.
pub(super) fn next_segment(
    folder: &Path,
    lines: &[FrameLine],
    start: usize,
    closed: bool,
) -> Option<SegmentRange> {
    let first = lines.get(start)?;
    let size = |line: &FrameLine| (line.width, line.height);
    if !settled_at(lines, start, closed)? {
        // A drag: one transition segment up to the point where a size settles.
        let mut end = start;
        while end < lines.len() && !settled_at(lines, end, closed)? {
            end += 1;
        }
        if end == lines.len() && !closed {
            return None;
        }
        let last = end.max(start + 1) - 1;
        let canvas = lines[start..=last].iter().fold((0, 0), |(w, h), line| {
            (w.max(line.width), h.max(line.height))
        });
        return Some(SegmentRange {
            first: start,
            last,
            canvas,
            padded: true,
        });
    }
    let canvas = size(first);
    let mut raw_bytes = 0_u64;
    for (index, line) in lines.iter().enumerate().skip(start) {
        let over_time = line.t_ms.saturating_sub(first.t_ms) >= SEGMENT_MS;
        let resized = line.width != 0 && size(line) != canvas;
        let frame_bytes = if line.new {
            std::fs::metadata(waiting_frame_path(folder, line.instant)).map_or(0, |file| file.len())
        } else {
            0
        };
        let over_budget = raw_bytes + frame_bytes > SEGMENT_RAW_BUDGET_BYTES;
        if index > start && (over_time || resized || over_budget) {
            return Some(SegmentRange {
                first: start,
                last: index - 1,
                canvas,
                padded: false,
            });
        }
        raw_bytes += frame_bytes;
    }
    closed.then(|| SegmentRange {
        first: start,
        last: lines.len() - 1,
        canvas,
        padded: false,
    })
}

/// Whether the size at `index` holds for `RESIZE_SETTLE_MS`. `None` when it
/// cannot be known yet (the clip is still recording and too few lines exist).
fn settled_at(lines: &[FrameLine], index: usize, closed: bool) -> Option<bool> {
    let line = &lines[index];
    for later in &lines[index..] {
        if later.t_ms >= line.t_ms + RESIZE_SETTLE_MS {
            return Some(true);
        }
        if later.width != 0 && (later.width, later.height) != (line.width, line.height) {
            return Some(false);
        }
    }
    closed.then_some(true)
}

/// Every instant that would have shown a picture the encoder could not read:
/// from its sample up to the next sample (the next new frame) or the end of
/// the segment. A picture held over from the segment before keeps the cause
/// it was named with there.
fn unreadable_lost(
    clip: &ClipJson,
    lines: &[FrameLine],
    range: SegmentRange,
    samples: &[super::clip_encoder::SegmentSample],
    unreadable: &[(usize, LossCause)],
) -> Vec<LostInstant> {
    let end = lines[range.last].instant + 1;
    let held_cause = shown_at(lines, range.first)
        .map(|at| lines[at].instant)
        .filter(|&instant| instant < lines[range.first].instant)
        .and_then(|instant| clip.lost.iter().find(|lost| lost.instant == instant))
        .map(|lost| lost.cause)
        .filter(|cause| {
            matches!(
                cause,
                LossCause::WaitingUnreadable | LossCause::WaitingMissing
            )
        });
    let mut lost = Vec::new();
    for &(order, cause) in unreadable {
        let Some(sample) = samples.get(order) else {
            continue;
        };
        let to = samples.get(order + 1).map_or(end, |next| next.instant);
        let cause = if order == 0 {
            held_cause.unwrap_or(cause)
        } else {
            cause
        };
        lost.extend((sample.instant..to).map(|instant| LostInstant { instant, cause }));
    }
    lost
}

/// The line of the picture on screen at `index`: the last new line at or
/// before it, whether or not its waiting file is still there.
fn shown_at(lines: &[FrameLine], index: usize) -> Option<usize> {
    (0..=index).rev().find(|&at| lines[at].new)
}

/// The instant whose picture is shown at the start of a segment: the segment's
/// first line when it is new, else the last new frame before it.
fn picture_at(lines: &[FrameLine], folder: &Path, index: usize) -> Option<usize> {
    (0..=index)
        .rev()
        .find(|&at| lines[at].new && waiting_frame_path(folder, lines[at].instant).is_file())
}

// ---------------------------------------------------------------------------
// The worker process (child side)
// ---------------------------------------------------------------------------

/// Entry of `--mode clip-worker`: works on one clip folder until it is ready
/// or failed. Exit code 0 ready, 1 failed, 2 another worker owns the clip.
#[cfg(windows)]
pub(crate) fn run_clip_worker_from_cli(folder: &Path) -> u8 {
    use fs2::FileExt as _;

    let lock_path = folder.join(WORKER_LOCK_FILE);
    let Ok(lock) = super::clip_folder::open_lock_file(&lock_path) else {
        return EXIT_FAILED;
    };
    if lock.try_lock_exclusive().is_err() {
        return EXIT_BUSY;
    }
    let outcome = ClipWorker {
        folder: folder.to_path_buf(),
    }
    .run();
    let _ = FileExt::unlock(&lock);
    match outcome {
        Ok(()) => EXIT_READY,
        Err(error) => {
            tracing::warn!(code = "CLIP_WORKER_FAILED", folder = %folder.display(), error = %error);
            let _ = ClipJson::update(folder, |clip| {
                clip.state = ClipState::Failed;
                clip.failure = Some(error);
            });
            EXIT_FAILED
        }
    }
}

#[cfg(windows)]
use fs2::FileExt;

/// One clip, worked on by the worker process.
#[cfg(windows)]
struct ClipWorker {
    folder: PathBuf,
}

#[cfg(windows)]
impl ClipWorker {
    /// B1: resumes from `clip.json`; forms, encodes and commits segments until
    /// every frame is committed; then B9.
    fn run(&self) -> Result<(), String> {
        self.discard_part_files();
        loop {
            let clip = ClipJson::read(&self.folder)?;
            match clip.state {
                ClipState::Ready => return Ok(()),
                ClipState::Failed => {
                    return Err(clip
                        .failure
                        .unwrap_or_else(|| "the clip is marked failed".to_owned()));
                }
                ClipState::Recording | ClipState::Encoding => {}
            }
            let lines = read_frame_lines(&self.folder)
                .map_err(|error| format!("could not read frames.jsonl: {error}"))?;
            let start = clip.segments.last().map_or(0, |segment| {
                lines.partition_point(|line| line.instant <= segment.last_instant)
            });
            let closed = clip.state == ClipState::Encoding;
            match next_segment(&self.folder, &lines, start, closed) {
                Some(range) => self.commit(&clip, &lines, range)?,
                // No line and no segment: the service ended before frame zero
                // was written, so the recovered clip holds nothing to be ready.
                None if closed && clip.segments.is_empty() => {
                    return Err("no frame recorded".to_owned());
                }
                None if closed => return self.ready(),
                None => std::thread::sleep(std::time::Duration::from_millis(WORKER_TICK_MS)),
            }
        }
    }

    /// B10: a `.part` file is the segment that was being encoded; it is
    /// deleted and encoded again from its waiting frames.
    fn discard_part_files(&self) {
        if let Ok(read) = std::fs::read_dir(&self.folder) {
            for entry in read.filter_map(Result::ok) {
                if super::clip_folder::is_part_file(&entry.path()) {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }

    /// B8: encodes a segment into `segment-NNNN.mp4.part`, flushes it, renames
    /// it, records its checkpoint, and only then deletes its waiting frames.
    fn commit(
        &self,
        clip: &ClipJson,
        lines: &[FrameLine],
        range: SegmentRange,
    ) -> Result<(), String> {
        let index = u32::try_from(clip.segments.len() + 1).unwrap_or(u32::MAX);
        let start_ms = lines[range.first].t_ms;
        let end_ms = lines[range.last].t_ms + 1_000 / u64::from(clip.fps.max(1));
        let samples = self.samples(lines, range, start_ms, end_ms);
        if samples.is_empty() {
            return Err(format!(
                "segment {index} has no waiting frame to start from (instants {}..={})",
                lines[range.first].instant, lines[range.last].instant
            ));
        }
        let part = self
            .folder
            .join(super::clip_folder::segment_part_name(index));
        let seed = self.seed(lines, range, &samples);
        let input = super::clip_encoder::SegmentInput {
            canvas_width: range.canvas.0,
            canvas_height: range.canvas.1,
            scale: super::capture_clip::scale_for_height(range.canvas.1),
            fps: clip.fps,
            samples: &samples,
            seed: seed.as_ref(),
        };
        let encoded = super::clip_encoder::encode_segment(&part, &input)?;
        // FlushFileBuffers needs write access on Windows; a read-only handle gets "Access is denied".
        std::fs::OpenOptions::new()
            .write(true)
            .open(&part)
            .and_then(|file| file.sync_all())
            .map_err(|error| format!("could not flush {}: {error}", part.display()))?;
        let file_name = super::clip_folder::segment_file_name(index);
        let finished = self.folder.join(&file_name);
        super::clip_folder::rename_retrying(&part, &finished)
            .map_err(|error| format!("could not rename {}: {error}", part.display()))?;
        let (bytes, sha256) = super::clip_folder::file_size_and_sha256(&finished)
            .map_err(|error| format!("could not hash {}: {error}", finished.display()))?;
        let checkpoint = super::clip_folder::SegmentCheckpoint {
            index,
            file: file_name,
            bytes,
            sha256,
            first_instant: lines[range.first].instant,
            last_instant: lines[range.last].instant,
            start_ms,
            end_ms,
            width: encoded.width,
            height: encoded.height,
            padded: range.padded,
            bound_held: encoded.bound_held,
            keyframe_instants: encoded.keyframe_instants,
            encoder: encoded.encoder,
            hardware: encoded.hardware,
            quality: encoded.quality,
        };
        let unreadable = unreadable_lost(clip, lines, range, &samples, &encoded.unreadable);
        ClipJson::update(&self.folder, |clip| {
            clip.segments.push(checkpoint);
            clip.add_lost(unreadable);
        })?;
        self.delete_committed_waiting(&samples, seed.as_ref(), encoded.last_good.as_ref());
        tracing::info!(
            code = "CLIP_SEGMENT_COMMITTED",
            folder = %self.folder.display(),
            index,
            bytes,
            "readback=clip_segment outcome=committed"
        );
        Ok(())
    }

    /// The samples of a segment: the picture shown at its start, then every
    /// new frame, each with its time within the segment and its duration (B5).
    fn samples(
        &self,
        lines: &[FrameLine],
        range: SegmentRange,
        start_ms: u64,
        end_ms: u64,
    ) -> Vec<super::clip_encoder::SegmentSample> {
        // Every expected new frame is a sample, also when its waiting file is
        // missing: the encoder then keeps the picture before it and its
        // instants are named as lost (`waiting_missing`).
        let mut picks: Vec<(usize, u64)> = Vec::new();
        if let Some(at) = shown_at(lines, range.first) {
            picks.push((at, 0));
        }
        for (index, line) in lines
            .iter()
            .enumerate()
            .take(range.last + 1)
            .skip(range.first + 1)
        {
            if line.new {
                picks.push((index, line.t_ms - start_ms));
            }
        }
        let segment_length = end_ms - start_ms;
        picks
            .iter()
            .enumerate()
            .map(|(order, &(index, local_ms))| {
                let next_ms = picks
                    .get(order + 1)
                    .map_or(segment_length, |&(_, next)| next);
                let line = &lines[index];
                super::clip_encoder::SegmentSample {
                    waiting: waiting_frame_path(&self.folder, line.instant),
                    width: line.width,
                    height: line.height,
                    instant: if order == 0 {
                        lines[range.first].instant
                    } else {
                        line.instant
                    },
                    local_ms,
                    duration_ms: next_ms - local_ms,
                }
            })
            .collect()
    }

    /// The last good picture before the segment (kept by the segment before),
    /// when it is not already the segment's first sample.
    fn seed(
        &self,
        lines: &[FrameLine],
        range: SegmentRange,
        samples: &[super::clip_encoder::SegmentSample],
    ) -> Option<super::clip_encoder::SegmentSample> {
        let at = picture_at(lines, &self.folder, range.first.checked_sub(1)?)?;
        let line = &lines[at];
        let waiting = waiting_frame_path(&self.folder, line.instant);
        (samples.first().map(|first| &first.waiting) != Some(&waiting)).then(|| {
            super::clip_encoder::SegmentSample {
                waiting,
                width: line.width,
                height: line.height,
                instant: line.instant,
                local_ms: 0,
                duration_ms: 0,
            }
        })
    }

    /// Deletes the waiting frames of a committed segment, except the last picture
    /// the encoder could draw: it is the picture shown at the start of the
    /// next segment, also when this segment's last frame was unreadable. The
    /// seed is deleted once a newer good picture replaces it.
    fn delete_committed_waiting(
        &self,
        samples: &[super::clip_encoder::SegmentSample],
        seed: Option<&super::clip_encoder::SegmentSample>,
        last_good: Option<&PathBuf>,
    ) {
        for waiting in samples.iter().chain(seed).map(|sample| &sample.waiting) {
            if Some(waiting) != last_good {
                let _ = std::fs::remove_file(waiting);
            }
        }
    }

    /// B9: every segment is committed: the conditions G and Q, state `ready`.
    fn ready(&self) -> Result<(), String> {
        let (clip, ()) = ClipJson::update(&self.folder, |clip| {
            let (g, q) = segment_conditions(clip);
            clip.conditions.g = Some(g);
            clip.conditions.q = Some(q);
            clip.state = ClipState::Ready;
        })?;
        if let Ok(read) = std::fs::read_dir(self.folder.join(super::clip_folder::WAITING_DIR)) {
            for entry in read.filter_map(Result::ok) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
        tracing::info!(
            code = "CLIP_READY",
            folder = %self.folder.display(),
            segments = clip.segments.len(),
            "readback=clip outcome=ready"
        );
        Ok(())
    }
}

/// B9: conditions G and Q from the committed segments. G is `padded` for the
/// instants of transition segments and not held above the encoder bound; Q is
/// held only when every segment's quality mode was accepted and read back at
/// a calibrated level (C6).
fn segment_conditions(
    clip: &ClipJson,
) -> (
    super::clip_folder::ConditionStatus,
    super::clip_folder::ConditionStatus,
) {
    let mut bound_failed = Vec::new();
    let mut padded = Vec::new();
    for segment in &clip.segments {
        let range = segment.first_instant..=segment.last_instant;
        if !segment.bound_held {
            bound_failed.extend(range);
        } else if segment.padded {
            padded.push(serde_json::json!([
                segment.first_instant,
                segment.last_instant
            ]));
        }
    }
    let g_status = if !bound_failed.is_empty() {
        "not_held"
    } else if padded.is_empty() {
        "held"
    } else {
        "padded"
    };
    let segments: Vec<serde_json::Value> = clip
        .segments
        .iter()
        .map(|segment| {
            serde_json::json!({
                "file": segment.file,
                "recorded_width": segment.width,
                "recorded_height": segment.height,
                "padded": segment.padded,
                "bound_held": segment.bound_held,
            })
        })
        .collect();
    let g = super::clip_folder::ConditionStatus {
        status: g_status.to_owned(),
        failed_instants: bound_failed,
        detail: Some(serde_json::json!({
            "window_width": clip.geometry.window_width,
            "window_height": clip.geometry.window_height,
            "scale": clip.geometry.scale,
            "segments": segments,
            "padded_instants": padded,
        })),
    };
    let statuses: Vec<&str> = clip
        .segments
        .iter()
        .map(|segment| segment.quality.status.as_str())
        .collect();
    let q_status = if statuses.contains(&"not_held") {
        "not_held"
    } else if !statuses.is_empty() && statuses.iter().all(|status| *status == "held") {
        "held"
    } else {
        "unverified"
    };
    let q = super::clip_folder::ConditionStatus {
        status: q_status.to_owned(),
        failed_instants: Vec::new(),
        detail: Some(serde_json::json!({
            "segments": clip.segments.iter().map(|segment| serde_json::json!({
                "file": segment.file,
                "encoder": segment.encoder,
                "hardware": segment.hardware,
                "quality": segment.quality,
            })).collect::<Vec<_>>(),
        })),
    };
    (g, q)
}

// ---------------------------------------------------------------------------
// The host (service side)
// ---------------------------------------------------------------------------

/// B10, at service start: every clip in state `recording` or `encoding`
/// enters the queue; a clip found `recording` becomes `encoding` with stop
/// reason `service_restart`, and its frames lost with the service are named.
#[cfg(windows)]
pub(crate) fn recover_clips_on_start() {
    std::thread::Builder::new()
        .name("synapse-clip-recovery".to_owned())
        .spawn(|| {
            for folder in ClipRegistry::all_folders() {
                // A recorder of a running service process (this one or
                // another) still holds the clip: it is live, not left over.
                if RecorderLock::held(&folder) {
                    continue;
                }
                let Ok(clip) = ClipJson::read(&folder) else {
                    continue;
                };
                match clip.state {
                    ClipState::Recording => {
                        if let Err(error) = mark_service_restart(&folder) {
                            tracing::warn!(code = "CLIP_RECOVERY_FAILED", folder = %folder.display(), error = %error);
                            continue;
                        }
                        ClipWorkerHost::global().enqueue(folder);
                    }
                    ClipState::Encoding => ClipWorkerHost::global().enqueue(folder),
                    ClipState::Ready | ClipState::Failed => {}
                }
            }
        })
        .map(drop)
        .unwrap_or_else(|error| {
            tracing::warn!(code = "CLIP_RECOVERY_THREAD_FAILED", error = %error);
        });
}

fn mark_service_restart(folder: &Path) -> Result<(), String> {
    let lines = read_frame_lines(folder).map_err(|error| error.to_string())?;
    ClipJson::update(folder, |clip| {
        let committed = clip.segments.last().map(|segment| segment.last_instant);
        let mut lost: Vec<LostInstant> = lines
            .iter()
            .filter(|line| {
                committed.is_none_or(|last| line.instant > last)
                    && line.new
                    && !waiting_frame_path(folder, line.instant).is_file()
            })
            .map(|line| LostInstant {
                instant: line.instant,
                cause: LossCause::ServiceRestart,
            })
            .collect();
        // The instants after the last written line up to the planned end were
        // never handled: instant k belongs to the clip while k / f < end_ms.
        let fps = u64::from(clip.fps.max(1));
        let first_unwritten = lines.last().map_or(0, |line| line.instant + 1);
        let planned_instants = clip.end_ms.saturating_mul(fps).div_ceil(1_000);
        lost.extend(
            (first_unwritten..planned_instants).map(|instant| LostInstant {
                instant,
                cause: LossCause::ServiceRestart,
            }),
        );
        clip.recorded_ms = lines.last().map_or(0, |line| line.t_ms + 1_000 / fps);
        clip.state = ClipState::Encoding;
        clip.stop_reason = Some(StopReason::ServiceRestart);
        // The recorder never wrote C and T (it writes them when it stops);
        // they start held and `add_lost` names every loss in them.
        let length_ms = clip.recorded_ms;
        clip.conditions
            .c
            .get_or_insert_with(|| super::clip_folder::ConditionStatus {
                status: "held".to_owned(),
                failed_instants: Vec::new(),
                detail: Some(serde_json::json!({
                    "delivery_completeness": "checked only in the proof",
                    "note": "the service ended during the recording; the hold time was not measured",
                })),
            });
        clip.conditions
            .t
            .get_or_insert_with(|| super::clip_folder::ConditionStatus {
                status: "held".to_owned(),
                failed_instants: Vec::new(),
                detail: Some(serde_json::json!({
                    "length_ms": length_ms,
                    "instants_lost": 0,
                    "stop_reason": StopReason::ServiceRestart.as_str(),
                    "note": "the service ended during the recording; the instants after the last written line are named as lost",
                })),
            });
        clip.add_lost(lost);
    })
    .map(drop)
}

/// One clip worker at a time for the whole service, with a first-in
/// first-out queue (C11).
#[cfg(windows)]
pub(super) struct ClipWorkerHost {
    queue: std::sync::Mutex<std::collections::VecDeque<PathBuf>>,
    wake: std::sync::Condvar,
}

#[cfg(windows)]
static HOST: std::sync::OnceLock<ClipWorkerHost> = std::sync::OnceLock::new();

#[cfg(windows)]
impl ClipWorkerHost {
    /// The host; its thread starts on first use.
    pub(super) fn global() -> &'static Self {
        HOST.get_or_init(|| {
            let spawned = std::thread::Builder::new()
                .name("synapse-clip-worker-host".to_owned())
                .spawn(|| Self::global().run());
            if let Err(error) = spawned {
                tracing::warn!(code = "CLIP_WORKER_HOST_FAILED", error = %error);
            }
            Self {
                queue: std::sync::Mutex::new(std::collections::VecDeque::new()),
                wake: std::sync::Condvar::new(),
            }
        })
    }

    /// Adds a clip folder at the end of the queue (once).
    pub(super) fn enqueue(&self, folder: PathBuf) {
        let mut queue = self
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !queue.contains(&folder) {
            queue.push_back(folder);
        }
        self.wake.notify_one();
    }

    fn next(&self) -> PathBuf {
        let mut queue = self
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if let Some(folder) = queue.pop_front() {
                return folder;
            }
            queue = self
                .wake
                .wait(queue)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    fn run(&self) {
        // Worker crashes in a row without a newly committed segment, per clip.
        let mut no_progress = std::collections::HashMap::new();
        loop {
            let folder = self.next();
            self.work_on(&folder, &mut no_progress);
            // B4, C14: a failed clip gets no further worker, so a recording
            // that still runs into it stops now.
            if ClipJson::read(&folder).is_ok_and(|clip| clip.state == ClipState::Failed) {
                super::capture_clip::stop_failed_recording(&folder);
            }
        }
    }

    /// One worker run on a clip, unless the clip is already ready or failed
    /// (the recorder enqueues its clip again when it stops). The run is judged
    /// by the state in `clip.json` afterwards, never by the exit code alone.
    fn work_on(&self, folder: &Path, no_progress: &mut std::collections::HashMap<PathBuf, u32>) {
        let segments_before = match ClipJson::read(folder) {
            Ok(clip) if matches!(clip.state, ClipState::Ready | ClipState::Failed) => {
                no_progress.remove(folder);
                return;
            }
            clip => clip.map_or(0, |clip| clip.segments.len()),
        };
        let detail = match WorkerRun::supervise(folder) {
            // Another worker process holds the clip; its host judges the outcome.
            Ok(EXIT_BUSY) => return,
            Ok(code) => format!("exit code {code}"),
            Err(error) => error,
        };
        self.after_crash(folder, segments_before, &detail, no_progress);
    }

    /// A clip still `recording` or `encoding` after its worker ended was left
    /// mid-work (a crash, a kill, or a worker that could not start), whatever
    /// the exit code. A worker crash loses nothing: the clip enters the queue
    /// again and the segment is encoded again from its waiting frames (B10); the
    /// second crash in a row without a new committed segment fails it, as B4
    /// does for the encoder.
    fn after_crash(
        &self,
        folder: &Path,
        segments_before: usize,
        detail: &str,
        no_progress: &mut std::collections::HashMap<PathBuf, u32>,
    ) {
        let Ok(clip) = ClipJson::read(folder) else {
            return;
        };
        if matches!(clip.state, ClipState::Ready | ClipState::Failed) {
            no_progress.remove(folder);
            return;
        }
        let crashes = if clip.segments.len() > segments_before {
            0
        } else {
            no_progress.get(folder).copied().unwrap_or(0) + 1
        };
        if crashes < NO_PROGRESS_CRASH_LIMIT {
            no_progress.insert(folder.to_path_buf(), crashes);
            tracing::warn!(code = "CLIP_WORKER_CRASHED", folder = %folder.display(), detail);
            self.enqueue(folder.to_path_buf());
            return;
        }
        no_progress.remove(folder);
        let failure = format!("the clip worker ended without progress: {detail}");
        let _ = ClipJson::update(folder, |clip| {
            clip.state = ClipState::Failed;
            clip.failure = Some(failure);
        });
    }
}

/// One worker process in its own job, supervised every second: the hard
/// processor cap is set to what the live part leaves of 10 % (A9, E3), and the
/// worker's own share is measured for condition B.
#[cfg(windows)]
struct WorkerRun {
    process: WorkerProcess,
    threads: u32,
    started: std::time::Instant,
    max_worker_share: f64,
    max_sum: f64,
    seconds_over_limit: u64,
}

#[cfg(windows)]
impl WorkerRun {
    fn supervise(folder: &Path) -> Result<u8, String> {
        let mut run = Self {
            process: WorkerProcess::spawn(folder)?,
            threads: logical_processors(),
            started: std::time::Instant::now(),
            max_worker_share: 0.0,
            max_sum: 0.0,
            seconds_over_limit: 0,
        };
        let mut cpu_before = run.process.cpu_hns();
        let mut tick_started = synapse_capture::now_hns();
        loop {
            let cap = ProcessorBudget::global().worker_cap();
            run.process.set_cap(cap)?;
            if let Some(code) = run.process.wait(WORKER_TICK_MS)? {
                run.report_b(folder);
                return Ok(code);
            }
            let now = synapse_capture::now_hns();
            let cpu = run.process.cpu_hns();
            let worker_share = share_percent(cpu - cpu_before, now - tick_started, run.threads);
            let sum = worker_share + ProcessorBudget::global().live_share();
            run.max_worker_share = run.max_worker_share.max(worker_share);
            run.max_sum = run.max_sum.max(sum);
            if sum > CPU_LIMIT_PERCENT {
                run.seconds_over_limit += 1;
            }
            cpu_before = cpu;
            tick_started = now;
        }
    }

    /// Condition B: the live share, the worker cap and the worker's share
    /// against 10 % in every second, and the time until ready.
    fn report_b(&self, folder: &Path) {
        let report = BReport::Worker {
            max_worker_share_percent: self.max_worker_share,
            max_sum_percent: self.max_sum,
            seconds_over_limit: self.seconds_over_limit,
            worker_seconds: self.started.elapsed().as_secs_f64(),
            limit_percent: CPU_LIMIT_PERCENT,
        };
        let _ = ClipJson::update(folder, |clip| clip.conditions.merge_b(report));
    }
}

/// The child process, its thread and its job (kill-on-close, so the worker
/// ends with the service; B10 resumes the work).
#[cfg(windows)]
struct WorkerProcess {
    process: windows::Win32::Foundation::HANDLE,
    job: windows::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
impl WorkerProcess {
    fn spawn(folder: &Path) -> Result<Self, String> {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::JobObjects::AssignProcessToJobObject;
        use windows::Win32::System::Threading::{
            BELOW_NORMAL_PRIORITY_CLASS, CREATE_NO_WINDOW, CREATE_SUSPENDED,
            CREATE_UNICODE_ENVIRONMENT, CreateProcessW, PROCESS_INFORMATION, ResumeThread,
            STARTUPINFOW,
        };
        use windows::core::{PCWSTR, PWSTR};

        let exe = std::env::current_exe()
            .map_err(|error| format!("could not resolve the current executable: {error}"))?;
        let command_line = [
            exe.to_string_lossy().into_owned(),
            "--mode".to_owned(),
            "clip-worker".to_owned(),
            "--clip-worker-folder".to_owned(),
            folder.to_string_lossy().into_owned(),
        ]
        .iter()
        .map(|arg| crate::desktop_worker::quote_windows_arg(arg))
        .collect::<Vec<_>>()
        .join(" ");
        let mut command_line_wide = crate::desktop_worker::wide_null(&command_line);
        let job = crate::desktop_worker::create_worker_kill_on_close_job()?;
        let startup_info = STARTUPINFOW {
            cb: u32::try_from(std::mem::size_of::<STARTUPINFOW>()).unwrap_or(u32::MAX),
            ..Default::default()
        };
        let mut info = PROCESS_INFORMATION::default();
        // SAFETY: a writable command line, a sized STARTUPINFOW and one out-parameter.
        let created = unsafe {
            CreateProcessW(
                PCWSTR::null(),
                Some(PWSTR(command_line_wide.as_mut_ptr())),
                None,
                None,
                false,
                CREATE_NO_WINDOW
                    | CREATE_SUSPENDED
                    | CREATE_UNICODE_ENVIRONMENT
                    | BELOW_NORMAL_PRIORITY_CLASS,
                None,
                PCWSTR::null(),
                &raw const startup_info,
                &raw mut info,
            )
        };
        if let Err(error) = created {
            // SAFETY: the job handle was created above and is closed once.
            let _ = unsafe { CloseHandle(job) };
            return Err(format!("could not start the clip worker: {error}"));
        }
        let worker = Self {
            process: info.hProcess,
            job,
        };
        // SAFETY: both handles are valid; the process is still suspended.
        let assigned = unsafe { AssignProcessToJobObject(job, info.hProcess) };
        let capped = worker.set_cap(ProcessorBudget::global().worker_cap());
        // SAFETY: the primary thread handle of the suspended child.
        let resumed = unsafe { ResumeThread(info.hThread) };
        // SAFETY: the thread handle is no longer needed and closed once.
        let _ = unsafe { CloseHandle(info.hThread) };
        assigned.map_err(|error| format!("could not put the clip worker in its job: {error}"))?;
        capped?;
        if resumed == u32::MAX {
            return Err("could not resume the clip worker".to_owned());
        }
        tracing::info!(code = "CLIP_WORKER_STARTED", pid = info.dwProcessId, folder = %folder.display());
        Ok(worker)
    }

    /// The job's hard processor cap, in percent of the whole machine. At zero
    /// the worker waits (the smallest rate Windows takes).
    fn set_cap(&self, percent: f64) -> Result<(), String> {
        use windows::Win32::System::JobObjects::{
            JOB_OBJECT_CPU_RATE_CONTROL_ENABLE, JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP,
            JOBOBJECT_CPU_RATE_CONTROL_INFORMATION, JOBOBJECT_CPU_RATE_CONTROL_INFORMATION_0,
            JobObjectCpuRateControlInformation, SetInformationJobObject,
        };

        // CpuRate is in hundredths of a percent, 1..=10000.
        let rate = (percent * 100.0).round().clamp(1.0, 10_000.0) as u32;
        let info = JOBOBJECT_CPU_RATE_CONTROL_INFORMATION {
            ControlFlags: JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP,
            Anonymous: JOBOBJECT_CPU_RATE_CONTROL_INFORMATION_0 { CpuRate: rate },
        };
        let size = u32::try_from(std::mem::size_of_val(&info)).unwrap_or(u32::MAX);
        // SAFETY: a valid job handle and a sized information block.
        unsafe {
            SetInformationJobObject(
                self.job,
                JobObjectCpuRateControlInformation,
                (&raw const info).cast(),
                size,
            )
        }
        .map_err(|error| format!("could not set the clip worker's processor cap: {error}"))
    }

    /// Processor time used by every process of the job, in 100 ns units.
    fn cpu_hns(&self) -> i64 {
        use windows::Win32::System::JobObjects::{
            JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JobObjectBasicAccountingInformation,
            QueryInformationJobObject,
        };

        let mut info = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        let size = u32::try_from(std::mem::size_of_val(&info)).unwrap_or(u32::MAX);
        // SAFETY: a valid job handle and a sized out-parameter.
        let read = unsafe {
            QueryInformationJobObject(
                Some(self.job),
                JobObjectBasicAccountingInformation,
                (&raw mut info).cast(),
                size,
                None,
            )
        };
        if read.is_err() {
            return 0;
        }
        info.TotalUserTime.saturating_add(info.TotalKernelTime)
    }

    /// Waits up to `ms` for the worker to end; its exit code once it has.
    fn wait(&self, ms: u64) -> Result<Option<u8>, String> {
        use windows::Win32::Foundation::WAIT_OBJECT_0;
        use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};

        // SAFETY: a valid process handle.
        let waited =
            unsafe { WaitForSingleObject(self.process, u32::try_from(ms).unwrap_or(u32::MAX)) };
        if waited != WAIT_OBJECT_0 {
            return Ok(None);
        }
        let mut code = 0_u32;
        // SAFETY: a valid process handle and one out-parameter.
        unsafe { GetExitCodeProcess(self.process, &raw mut code) }
            .map_err(|error| format!("could not read the clip worker's exit code: {error}"))?;
        Ok(Some(u8::try_from(code).unwrap_or(u8::MAX)))
    }
}

#[cfg(windows)]
impl Drop for WorkerProcess {
    fn drop(&mut self) {
        use windows::Win32::Foundation::CloseHandle;
        // SAFETY: both handles are owned here and closed once; closing the
        // kill-on-close job ends a worker that is still running.
        unsafe {
            let _ = CloseHandle(self.process);
            let _ = CloseHandle(self.job);
        }
    }
}
