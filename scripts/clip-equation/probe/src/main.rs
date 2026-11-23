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
//!
//! Windows are only ever the probe's own; see `probe_window.rs` and, for
//! package 0b, `chrome_window.rs`.

mod block_coders;
mod chrome_window;
#[allow(dead_code)]
#[path = "../../../../crates/synapse-mcp/src/server/clip_decoder.rs"]
mod clip_decoder;
#[allow(dead_code)]
#[path = "../../../../crates/synapse-mcp/src/server/clip_encoder.rs"]
mod clip_encoder;
#[allow(dead_code)]
#[path = "../../../../crates/synapse-mcp/src/server/clip_exact.rs"]
mod clip_exact;
#[allow(dead_code)]
#[path = "../../../../crates/synapse-mcp/src/server/clip_folder.rs"]
mod clip_folder;
#[allow(dead_code)]
#[path = "../../../../crates/synapse-mcp/src/server/clip_layers.rs"]
mod clip_layers;
#[allow(dead_code)]
#[path = "../../../../crates/synapse-mcp/src/server/clip_motion.rs"]
mod clip_motion;
#[allow(dead_code)]
#[path = "../../../../crates/synapse-mcp/src/server/clip_moving.rs"]
mod clip_moving;
#[allow(dead_code)]
#[path = "../../../../crates/synapse-mcp/src/server/clip_recovery.rs"]
mod clip_recovery;
mod clock;
mod compress;
mod delivery;
mod disk;
mod encoder;
mod exp_corpus;
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

/// R1 M1: measure six sequential hardware encoder starts in one process.
fn encoder_start() -> Result<()> {
    use clip_encoder::{ClipEncoder, MediaFoundationRuntime};
    use std::{collections::HashMap, path::Path, time::Instant};

    const FPS: u32 = 30;
    const FRAME_HNS: i64 = 10_000_000 / FPS as i64;
    const SIZES: [(u32, u32); 6] = [
        (64, 64),
        (640, 368),
        (1920, 1080),
        (64, 64),
        (1920, 1080),
        (640, 368),
    ];
    fn create(folder: &Path, index: usize) -> Result<ClipEncoder> {
        let (width, height) = SIZES[index];
        let started = Instant::now();
        let mut encoder = ClipEncoder::create(
            &folder.join(format!("start-{index}.mp4")),
            width,
            height,
            FPS,
        )?;
        let creation_ms = started.elapsed().as_secs_f64() * 1000.0;
        println!(
            "encoder={} width={width} height={height} creation_ms={creation_ms:.3}",
            index + 1
        );
        let mut frame = vec![128; (width * height * 3 / 2) as usize];
        frame[..(width * height) as usize].fill(16);
        for instant in 0..3 {
            encoder.write_nv12(&frame, instant * FRAME_HNS, FRAME_HNS)?;
        }
        Ok(encoder)
    }

    let args: HashMap<String, String> = std::env::args()
        .skip(2)
        .filter_map(|arg| {
            arg.split_once('=')
                .map(|(key, value)| (key.to_owned(), value.to_owned()))
        })
        .collect();
    let keep = match args.get("keep").map(String::as_str).unwrap_or("0") {
        "0" => false,
        "1" => true,
        _ => return Err("keep must be 0 or 1".into()),
    };
    let other = match args.get("thread").map(String::as_str).unwrap_or("same") {
        "same" => false,
        "other" => true,
        _ => return Err("thread must be same or other".into()),
    };
    if keep && other {
        return Err("thread=other ends the first thread; use keep=0".into());
    }
    let folder = std::path::PathBuf::from(args.get("out").ok_or("exp-encoder-start requires out=")?);
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    println!(
        "keep={} thread={}",
        u8::from(keep),
        if other { "other" } else { "same" }
    );
    // Keep the process runtime alive even when the first encoder's thread ends.
    let _runtime = MediaFoundationRuntime::start()?;
    let mut held = None;
    if other {
        let first_folder = folder.clone();
        std::thread::spawn(move || {
            let _runtime = MediaFoundationRuntime::start()?;
            create(&first_folder, 0)?.finish()
        })
        .join()
        .map_err(|_| "first encoder thread panicked".to_owned())??;
    } else {
        let first = create(&folder, 0)?;
        if keep {
            held = Some(first);
        } else {
            first.finish()?;
        }
    }
    for index in 1..SIZES.len() {
        create(&folder, index)?.finish()?;
    }
    if let Some(encoder) = held {
        encoder.finish()?;
    }
    Ok(())
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
        "exp-dyn" => exp_dyn::run(),
        "exp-encoder-start" => encoder_start(),
        "exp-rate" => exp_meter::run_rate(),
        _ => Err(format!(
            "unknown command '{command}'; use info, readback, observe, delivery, newframes, disk, threading, lateness, encoder, compress, exp-encoders, exp-corpus or exp-z"
        )),
    };
    if let Err(error) = outcome {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}
