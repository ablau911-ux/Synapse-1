//! exp-av1-recover: run the PRODUCT's recovery (`crate::clip_recovery::ClipRecovery::run`,
//! the entry point the server uses for one clip folder whose recorder died) on a folder in
//! place, then report what the folder holds.
//!
//!   clip-probe exp-av1-recover folder=<clip folder>
//!
//! Prints one JSON line: ok, error, finished (recovery's answer: the clip needs no more
//! recovery), state, stop_reason, instants (from clip.json after),
//! index_lines, lost, lost_causes (from frames.jsonl after), stills_on_disk (instants),
//! parts_on_disk (part numbers). A recovery error is reported with ok=false and the run
//! still exits 0: the check judges it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::json;

use crate::Result;
use crate::clip_folder::{ClipJson, STILLS_DIR, VIDEO_DIR, read_index};
use crate::clip_recovery::ClipRecovery;
use crate::exp_meter::Args;

const STILL_PREFIX: &str = "still-";
const STILL_SUFFIX: &str = ".webp";
const PART_PREFIX: &str = "part-";
const PART_SUFFIX: &str = ".ivf";

pub fn run() -> Result<()> {
    let args = Args::from_env();
    let folder = PathBuf::from(args.text("folder")?);
    let outcome = ClipRecovery::run(&folder);
    let clip = ClipJson::read(&folder).ok();
    let lines = read_index(&folder).unwrap_or_default();
    let lost_causes: BTreeSet<&str> = lines.iter().filter_map(|line| line.lost.as_deref()).collect();
    let line = json!({
        "ok": outcome.is_ok(),
        "finished": outcome.as_ref().ok(),
        "error": outcome.err(),
        "state": clip.as_ref().map(|clip| clip.state.as_str()),
        "stop_reason": clip.as_ref().and_then(|clip| clip.stop_reason).map(|reason| reason.as_str()),
        "instants": clip.as_ref().map(|clip| clip.instants),
        "index_lines": lines.len(),
        "lost": lines.iter().filter(|line| line.lost.is_some()).count(),
        "lost_causes": lost_causes,
        "stills_on_disk": numbered_files(&folder.join(STILLS_DIR), STILL_PREFIX, STILL_SUFFIX),
        "parts_on_disk": numbered_files(&folder.join(VIDEO_DIR), PART_PREFIX, PART_SUFFIX)
    });
    println!("{line}");
    Ok(())
}

/// The numbers of the files `<prefix><number><suffix>` in `directory`, in order.
fn numbered_files(directory: &Path, prefix: &str, suffix: &str) -> Vec<u64> {
    let mut numbers: Vec<u64> = std::fs::read_dir(directory)
        .map(|entries| {
            entries
                .filter_map(std::result::Result::ok)
                .filter_map(|entry| {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    name.strip_prefix(prefix)?
                        .strip_suffix(suffix)?
                        .parse()
                        .ok()
                })
                .collect()
        })
        .unwrap_or_default();
    numbers.sort_unstable();
    numbers
}
