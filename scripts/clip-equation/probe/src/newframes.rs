//! M4: the new-frame rate n_new. The probe's own text window is captured with
//! the cursor on; four phases follow one another:
//!
//!   idle    nothing happens
//!   caret   the edit control has the focus of the window's own thread
//!   typing  8 characters per second posted as WM_CHAR to the probe's own edit control
//!   mouse   the cursor moves inside the probe window (SetCursorPos, 60 per
//!           second, at most 5 s), then goes back where it was
//!
//! For each phase, on the 50 ms instant grid: how many instants get a new
//! frame by identity (a new composition time, the product's rule) and by
//! content (different pixels), per second.
//!
//! `newframes nocursor` runs only the mouse phase, with the cursor left out of
//! the frames, to show what the cursor alone contributes (a separate run, so
//! the mouse never moves for more than 5 s in one run).

use std::time::Duration;

use windows::Win32::Foundation::{POINT, RECT};
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, SetCursorPos};

use crate::Result;
use crate::clock::{self, CycleRate, HNS_PER_MS, HNS_PER_SECOND};
use crate::hash_frames::HashFrames;
use crate::observer::{self, FrameStats, FrameTimes, Interval};
use crate::probe_window::{Content, ProbeWindow};
use crate::slots::INSTANT_HNS;

const WINDOW_RECT: RECT = RECT {
    left: 60,
    top: 60,
    right: 60 + 854,
    bottom: 60 + 480,
};
const IDLE_HNS: i64 = 5 * HNS_PER_SECOND;
const CARET_HNS: i64 = 6 * HNS_PER_SECOND;
const TYPING_CHARS_PER_SECOND: u32 = 8;
const TYPING_SECONDS: u32 = 5;
/// The owner's limit for moving the mouse.
const MOUSE_HNS: i64 = 4_900 * HNS_PER_MS;
const MOUSE_STEP_HNS: i64 = 166_667;

struct Phase {
    name: &'static str,
    from: i64,
    to: i64,
}

pub fn run() -> Result<()> {
    let with_cursor = std::env::args().nth(2).as_deref() != Some("nocursor");
    let rate = CycleRate::calibrate();
    let window = ProbeWindow::open(Content::Text, WINDOW_RECT)?;
    std::thread::sleep(Duration::from_millis(500));
    let control = observer::start_with_cursor::<HashFrames>(
        window.raw_hwnd(),
        Interval::Default,
        with_cursor,
        (),
    )?;
    std::thread::sleep(Duration::from_millis(1500));
    println!("M4 cursor_in_frames={with_cursor}");
    let phases = if with_cursor {
        all_phases(&window)
    } else {
        vec![move_mouse_inside(&window)]
    };
    std::thread::sleep(Duration::from_millis(300));
    let handler = observer::stop(control)?;
    let handler = handler.lock();
    for phase in &phases {
        report(phase, handler.frames(), handler.hashes(), rate);
    }
    Ok(())
}

fn all_phases(window: &ProbeWindow) -> Vec<Phase> {
    let mut phases = Vec::new();

    let from = clock::now_hns();
    clock::sleep_until(from + IDLE_HNS);
    phases.push(Phase {
        name: "idle",
        from,
        to: clock::now_hns(),
    });

    window.focus_edit();
    std::thread::sleep(Duration::from_millis(300));
    let from = clock::now_hns();
    clock::sleep_until(from + CARET_HNS);
    phases.push(Phase {
        name: "caret",
        from,
        to: clock::now_hns(),
    });

    let from = clock::now_hns();
    let step = HNS_PER_SECOND / i64::from(TYPING_CHARS_PER_SECOND);
    for index in 0..TYPING_CHARS_PER_SECOND * TYPING_SECONDS {
        let letter = char::from(b'a' + (index % 26) as u8);
        window.type_char(letter);
        clock::sleep_until(from + i64::from(index + 1) * step);
    }
    phases.push(Phase {
        name: "typing",
        from,
        to: clock::now_hns(),
    });

    phases.push(move_mouse_inside(window));
    phases
}

/// Moves the cursor along an ellipse inside the probe window, then puts it back.
fn move_mouse_inside(window: &ProbeWindow) -> Phase {
    let rect = window.rect();
    let (center_x, center_y) = (
        f64::from(rect.left + rect.right) / 2.0,
        f64::from(rect.top + rect.bottom) / 2.0,
    );
    let (radius_x, radius_y) = (
        f64::from(rect.right - rect.left) * 0.35,
        f64::from(rect.bottom - rect.top) * 0.35,
    );
    let mut saved = POINT::default();
    // SAFETY: one out-parameter.
    let had_position = unsafe { GetCursorPos(&raw mut saved) }.is_ok();
    let from = clock::now_hns();
    let mut step = 0_i64;
    while clock::now_hns() - from < MOUSE_HNS {
        let angle = step as f64 * 0.05;
        let x = (center_x + radius_x * angle.cos()).round() as i32;
        let y = (center_y + radius_y * (2.0 * angle).sin()).round() as i32;
        // SAFETY: a point inside the probe's own window.
        let _ = unsafe { SetCursorPos(x, y) };
        step += 1;
        clock::sleep_until(from + step * MOUSE_STEP_HNS);
    }
    let to = clock::now_hns();
    if had_position {
        // SAFETY: restores the owner's cursor position.
        let _ = unsafe { SetCursorPos(saved.x, saved.y) };
    }
    Phase {
        name: "mouse",
        from,
        to,
    }
}

fn report(phase: &Phase, frames: &[FrameTimes], hashes: &[u64], rate: CycleRate) {
    let range = observer::range_between(frames, phase.from, phase.to);
    let stats = FrameStats::of(&frames[range], phase.to - phase.from, rate);
    let (mut instants, mut new_by_identity, mut new_by_content) = (0_u32, 0_u32, 0_u32);
    let mut previous: Option<usize> = None;
    let mut instant = phase.from;
    while instant < phase.to {
        // The last frame composed at or before this instant.
        let chosen = frames
            .partition_point(|frame| frame.composed <= instant)
            .checked_sub(1);
        if let (Some(now), Some(before)) = (chosen, previous) {
            instants += 1;
            if frames[now].composed != frames[before].composed {
                new_by_identity += 1;
            }
            if hashes[now] != hashes[before] {
                new_by_content += 1;
            }
        }
        previous = chosen.or(previous);
        instant += INSTANT_HNS;
    }
    let seconds = (phase.to - phase.from) as f64 / HNS_PER_SECOND as f64;
    println!(
        "M4 phase={} seconds={seconds:.2} delivered_frames={} delivered_per_s={:.2} instants={instants} n_new_identity_per_s={:.2} n_new_content_per_s={:.2}",
        phase.name,
        stats.frames,
        stats.per_second,
        f64::from(new_by_identity) / seconds,
        f64::from(new_by_content) / seconds
    );
}
