//! exp-av1-registry: runs one change of the PRODUCT's clip registry
//! (`crate::clip_folder::ClipRegistry`, the list under the default clips folder,
//! `%LOCALAPPDATA%\synapse\clips\active-clips.json`). The bench sets LOCALAPPDATA to a
//! folder of its own, so the user's registry is never touched.
//!
//!   clip-probe exp-av1-registry action=mark_finished clip=<clip id>
//!   clip-probe exp-av1-registry action=recover   (the recovery the service runs at start)
//!   clip-probe exp-av1-registry action=remove clip=<clip id> root_created=<true|false>
//!     (what a `record` that failed before its first picture does with its registry entry;
//!     root_created: that start created the default clips folder)
//!
//! Prints one JSON line: ok, error. An error of the product is reported with ok=false and
//! the run still exits 0: the check judges it.

use serde_json::json;

use crate::Result;
use crate::clip_folder::ClipRegistry;
use crate::clip_recovery::ClipRecovery;
use crate::exp_meter::Args;

pub fn run() -> Result<()> {
    let args = Args::from_env();
    let outcome = match args.text("action")? {
        "mark_finished" => ClipRegistry::mark_finished(args.text("clip")?),
        "recover" => ClipRecovery::recover_registered(),
        "remove" => ClipRegistry::remove(args.text("clip")?, args.text("root_created")? == "true"),
        other => {
            return Err(format!(
                "unknown action={other}; use mark_finished, recover or remove"
            ));
        }
    };
    let line = json!({ "ok": outcome.is_ok(), "error": outcome.err() });
    println!("{line}");
    Ok(())
}
