//! Windows capture of the probe window through windows-capture 2.0.0, the
//! crate the product uses, plus the per-frame timing every handler records.
//!
//! Session settings follow plan O1: cursor and "no border" are asked for only
//! where windows-capture reports them supported; secondary windows and dirty
//! regions stay Default; the update interval is the one under test.

use std::ffi::c_void;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use windows_capture::capture::{CaptureControl, GraphicsCaptureApiHandler};
use windows_capture::frame::Frame;
use windows_capture::graphics_capture_api::GraphicsCaptureApi;
use windows_capture::settings::{
    ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
    GraphicsCaptureItemType, MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
};
use windows_capture::window::Window;

use crate::Result;
use crate::clock::{self, CycleRate, HNS_PER_SECOND, hns_to_ms};
use crate::stats::Summary;

/// T_r: one refresh of the 60 Hz reference screen, in milliseconds.
pub const REFERENCE_PERIOD_MS: f64 = 1000.0 / 60.0;

/// The minimum update interval asked of Windows.
#[derive(Clone, Copy, Debug)]
pub enum Interval {
    Default,
    Millis(f64),
}

impl Interval {
    pub fn name(self) -> String {
        match self {
            Self::Default => "default".to_owned(),
            Self::Millis(ms) => format!("{ms}ms"),
        }
    }
}

/// A window handle that can cross to the capture thread.
struct WindowItem(isize);

impl TryInto<GraphicsCaptureItemType> for WindowItem {
    type Error = windows::core::Error;

    fn try_into(self) -> std::result::Result<GraphicsCaptureItemType, Self::Error> {
        Window::from_raw_hwnd(self.0 as *mut c_void).try_into()
    }
}

/// Starts capturing the window `hwnd` on windows-capture's own thread, with
/// the cursor drawn into the frames where that setting is supported.
pub fn start<H>(
    hwnd: isize,
    interval: Interval,
    flags: H::Flags,
) -> Result<CaptureControl<H, String>>
where
    H: GraphicsCaptureApiHandler<Error = String> + Send + 'static,
    H::Flags: Send,
{
    start_with_cursor(hwnd, interval, true, flags)
}

/// As `start`, choosing whether the cursor is drawn into the frames.
pub fn start_with_cursor<H>(
    hwnd: isize,
    interval: Interval,
    with_cursor: bool,
    flags: H::Flags,
) -> Result<CaptureControl<H, String>>
where
    H: GraphicsCaptureApiHandler<Error = String> + Send + 'static,
    H::Flags: Send,
{
    let cursor = match (
        GraphicsCaptureApi::is_cursor_settings_supported().unwrap_or(false),
        with_cursor,
    ) {
        (false, _) => CursorCaptureSettings::Default,
        (true, true) => CursorCaptureSettings::WithCursor,
        (true, false) => CursorCaptureSettings::WithoutCursor,
    };
    let border = if GraphicsCaptureApi::is_border_settings_supported().unwrap_or(false) {
        DrawBorderSettings::WithoutBorder
    } else {
        DrawBorderSettings::Default
    };
    let update = match interval {
        Interval::Default => MinimumUpdateIntervalSettings::Default,
        Interval::Millis(ms) => {
            MinimumUpdateIntervalSettings::Custom(Duration::from_secs_f64(ms / 1000.0))
        }
    };
    let settings = Settings::new(
        WindowItem(hwnd),
        cursor,
        border,
        SecondaryWindowSettings::Default,
        update,
        DirtyRegionSettings::Default,
        ColorFormat::Bgra8,
        flags,
    );
    H::start_free_threaded(settings).map_err(|error| format!("capture start failed: {error:?}"))
}

/// Stops the capture, waits for its thread and hands back the handler.
pub fn stop<H>(control: CaptureControl<H, String>) -> Result<Arc<Mutex<H>>>
where
    H: GraphicsCaptureApiHandler<Error = String> + Send + 'static,
{
    let handler = control.callback();
    control
        .stop()
        .map_err(|error| format!("capture stop failed: {error:?}"))?;
    Ok(handler)
}

/// The composition time of a frame (`SystemRelativeTime`, 100 ns units).
pub fn composition_time(frame: &Frame) -> i64 {
    frame
        .timestamp()
        .map_or_else(|_| clock::now_hns(), |time| time.Duration)
}

/// Timing of one delivered frame.
#[derive(Clone, Copy, Debug)]
pub struct FrameTimes {
    /// When Windows composed the frame.
    pub composed: i64,
    /// When the handler was entered.
    pub entered: i64,
    /// When the handler returned (the frame goes back to the one-buffer pool).
    pub returned: i64,
    /// Capture-thread cycle counter at handler entry.
    pub entry_cycles: u64,
    /// Cycles spent inside the handler.
    pub handler_cycles: u64,
}

/// Started at handler entry, finished at handler exit.
pub struct HandlerClock {
    entered: i64,
    cycles: u64,
}

impl HandlerClock {
    pub fn start() -> Self {
        Self {
            entered: clock::now_hns(),
            cycles: clock::thread_cycles(),
        }
    }

    pub fn finish(self, composed: i64) -> FrameTimes {
        FrameTimes {
            composed,
            entered: self.entered,
            returned: clock::now_hns(),
            entry_cycles: self.cycles,
            handler_cycles: clock::thread_cycles() - self.cycles,
        }
    }
}

/// Index range of the frames whose handler was entered in `[from, to)`.
pub fn range_between(frames: &[FrameTimes], from: i64, to: i64) -> std::ops::Range<usize> {
    let start = frames.partition_point(|frame| frame.entered < from);
    let end = frames.partition_point(|frame| frame.entered < to);
    start..end.max(start)
}

/// Delivery and cost figures of a run of frames.
pub struct FrameStats {
    pub frames: usize,
    pub per_second: f64,
    pub gap_ms: Summary,
    pub gaps_over_reference: usize,
    pub hold_ms: Summary,
    pub handler_ms: Summary,
    pub latency_ms: Summary,
    /// Capture-thread processor time per delivered frame (everything on that thread).
    pub thread_us_per_frame: f64,
    /// Processor time inside the handler per delivered frame.
    pub handler_us_per_frame: f64,
}

impl FrameStats {
    pub fn of(frames: &[FrameTimes], span_hns: i64, rate: CycleRate) -> Self {
        let gaps: Vec<f64> = frames
            .windows(2)
            .map(|pair| hns_to_ms(pair[1].composed - pair[0].composed))
            .collect();
        let thread_us_per_frame = match (frames.first(), frames.last()) {
            (Some(first), Some(last)) if frames.len() > 1 => {
                rate.micros(last.entry_cycles - first.entry_cycles) / (frames.len() - 1) as f64
            }
            _ => 0.0,
        };
        let handler_cycles: u64 = frames.iter().map(|frame| frame.handler_cycles).sum();
        Self {
            frames: frames.len(),
            per_second: frames.len() as f64 * HNS_PER_SECOND as f64 / span_hns.max(1) as f64,
            gaps_over_reference: gaps
                .iter()
                .filter(|&&gap| gap > REFERENCE_PERIOD_MS)
                .count(),
            gap_ms: Summary::of(&gaps),
            hold_ms: Summary::of(
                &frames
                    .iter()
                    .map(|f| hns_to_ms(f.returned - f.composed))
                    .collect::<Vec<_>>(),
            ),
            handler_ms: Summary::of(
                &frames
                    .iter()
                    .map(|f| hns_to_ms(f.returned - f.entered))
                    .collect::<Vec<_>>(),
            ),
            latency_ms: Summary::of(
                &frames
                    .iter()
                    .map(|f| hns_to_ms(f.entered - f.composed))
                    .collect::<Vec<_>>(),
            ),
            thread_us_per_frame,
            handler_us_per_frame: rate.micros(handler_cycles) / frames.len().max(1) as f64,
        }
    }

    pub fn print(&self, prefix: &str) {
        println!(
            "{prefix} delivered_frames={} delivered_per_s={:.2}",
            self.frames, self.per_second
        );
        println!(
            "{prefix} gap_ms {} gaps_over_T_r(16.67ms)={}",
            self.gap_ms, self.gaps_over_reference
        );
        println!("{prefix} hold_ms(composition->release) {}", self.hold_ms);
        println!("{prefix} handler_ms(entry->return) {}", self.handler_ms);
        println!(
            "{prefix} delivery_latency_ms(composition->entry) {}",
            self.latency_ms
        );
        println!(
            "{prefix} capture_thread_cpu_us_per_frame={:.2} handler_cpu_us_per_frame={:.2}",
            self.thread_us_per_frame, self.handler_us_per_frame
        );
    }
}
