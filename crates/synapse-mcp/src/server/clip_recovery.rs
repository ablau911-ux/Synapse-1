//! Recovery of clip folders whose recorder died mid-way (plan 5.0 §3, W5).
//!
//! A clip whose recorder no longer holds its lock and whose `clip.json`
//! still says `recording` is made readable: every IVF part is cut to its last
//! complete picture and gets its picture count; every complete part and
//! every still the index names is kept; index lines whose picture did not
//! reach the disk, and the instants never written up to the planned end,
//! are named lost (`service_restart`), each once. The clip is then `ready`.

use std::collections::BTreeMap;
use std::path::Path;

use super::clip_folder::{
    ClipJson, ClipRegistry, ClipState, ConditionStatus, FRAMES_FILE, IndexLine,
    LOST_SERVICE_RESTART, LostInstant, PartInfo, RecorderLock, STILLS_DIR, StillInfo, StopReason,
    part_path, read_index, repair_ivf, still_path, write_bytes_atomically,
};

const MILLISECONDS_PER_SECOND: u64 = 1_000;

pub(super) struct ClipRecovery;

impl ClipRecovery {
    pub(super) fn run(folder: &Path) -> Result<(), String> {
        // The recorder lock is held across recovery, so a second recovery waits out.
        if RecorderLock::held(folder) {
            return Ok(());
        }
        let _lock = RecorderLock::acquire(folder)?;
        let clip = ClipJson::read(folder)?;
        if clip.state != ClipState::Recording {
            return Ok(());
        }
        if clip.fps == 0 {
            return Err("recording has zero fps".into());
        }
        let complete = Self::repair_parts(folder)?;
        let read = read_index(folder).map_err(|error| error.to_string())?;
        let planned = clip.end_ms * u64::from(clip.fps) / MILLISECONDS_PER_SECOND;
        let lines = Self::repair_index(&read, &complete, planned, clip.fps);
        if lines != read {
            let mut text = String::new();
            for line in &lines {
                text.push_str(&line.text()?);
            }
            write_bytes_atomically(&folder.join(FRAMES_FILE), text.as_bytes())?;
        }
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

    /// The index after the recorder's death: in order from instant 0 with no
    /// gap; an `enc` line whose picture is not complete on disk, and a `same`
    /// line after a lost one, become lost; the instants never written, up to
    /// the planned end, are added as lost.
    fn repair_index(
        read: &[IndexLine],
        complete: &BTreeMap<u32, u32>,
        planned: u64,
        fps: u32,
    ) -> Vec<IndexLine> {
        let lost_line = |instant: u64| IndexLine {
            instant,
            t_ms: instant * MILLISECONDS_PER_SECOND / u64::from(fps),
            enc: None,
            same: None,
            lost: Some(LOST_SERVICE_RESTART.to_owned()),
            still: None,
        };
        let mut lines: Vec<IndexLine> = Vec::with_capacity(read.len());
        for (expected, line) in (0_u64..).zip(read) {
            if line.instant != expected {
                break;
            }
            let picture_missing = line
                .enc
                .is_some_and(|(part, index)| complete.get(&part).is_none_or(|&held| index >= held));
            let previous_lost = lines.last().is_some_and(|before| before.lost.is_some());
            let shows_missing = line.same.is_some() && previous_lost;
            if picture_missing || shows_missing {
                lines.push(lost_line(line.instant));
            } else {
                lines.push(line.clone());
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

    /// The stills the index names; a still file without its index line (its
    /// line had not been written when the recorder died) is removed, so the
    /// folder holds exactly the files of plan §3.
    fn stills(folder: &Path, lines: &[IndexLine]) -> Result<Vec<StillInfo>, String> {
        let mut stills = Vec::new();
        for line in lines.iter().filter(|line| line.still == Some(true)) {
            let path = still_path(folder, line.instant);
            if let Ok(metadata) = std::fs::metadata(&path) {
                stills.push(StillInfo::new(folder, line.instant, metadata.len()));
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

pub(crate) fn recover_clips_on_start() {
    let started = std::thread::Builder::new()
        .name("synapse-clip-recovery".into())
        .spawn(|| {
            for folder in ClipRegistry::all_folders() {
                if let Err(error) = ClipRecovery::run(&folder) {
                    tracing::warn!(code = "CLIP_RECOVERY_FAILED", folder = %folder.display(), error = %error);
                }
            }
        });
    if let Err(error) = started {
        tracing::warn!(code = "CLIP_RECOVERY_THREAD_FAILED", error = %error);
    }
}
