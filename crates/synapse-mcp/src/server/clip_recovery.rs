//! Recovery of clip folders whose recorder died mid-way (plan 5.0 §3, W5).
//!
//! A clip whose recorder no longer holds its lock and whose `clip.json`
//! still says `recording` is made readable: every IVF part is cut to its last
//! complete picture and gets its picture count (a part killed inside its
//! header holds none and is removed); every complete part and every still
//! the index names is kept; index lines whose video picture did not reach the
//! disk, and the instants never written up to the planned end, are named lost
//! (`service_restart`), each once. A lost line keeps its still mark while
//! the still's file is on disk: a complete still is never deleted for a
//! missing video picture, and a mark without a file is dropped. The clip is
//! then `ready`. A recording that fails gets the same index rule at once
//! (`reconcile_failed`, with the cause of its failure, `lost_cause` in
//! `clip.json`); recovery applies it again to every `failed` clip, with that
//! cause, which stays `failed`, in case that reconcile failed or never ran
//! (a recorder thread that ended without finishing).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::clip_decoder::decode_still_bytes;
use super::clip_folder::{
    CLIP_JSON_FILE, ClipJson, ClipRegistry, ClipState, ConditionStatus, FRAMES_FILE, IndexLine,
    LOST_SERVICE_RESTART, LostInstant, MAX_DURATION_MS, PartInfo, RECORD_FPS, RecorderLock,
    STILLS_DIR, StillInfo, StopReason, part_path, read_index, repair_ivf, retry_brief_hold,
    still_path, write_bytes_atomically,
};

const MILLISECONDS_PER_SECOND: u64 = 1_000;

pub(super) struct ClipRecovery;

impl ClipRecovery {
    /// Recovers one clip folder. Returns `true` once the clip needs no more
    /// recovery (recovered now, `ready`, `failed` with an index that agrees,
    /// of another format, or without `clip.json`), `false` while a recorder
    /// holds it.
    pub(super) fn run(folder: &Path) -> Result<bool, String> {
        // The recorder lock is held across recovery, so a second recovery waits
        // out. The recorder takes it before the clip is registered.
        if RecorderLock::held(folder) {
            return Ok(false);
        }
        // A start that ended before its clip.json existed recorded nothing.
        if !folder.join(CLIP_JSON_FILE).is_file() {
            return Ok(true);
        }
        // A clip of another (older) format is no recording of this recorder:
        // it needs no recovery and is skipped quietly, before anything is
        // written in its folder.
        if let Some(format) = ClipJson::other_format(folder)? {
            tracing::debug!(code = "CLIP_RECOVERY_SKIPPED_FORMAT", folder = %folder.display(), format = %format);
            return Ok(true);
        }
        let _lock = RecorderLock::acquire(folder)?;
        let clip = ClipJson::read(folder)?;
        match clip.state {
            ClipState::Recording => {}
            // Its reconcile at failure can itself have failed
            // (CAPTURE_CLIP_RECONCILE_FAILED): the same rule again. An index
            // that already agrees is left as it is, and clip.json is written
            // only when it does not list a lost instant yet.
            ClipState::Failed => {
                let lost = Self::reconcile_failed(folder, clip.fps, clip.failed_lost_cause())?;
                if lost.iter().all(|lost| clip.lists_lost(lost.instant)) {
                    return Ok(true);
                }
                return ClipJson::update(folder, |clip| clip.add_lost(lost)).map(|_| true);
            }
            ClipState::Ready => return Ok(true),
        }
        // Instant k is recorded when k / f < end: ceil(end * f / 1000) instants.
        // The recorder makes no clip longer than MAX_DURATION_MS at RECORD_FPS,
        // so a plan beyond that (a clip made before the limit existed, or a
        // damaged clip.json) is capped there: what is on disk is recovered and
        // no lost line is made beyond the longest recording. A plan whose
        // product overflows is beyond it too.
        let longest = MAX_DURATION_MS * u64::from(RECORD_FPS) / MILLISECONDS_PER_SECOND;
        let planned = clip
            .end_ms
            .checked_mul(u64::from(clip.fps))
            .map_or(longest, |scaled| {
                scaled.div_ceil(MILLISECONDS_PER_SECOND).min(longest)
            });
        let (lines, complete) =
            Self::reconcile(folder, Some(planned), clip.fps, LOST_SERVICE_RESTART)?;
        let parts = Self::parts(folder, &lines, &complete)?;
        let stills = Self::stills(folder, &lines)?;
        let lost: Vec<LostInstant> = lines
            .iter()
            .filter_map(|line| {
                line.lost.as_ref().map(|cause| LostInstant {
                    instant: line.instant,
                    cause: cause.clone(),
                })
            })
            .collect();
        let encoded = lines.iter().filter(|line| line.enc.is_some()).count();
        let same = lines.iter().filter(|line| line.same.is_some()).count();
        let instants = lines.len() as u64;
        ClipJson::update(folder, |clip| -> Result<(), String> {
            clip.parts = parts;
            clip.stills = stills;
            clip.instants = instants;
            clip.recorded_ms = instants * MILLISECONDS_PER_SECOND / u64::from(clip.fps);
            clip.state = ClipState::Ready;
            clip.stop_reason = Some(StopReason::ServiceRestart);
            clip.conditions
                .capture
                .get_or_insert_with(|| ConditionStatus::held(serde_json::json!({})));
            clip.conditions.totals = Some(ConditionStatus::held(serde_json::json!({
                "length_ms": clip.recorded_ms, "instants": instants, "encoded": encoded,
                "same": same, "lost": lost.len(), "stop_reason": StopReason::ServiceRestart.as_str()
            })));
            clip.add_lost(lost);
            clip.set_total_bytes(folder)
        })
        .and_then(|(_, result)| result)
        .map(|()| true)
    }

    /// Makes the index of a recording that failed (its encoder stopped)
    /// agree with the pictures on disk, before the clip is published
    /// `failed`, and again on recovery of a failed clip: every line whose video
    /// picture did not reach the disk is named lost (`cause`), by the rule
    /// recovery uses. No instant is added. Returns every lost instant of the
    /// index, for `clip.json`.
    pub(super) fn reconcile_failed(
        folder: &Path,
        fps: u32,
        cause: &str,
    ) -> Result<Vec<LostInstant>, String> {
        let (lines, _) = Self::reconcile(folder, None, fps, cause)?;
        Ok(lines
            .iter()
            .filter_map(|line| {
                line.lost.as_ref().map(|cause| LostInstant {
                    instant: line.instant,
                    cause: cause.clone(),
                })
            })
            .collect())
    }

    /// Repairs the parts, then rewrites the index (when it changes) by
    /// `repair_index`; `planned` instants are filled up with lost lines.
    /// Returns the index and each part's count of complete pictures. `fps` is
    /// never 0: `ClipJson::read` refuses such a clip, and the recorder passes
    /// `RECORD_FPS`.
    fn reconcile(
        folder: &Path,
        planned: Option<u64>,
        fps: u32,
        cause: &str,
    ) -> Result<(Vec<IndexLine>, BTreeMap<u32, u32>), String> {
        let read = read_index(folder).map_err(|error| error.to_string())?;
        let planned = planned.unwrap_or(read.len() as u64);
        // Each named still is read once, before anything is written: a still
        // that cannot be read stops recovery of this clip unchanged.
        let named: BTreeSet<u64> = read.iter().filter_map(IndexLine::still_file).collect();
        let mut on_disk = BTreeSet::new();
        for instant in named {
            if Self::still_on_disk(&still_path(folder, instant))? {
                on_disk.insert(instant);
            }
        }
        let complete = Self::repair_parts(folder)?;
        let lines = Self::repair_index(&read, &complete, planned, fps, cause, |instant| {
            on_disk.contains(&instant)
        });
        if lines != read {
            let mut text = String::new();
            for line in &lines {
                text.push_str(&line.text()?);
            }
            write_bytes_atomically(&folder.join(FRAMES_FILE), text.as_bytes())?;
        }
        Ok((lines, complete))
    }

    /// Whether a still is on disk: `false` when its file is absent, or was
    /// read in full and does not decode (a power loss left it empty or cut).
    /// A file that cannot be opened or read (another process holds it past
    /// the brief-hold retries) is an error, never a missing still.
    fn still_on_disk(path: &Path) -> Result<bool, String> {
        match retry_brief_hold(|| std::fs::read(path)) {
            Ok(bytes) => Ok(decode_still_bytes(&bytes, path).is_ok()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(format!("could not read {}: {error}", path.display())),
        }
    }

    /// Repairs every part file `video/part-NNNN.ivf`, numbered from 0000;
    /// returns each part's count of complete pictures.
    fn repair_parts(folder: &Path) -> Result<BTreeMap<u32, u32>, String> {
        let mut complete = BTreeMap::new();
        let mut part = 0;
        while part_path(folder, part).is_file() {
            let (pictures, _) = repair_ivf(&part_path(folder, part))?;
            complete.insert(part, pictures);
            part += 1;
        }
        Ok(complete)
    }

    /// The index after the recorder's death or failure: in order from
    /// instant 0 with no gap; an `enc` line whose picture is not complete on
    /// disk, and a `same` line after a line that shows no picture, become
    /// lost (`cause`); the instants never written, up to `planned`, are added
    /// as lost. Every line keeps its still mark (`still` or `still_of`) only
    /// while that still is on disk (`still_on_disk`): a lost line, one that
    /// becomes lost here or was lost already (it keeps its own cause), keeps
    /// it, since the still shows the instant's exact picture and is never
    /// deleted for a missing video picture; a mark whose still is gone is
    /// dropped, so no line names a picture that cannot be shown.
    fn repair_index(
        read: &[IndexLine],
        complete: &BTreeMap<u32, u32>,
        planned: u64,
        fps: u32,
        cause: &str,
        still_on_disk: impl Fn(u64) -> bool,
    ) -> Vec<IndexLine> {
        let lost_line = |instant: u64| IndexLine {
            instant,
            t_ms: instant * MILLISECONDS_PER_SECOND / u64::from(fps),
            enc: None,
            same: None,
            lost: Some(cause.to_owned()),
            still: None,
            still_of: None,
            cursor: None,
        };
        let mut lines: Vec<IndexLine> = Vec::with_capacity(read.len());
        for (expected, line) in (0_u64..).zip(read) {
            if line.instant != expected {
                break;
            }
            let picture_missing = line
                .enc
                .is_some_and(|(part, index)| complete.get(&part).is_none_or(|&held| index >= held));
            // A lost line with a still still shows a picture: the one of its still.
            let previous_shows_nothing = lines
                .last()
                .is_some_and(|before| before.lost.is_some() && before.still_file().is_none());
            let shows_missing = line.same.is_some() && previous_shows_nothing;
            let still_kept = line.still_file().is_some_and(&still_on_disk);
            let (still, still_of) = (
                line.still.filter(|_| still_kept),
                line.still_of.filter(|_| still_kept),
            );
            if picture_missing || shows_missing || line.lost.is_some() {
                lines.push(IndexLine {
                    enc: None,
                    same: None,
                    lost: line.lost.clone().or_else(|| Some(cause.to_owned())),
                    still,
                    still_of,
                    ..line.clone()
                });
            } else {
                lines.push(IndexLine {
                    still,
                    still_of,
                    ..line.clone()
                });
            }
        }
        let written = lines.len() as u64;
        lines.extend((written..planned).map(lost_line));
        lines
    }

    /// The complete parts the index uses, read back; part files without one
    /// complete picture are removed.
    fn parts(
        folder: &Path,
        lines: &[IndexLine],
        complete: &BTreeMap<u32, u32>,
    ) -> Result<Vec<PartInfo>, String> {
        let mut instants: BTreeMap<u32, (u64, u64)> = BTreeMap::new();
        for line in lines {
            if let Some((part, _)) = line.enc {
                let span = instants.entry(part).or_insert((line.instant, line.instant));
                span.1 = line.instant;
            }
        }
        let mut parts = Vec::new();
        for (&part, &pictures) in complete {
            match instants.get(&part) {
                Some(&(first, last)) if pictures > 0 => {
                    parts.push(PartInfo::read_back(folder, part, first, last, pictures)?);
                }
                _ => {
                    let path = part_path(folder, part);
                    std::fs::remove_file(&path)
                        .map_err(|error| format!("could not remove {}: {error}", path.display()))?;
                }
            }
        }
        Ok(parts)
    }

    /// The stills the index names: each `still` line's own file and every
    /// file a `still_of` line points to. A still file the index does not name
    /// (its line had not been written when the recorder died, or the file
    /// does not decode and `repair_index` dropped its mark) is removed, so
    /// the folder holds exactly the files of plan §3.
    fn stills(folder: &Path, lines: &[IndexLine]) -> Result<Vec<StillInfo>, String> {
        let named: BTreeSet<u64> = lines.iter().filter_map(IndexLine::still_file).collect();
        let mut stills = Vec::new();
        for instant in named {
            let path = still_path(folder, instant);
            if let Ok(metadata) = std::fs::metadata(&path) {
                stills.push(StillInfo::new(folder, instant, metadata.len()));
            }
        }
        let directory = folder.join(STILLS_DIR);
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(stills),
            Err(error) => return Err(format!("could not list {}: {error}", directory.display())),
        };
        for entry in entries {
            let path = entry.map_err(|error| error.to_string())?.path();
            let named = stills
                .iter()
                .any(|still| still_path(folder, still.instant) == path);
            if !named && path.is_file() {
                std::fs::remove_file(&path)
                    .map_err(|error| format!("could not remove {}: {error}", path.display()))?;
            }
        }
        Ok(stills)
    }
}

impl ClipRecovery {
    /// Recovery at service start: the clips of the default folder that were
    /// never registered are registered, then every unfinished clip of the
    /// registry is recovered, and marked finished once it needs no more
    /// recovery.
    pub(super) fn recover_registered() {
        if let Err(error) = ClipRegistry::register_default_folder() {
            tracing::warn!(code = "CLIP_REGISTRY_SCAN_FAILED", error = %error);
        }
        // A registry that cannot be read is logged and nothing changes.
        let unfinished = match ClipRegistry::unfinished() {
            Ok(unfinished) => unfinished,
            Err(error) => {
                tracing::warn!(code = "CLIP_REGISTRY_READ_FAILED", error = %error);
                return;
            }
        };
        for entry in unfinished {
            let folder = &entry.folder;
            match Self::run(folder) {
                Ok(true) => {
                    if let Err(error) = ClipRegistry::mark_finished(&entry.clip_id) {
                        tracing::warn!(code = "CLIP_REGISTRY_WRITE_FAILED", folder = %folder.display(), error = %error);
                    }
                }
                Ok(false) => {}
                Err(error) => {
                    tracing::warn!(code = "CLIP_RECOVERY_FAILED", folder = %folder.display(), error = %error);
                }
            }
        }
    }
}

pub(crate) fn recover_clips_on_start() {
    let started = std::thread::Builder::new()
        .name("synapse-clip-recovery".into())
        .spawn(ClipRecovery::recover_registered);
    if let Err(error) = started {
        tracing::warn!(code = "CLIP_RECOVERY_THREAD_FAILED", error = %error);
    }
}
