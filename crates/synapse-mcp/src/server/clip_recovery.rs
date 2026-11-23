//! Recovery of recording folders whose recorder no longer holds its lock.
use std::collections::BTreeMap;
use std::path::Path;

use super::clip_exact::{EXACT_GROUP_PICTURES, ExactSummary};
use super::clip_folder::{
    ClipJson, ClipRegistry, ClipState, CommittedSegment, ConditionStatus, FRAMES_FILE, LossCause,
    LostInstant, RecorderLock, StopReason, read_frame_lines,
};
use super::clip_moving::MovingSegment;

const MILLISECONDS_PER_SECOND: u64 = 1_000;

pub(super) struct ClipRecovery;
impl ClipRecovery {
    pub(super) fn run(folder: &Path) -> Result<(), String> {
        // Acquire the recorder lock across recovery, preventing a second recovery.
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
        // A crash before the first picture can precede creation of the index.
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(folder.join(FRAMES_FILE))
            .map_err(|e| e.to_string())?;
        let lines = read_frame_lines(folder).map_err(|e| e.to_string())?;
        let mut lost = lines
            .iter()
            .filter_map(|l| {
                l.lost.map(|cause| LostInstant {
                    instant: l.instant,
                    cause,
                })
            })
            .collect::<Vec<_>>();
        let mut groups = BTreeMap::new();
        for line in &lines {
            if let Some(reference) = line.exact {
                groups
                    .entry(reference.group)
                    .or_insert_with(Vec::new)
                    .push((line.instant, reference));
            }
        }
        let mut totals = ExactSummary::default();
        let last_group = groups.keys().next_back().copied();
        for (group, records) in groups {
            let path = folder.join(format!("frames/group-{group:06}.zst"));
            let size = std::fs::metadata(&path).map_or(0, |m| m.len());
            let mut end = 0;
            let mut valid = 0;
            for (instant, reference) in &records {
                let record_end = reference.offset.saturating_add(u64::from(reference.bytes));
                if record_end <= size {
                    end = end.max(record_end);
                    valid += 1;
                } else {
                    lost.push(LostInstant {
                        instant: *instant,
                        cause: LossCause::ServiceRestart,
                    });
                }
            }
            if valid > 0 {
                let open =
                    Some(group) == last_group && records.len() < EXACT_GROUP_PICTURES as usize;
                if open {
                    std::fs::OpenOptions::new()
                        .write(true)
                        .open(&path)
                        .and_then(|f| f.set_len(end))
                        .map_err(|e| e.to_string())?;
                }
                totals.groups += 1;
                totals.records += valid;
                totals.bytes += if open { end } else { size };
            }
        }
        let mut intervals = BTreeMap::new();
        for line in &lines {
            if let Some(reference) = line.moving {
                intervals
                    .entry(reference.segment)
                    .or_insert_with(Vec::new)
                    .push(line);
            }
        }
        let mut recovered = Vec::new();
        for (index, interval) in intervals {
            if clip
                .moving
                .segments
                .iter()
                .any(|s| s.segment.index == index)
            {
                continue;
            }
            let first = interval[0];
            let expected = interval
                .iter()
                .filter_map(|l| l.moving.map(|r| r.frame + 1))
                .max()
                .unwrap_or(0);
            let segment = match first.moving_rect {
                Some(rect) => {
                    MovingSegment::from_file(folder, index, rect, first.instant, expected)?
                }
                None => None,
            };
            let held = segment.as_ref().map_or(0, |s| s.frames);
            for line in &interval {
                if line.moving.is_some_and(|r| r.frame >= held) {
                    lost.push(LostInstant {
                        instant: line.instant,
                        cause: LossCause::ServiceRestart,
                    });
                }
            }
            if let Some(mut segment) = segment {
                // Recovery cannot attest that the interval ended normally.
                segment.complete = false;
                recovered.push(CommittedSegment::read_back(folder, segment)?);
            }
        }
        if let Ok(entries) = std::fs::read_dir(folder.join("moving")) {
            for entry in entries {
                let path = entry.map_err(|e| e.to_string())?.path();
                if path
                    .extension()
                    .is_some_and(|extension| extension == "part")
                {
                    std::fs::remove_file(path).map_err(|e| e.to_string())?;
                }
            }
        }
        let recorded = lines.last().map_or(0, |l| l.instant + 1);
        let planned = clip.end_ms * u64::from(clip.fps) / MILLISECONDS_PER_SECOND;
        lost.extend((recorded..planned).map(|instant| LostInstant {
            instant,
            cause: LossCause::ServiceRestart,
        }));
        ClipJson::update(folder, |clip| -> Result<(), String> {
            clip.moving.segments.extend(recovered);
            clip.moving.segments.sort_by_key(|s| s.segment.index);
            clip.moving.status = if clip.moving.segments.is_empty() { "not_used" } else { "available" }.into();
            clip.exact = Some(totals.into());
            clip.recorded_ms = recorded * MILLISECONDS_PER_SECOND / u64::from(clip.fps);
            clip.state = ClipState::Ready;
            clip.stop_reason = Some(StopReason::ServiceRestart);
            clip.conditions.c.get_or_insert_with(|| ConditionStatus { status: "held".into(), ..ConditionStatus::default() });
            clip.conditions.t = Some(ConditionStatus { status: "held".into(), failed_instants: Vec::new(),
                detail: Some(serde_json::json!({ "length_ms": clip.recorded_ms, "bytes": 0, "instants_lost": 0, "stop_reason": "service_restart" })) });
            clip.add_lost(lost);
            clip.set_total_bytes(folder)
        }).and_then(|(_, result)| result)
    }
}

pub(crate) fn recover_clips_on_start() {
    let started = std::thread::Builder::new().name("synapse-clip-recovery".into()).spawn(|| {
        for folder in ClipRegistry::all_folders() {
            if let Err(error) = ClipRecovery::run(&folder) { tracing::warn!(code = "CLIP_RECOVERY_FAILED", folder = %folder.display(), error = %error); }
        }
    });
    if let Err(error) = started {
        tracing::warn!(code = "CLIP_RECOVERY_THREAD_FAILED", error = %error);
    }
}
