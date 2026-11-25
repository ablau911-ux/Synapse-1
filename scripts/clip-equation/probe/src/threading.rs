//! M6 and M7: the threading rule and the selection lateness, with slots
//! assigned by instant (`slots.rs`) on a multithread-protected device.
//!
//! M6 (`threading`): 1280 x 720 counter window, capture interval Default. The
//! reader maps once with a blocking `Map`, once with "do not wait" and retry.
//! Reported: the handler's hold time while the reader maps, the reader's
//! read-back time, retries and processor time.
//!
//! M7 (`lateness`): 854 x 480 counter window. The reader reads back every new
//! frame and writes it raw to the clip volume, as the live part will; 4 s idle,
//! then 4 s with every logical processor kept busy. Reported per phase: how
//! many closed instants the reader is behind, slots in use, frames lost.

use std::sync::Arc;
use std::time::Duration;

use windows::Win32::Foundation::RECT;

use crate::Result;
use crate::clock::{self, CycleRate, HNS_PER_SECOND};
use crate::gpu::MapWait;
use crate::load::BusyLoad;
use crate::observe::WINDOW_RECT;
use crate::observer::{self, FrameStats, FrameTimes, Interval};
use crate::probe_window::{Content, ProbeWindow};
use crate::scratch::ScratchFolder;
use crate::slots::{
    self, INSTANT_HNS, InstantRecord, InstantSlots, InstantSlotsFlags, READ_MARGIN_HNS, SlotShared,
};
use crate::stats::Summary;

const M6_HNS: i64 = 8 * HNS_PER_SECOND;
const M7_PHASE_HNS: i64 = 4 * HNS_PER_SECOND;
const M7_WINDOW_RECT: RECT = RECT {
    left: 40,
    top: 40,
    right: 40 + 854,
    bottom: 40 + 480,
};
const M6_WAITS: [MapWait; 3] = [MapWait::Block, MapWait::PollYield, MapWait::PollSleep];
/// M7 writes raw frames; the yield-retry wait is left out to stay well inside the disk cap.
const M7_WAITS: [MapWait; 2] = [MapWait::Block, MapWait::PollSleep];

/// One capture plus reader run.
struct Run {
    frames: Vec<FrameTimes>,
    records: Vec<InstantRecord>,
    origin: i64,
    overflows: u64,
    max_in_use: usize,
}

fn run_once(
    window: &ProbeWindow,
    wait: MapWait,
    span_hns: i64,
    spool: Option<&mut ScratchFolder>,
    load_after_hns: Option<i64>,
) -> Result<(Run, i64, i64)> {
    let shared = SlotShared::new();
    let control = observer::start::<InstantSlots>(
        window.raw_hwnd(),
        Interval::Default,
        InstantSlotsFlags {
            shared: Arc::clone(&shared),
        },
    )?;
    std::thread::sleep(Duration::from_secs(1));
    let from = clock::now_hns();
    let to = from + span_hns;
    let load_thread = load_after_hns.map(|after| {
        std::thread::spawn(move || {
            clock::sleep_until(from + after);
            let threads = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
            let _load = BusyLoad::start(threads);
            clock::sleep_until(to);
        })
    });
    let records = slots::run_reader(&shared, wait, to, spool);
    if let Some(thread) = load_thread {
        let _ = thread.join();
    }
    let handler = observer::stop(control)?;
    let records = records?;
    let frames = handler.lock().frames().to_vec();
    let table = shared.table();
    let run = Run {
        frames,
        records,
        origin: table.origin().unwrap_or(from),
        overflows: table.overflows(),
        max_in_use: table.max_in_use(),
    };
    Ok((run, from, to))
}

/// Frames delivered later than the reader's margin after their instant.
fn late_deliveries(run: &Run) -> usize {
    run.frames
        .iter()
        .filter(|frame| {
            let instant = slots::instant_at_or_after(run.origin, frame.composed);
            frame.entered > run.origin + instant * INSTANT_HNS + READ_MARGIN_HNS
        })
        .count()
}

fn print_reader(prefix: &str, records: &[InstantRecord], rate: CycleRate) {
    let reads: Vec<&InstantRecord> = records.iter().filter(|record| record.new_frame).collect();
    let read_ms: Vec<f64> = reads.iter().map(|record| record.read_ms).collect();
    let read_cpu_ms: Vec<f64> = reads
        .iter()
        .map(|record| rate.millis(record.read_cycles))
        .collect();
    let lateness: Vec<f64> = records
        .iter()
        .map(|record| record.lateness as f64)
        .collect();
    let retries: u64 = reads
        .iter()
        .map(|record| u64::from(record.map_retries))
        .sum();
    println!(
        "{prefix} instants={} new_frames={} read_ms {} | read_cpu_ms {} | map_retries_per_read={:.1}",
        records.len(),
        reads.len(),
        Summary::of(&read_ms),
        Summary::of(&read_cpu_ms),
        retries as f64 / reads.len().max(1) as f64
    );
    println!("{prefix} lateness_instants {}", Summary::of(&lateness));
}

pub fn run_threading() -> Result<()> {
    let rate = CycleRate::calibrate();
    let window = ProbeWindow::open(Content::Counter, WINDOW_RECT)?;
    std::thread::sleep(Duration::from_secs(1));
    for wait in M6_WAITS {
        let prefix = format!("M6 wait={}", wait.name());
        let (run, from, to) = run_once(&window, wait, M6_HNS, None, None)?;
        let range = observer::range_between(&run.frames, from, to);
        FrameStats::of(&run.frames[range], to - from, rate).print(&prefix);
        print_reader(&prefix, &run.records, rate);
        println!(
            "{prefix} slot_overflows={} max_slots_in_use={} deliveries_later_than_margin({}ms)={}",
            run.overflows,
            run.max_in_use,
            clock::hns_to_ms(READ_MARGIN_HNS),
            late_deliveries(&run)
        );
    }
    Ok(())
}

pub fn run_lateness() -> Result<()> {
    let rate = CycleRate::calibrate();
    let window = ProbeWindow::open(Content::Counter, M7_WINDOW_RECT)?;
    std::thread::sleep(Duration::from_secs(1));
    for wait in M7_WAITS {
        let mut spool = ScratchFolder::create("lateness")?;
        // 20 frames per second of 854 x 480 x 4 bytes for 8 s, with room to spare.
        spool.reserve(200 * 854 * 480 * 4)?;
        let outcome = run_once(
            &window,
            wait,
            2 * M7_PHASE_HNS,
            Some(&mut spool),
            Some(M7_PHASE_HNS),
        );
        let written = spool.written();
        spool.clear()?;
        let (run, from, to) = outcome?;
        let middle = from + M7_PHASE_HNS;
        for (phase, start, end) in [("idle", from, middle), ("load", middle, to)] {
            let prefix = format!("M7 wait={} phase={phase}", wait.name());
            let records: Vec<InstantRecord> = run
                .records
                .iter()
                .copied()
                .filter(|record| record.done >= start && record.done < end)
                .collect();
            print_reader(&prefix, &records, rate);
            let range = observer::range_between(&run.frames, start, end);
            let frames = &run.frames[range];
            let hold: Vec<f64> = frames
                .iter()
                .map(|frame| clock::hns_to_ms(frame.returned - frame.composed))
                .collect();
            println!(
                "{prefix} delivered_frames={} hold_ms {}",
                frames.len(),
                Summary::of(&hold)
            );
        }
        println!(
            "M7 wait={} slot_overflows={} max_slots_in_use={} raw_bytes_written={written}",
            wait.name(),
            run.overflows,
            run.max_in_use
        );
    }
    Ok(())
}
