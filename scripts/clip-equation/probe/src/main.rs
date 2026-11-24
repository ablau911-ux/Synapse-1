//! Package 0 of the clip recorder (docs/clip-recorder-plan.md, section 0):
//! measurement probes. Each subcommand runs one measurement on this PC and
//! prints its figures as plain `key=value` lines. Nothing here is product code.
//!
//!   clip-probe info        PC facts and the processor-time clock check
//!   clip-probe readback    M1  GPU read-back cost at three sizes, on every adapter
//!   clip-probe observe     M2  observer cost per delivered frame (no read-back)
//!   clip-probe delivery    M3  delivery at Default, 16.7 ms and 11 ms requested interval
//!   clip-probe newframes   M4  new-frame rate: idle, caret, typing, mouse movement
//!   clip-probe disk        M5  raw write rate and its processor time
//!   clip-probe threading   M6  blocking Map against "do not wait" Map with retry
//!   clip-probe lateness    M7  selection lateness, idle and under processor load
//!   clip-probe encoder     M8  Media Foundation H.264 settings set and read back
//!   clip-probe compress    M10 to M14 (package 0b) compression of the waiting frames
//!   clip-probe exp-encoders  experiments step 1a: every H.264, HEVC and AV1 encoder
//!   clip-probe exp-corpus    experiments step 1b: the fixed lossless frame corpus
//!   clip-probe exp-z         experiments steps 2 and 3, way Z: one zstd way on one corpus folder
//!   clip-probe exp-av1       replays a corpus scene through the product's recorder
//!                            (clip_stream.rs): out=<dir> folder=<corpus scene>
//!   clip-probe exp-av1-fetch fetches one instant's picture through the product's decoder:
//!                            folder=<clip folder> instant=<k> [png=<file>]
//!   clip-probe exp-av1-recover runs the product's recovery on a clip folder in place:
//!                            folder=<clip folder>
//!
//! Windows are only ever the probe's own; see `probe_window.rs` and, for
//! package 0b, `chrome_window.rs`.

mod block_coders;
mod chrome_window;
// The product's clip recorder and decoder, so the bench runs the app's own code.
#[allow(dead_code)]
#[path = "../../../../crates/synapse-mcp/src/server/clip_decoder.rs"]
mod clip_decoder;
#[allow(dead_code)]
#[path = "../../../../crates/synapse-mcp/src/server/clip_encoder.rs"]
mod clip_encoder;
#[allow(dead_code)]
#[path = "../../../../crates/synapse-mcp/src/server/clip_folder.rs"]
mod clip_folder;
#[allow(dead_code)]
#[path = "../../../../crates/synapse-mcp/src/server/clip_recovery.rs"]
mod clip_recovery;
#[allow(dead_code)]
#[path = "../../../../crates/synapse-mcp/src/server/clip_stream.rs"]
mod clip_stream;
mod clock;
mod compress;
mod delivery;
mod disk;
mod encoder;
mod exp_corpus;
mod exp_av1;
mod exp_av1_fetch;
mod exp_av1_recover;
mod exp_cpu;
mod exp_d;
mod exp_dyn;
mod exp_encoders;
#[allow(dead_code)]
mod exp_measure;
mod exp_meter;
#[allow(dead_code)]
mod exp_settings;
mod exp_z;
mod gpu;
mod hash_frames;
mod info;
mod load;
mod newframes;
mod nv12_copy;
mod observe;
mod observer;
mod probe_window;
mod readback;
mod scratch;
mod slot_copy;
mod slots;
mod stats;
mod threading;
mod waiting_codec;

use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};

/// Every probe reports failures as text.
pub type Result<T> = std::result::Result<T, String>;

/// Maps a Windows error to text naming the step that failed.
pub fn win(step: &'static str) -> impl Fn(windows::core::Error) -> String {
    move |error| format!("{step} failed: {error}")
}

fn main() {
    // Physical pixels everywhere: window sizes and captured frame sizes agree.
    // SAFETY: called once, before any window exists.
    let _ = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let command = std::env::args().nth(1).unwrap_or_default();
    let outcome = match command.as_str() {
        "info" => info::run(),
        "readback" => readback::run(),
        "observe" => observe::run(),
        "delivery" => delivery::run(),
        "newframes" => newframes::run(),
        "disk" => disk::run(),
        "threading" => threading::run_threading(),
        "lateness" => threading::run_lateness(),
        "encoder" => encoder::run(),
        "compress" => compress::run(),
        "exp-encoders" => exp_encoders::run(),
        "exp-corpus" => exp_corpus::run(),
        "exp-z" => exp_z::run(),
        "exp-d" => exp_d::run(),
        "exp-rate" => exp_meter::run_rate(),
        "exp-av1" => exp_av1::run(),
        "exp-av1-fetch" => exp_av1_fetch::run(),
        "exp-av1-recover" => exp_av1_recover::run(),
        _ => Err(format!(
            "unknown command '{command}'; use info, readback, observe, delivery, newframes, disk, threading, lateness, encoder, compress, exp-encoders, exp-corpus, exp-z, exp-d, exp-rate, exp-av1, exp-av1-fetch or exp-av1-recover"
        )),
    };
    if let Err(error) = outcome {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}
