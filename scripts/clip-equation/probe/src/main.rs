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
//!
//! Windows are only ever the probe's own; see `probe_window.rs`.

mod clock;
mod delivery;
mod disk;
mod encoder;
mod gpu;
mod hash_frames;
mod info;
mod load;
mod newframes;
mod observe;
mod observer;
mod probe_window;
mod readback;
mod scratch;
mod slot_copy;
mod slots;
mod stats;
mod threading;

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
        _ => Err(format!(
            "unknown command '{command}'; use info, readback, observe, delivery, newframes, disk, threading, lateness or encoder"
        )),
    };
    if let Err(error) = outcome {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}
