//! M2: the observer's processor cost per delivered frame (symbol c_obs): one
//! GPU copy into a slot plus the composition time, no read-back, while the
//! probe window repaints on every composition (the display rate).
//!
//! Two figures: the capture thread's own processor time per frame (every
//! cycle on that thread between frames, crate work included), and the whole
//! process minus the painter thread minus the same process measured before
//! the capture started (this also catches driver and runtime threads).

use windows::Win32::Foundation::RECT;

use crate::Result;
use crate::clock::{self, CycleRate, HNS_PER_SECOND};
use crate::gpu::Gpu;
use crate::observer::{self, FrameStats, Interval};
use crate::probe_window::{Content, ProbeWindow};
use crate::slot_copy::{SlotCopy, SlotCopyFlags};

pub const WINDOW_RECT: RECT = RECT {
    left: 40,
    top: 40,
    right: 40 + 1280,
    bottom: 40 + 720,
};
const BASELINE_HNS: i64 = 5 * HNS_PER_SECOND;
const MEASURE_HNS: i64 = 10 * HNS_PER_SECOND;

/// Cycles of the process that are neither the painter nor this thread.
struct ProcessRest {
    cycles: f64,
    painter: u64,
}

impl ProcessRest {
    fn over(window: &ProbeWindow, span_hns: i64) -> Self {
        let (process, painter, main) = (
            clock::process_cycles(),
            window.painter_cycles(),
            clock::thread_cycles(),
        );
        clock::sleep_until(clock::now_hns() + span_hns);
        let painter = window.painter_cycles() - painter;
        let main = clock::thread_cycles() - main;
        let process = clock::process_cycles() - process;
        Self {
            cycles: process as f64 - painter as f64 - main as f64,
            painter,
        }
    }
}

pub fn run() -> Result<()> {
    let rate = CycleRate::calibrate();
    println!(
        "cycle_counter_hz={:.0} capture_device_adapter=\"{}\"",
        rate.per_second(),
        Gpu::default_adapter_name()?
    );
    let window = ProbeWindow::open(Content::Counter, WINDOW_RECT)?;
    std::thread::sleep(std::time::Duration::from_secs(1));

    let paints_before = window.paints();
    let baseline = ProcessRest::over(&window, BASELINE_HNS);
    let baseline_paints = window.paints() - paints_before;

    let control = observer::start::<SlotCopy>(
        window.raw_hwnd(),
        Interval::Default,
        SlotCopyFlags {
            read_counter: false,
        },
    )?;
    std::thread::sleep(std::time::Duration::from_secs(1));
    let from = clock::now_hns();
    let paints_from = window.paints();
    let measured = ProcessRest::over(&window, MEASURE_HNS);
    let to = clock::now_hns();
    let paints = window.paints() - paints_from;
    let handler = observer::stop(control)?;
    let handler = handler.lock();
    let all = handler.frames();
    let frames = &all[observer::range_between(all, from, to)];
    let stats = FrameStats::of(frames, to - from, rate);
    stats.print("M2");

    let capture_cycles =
        measured.cycles - baseline.cycles * (MEASURE_HNS as f64 / BASELINE_HNS as f64);
    let per_frame_us = capture_cycles / rate.per_second() * 1e6 / stats.frames.max(1) as f64;
    println!(
        "M2 painter paints_baseline={baseline_paints} paints_measured={paints} painter_cpu_ms_baseline={:.1} painter_cpu_ms_measured={:.1}",
        rate.millis(baseline.painter),
        rate.millis(measured.painter)
    );
    println!(
        "M2 c_obs_capture_thread_us={:.2} c_obs_process_rest_us={:.2} handler_only_us={:.2}",
        stats.thread_us_per_frame, per_frame_us, stats.handler_us_per_frame
    );
    Ok(())
}
