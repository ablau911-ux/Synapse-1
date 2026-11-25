//! M3: does Windows deliver every screen refresh? The probe window repaints
//! once per composition with its paint counter in the top-left blocks; the
//! capture runs with the minimum update interval at Default (the Windows 10
//! path), 16.7 ms and 11 ms. For each: the delivered rate, the gaps between
//! composition times (longest gap against T_r = 16.7 ms), and the paints that
//! never reached a delivered frame.

use std::time::Duration;

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{DWM_TIMING_INFO, DwmGetCompositionTimingInfo};

use crate::Result;
use crate::clock::{self, CycleRate, HNS_PER_SECOND};
use crate::observe::WINDOW_RECT;
use crate::observer::{self, FrameStats, Interval};
use crate::probe_window::{COUNTER_BITS, Content, ProbeWindow};
use crate::slot_copy::{SlotCopy, SlotCopyFlags};

const MEASURE_HNS: i64 = 8 * HNS_PER_SECOND;
const INTERVALS: [Interval; 3] = [
    Interval::Default,
    Interval::Millis(16.7),
    Interval::Millis(11.0),
];

pub fn run() -> Result<()> {
    let rate = CycleRate::calibrate();
    let mut timing = DWM_TIMING_INFO {
        cbSize: size_of::<DWM_TIMING_INFO>() as u32,
        ..Default::default()
    };
    // SAFETY: a DWM_TIMING_INFO with its size set; a null window means the desktop.
    if unsafe { DwmGetCompositionTimingInfo(HWND::default(), &raw mut timing) }.is_ok() {
        let (numerator, denominator) = (
            timing.rateRefresh.uiNumerator,
            timing.rateRefresh.uiDenominator,
        );
        println!("M3 display_refresh={numerator}/{denominator}");
    }
    let window = ProbeWindow::open(Content::Counter, WINDOW_RECT)?;
    std::thread::sleep(Duration::from_secs(1));
    for interval in INTERVALS {
        let prefix = format!("M3 interval={}", interval.name());
        let control = observer::start::<SlotCopy>(
            window.raw_hwnd(),
            interval,
            SlotCopyFlags { read_counter: true },
        )?;
        std::thread::sleep(Duration::from_secs(1));
        let (from, paints_from) = (clock::now_hns(), window.paints());
        clock::sleep_until(from + MEASURE_HNS);
        let (to, paints_to) = (clock::now_hns(), window.paints());
        let handler = observer::stop(control)?;
        let mut handler = handler.lock();
        let counters = handler.counters()?;
        let all = handler.frames();
        let range = observer::range_between(all, from, to);
        FrameStats::of(&all[range.clone()], to - from, rate).print(&prefix);
        let seen = counters.get(range).unwrap_or_default();
        print_counter_report(&prefix, seen, paints_to - paints_from);
    }
    Ok(())
}

/// Paints between consecutive delivered frames that no delivered frame showed.
fn print_counter_report(prefix: &str, seen: &[Option<u32>], paints: u64) {
    let modulus = 1_u32 << COUNTER_BITS;
    let (mut missing, mut repeats, mut longest_skip, mut unread) = (0_u64, 0_u64, 0_u32, 0_u64);
    let mut previous: Option<u32> = None;
    for value in seen {
        let Some(value) = *value else {
            unread += 1;
            continue;
        };
        if let Some(last) = previous {
            let step = (value + modulus - last) % modulus;
            if step == 0 {
                repeats += 1;
            } else {
                missing += u64::from(step - 1);
                longest_skip = longest_skip.max(step - 1);
            }
        }
        previous = Some(value);
    }
    println!(
        "{prefix} paints={paints} frames_with_counter={} paints_not_delivered={missing} longest_run_not_delivered={longest_skip} repeated_paint_frames={repeats} unread={unread}",
        seen.len() as u64 - unread
    );
}
