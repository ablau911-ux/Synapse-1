//! exp-av1, live mode: Windows.Graphics.Capture of one window by handle, cursor not drawn,
//! handed to the `Recorder` one instant at a time.
//!
//! The instant rule is exp-corpus's: instant k ends at s_k = (k0 + k) · 50 ms on the
//! performance counter and shows the last frame composed at or before s_k. Here k0 is the
//! instant of the first frame Windows delivers, so the recording starts with a picture.
//! Shortly after s_k (`READ_MARGIN_HNS`) the main thread takes instant k from the capture
//! handler: the frame composed in it, "unchanged" when none was, or a named loss.

use std::collections::VecDeque;
use std::sync::Arc;

use parking_lot::Mutex;
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext};
use windows_capture::capture::{Context, GraphicsCaptureApiHandler};
use windows_capture::frame::Frame;
use windows_capture::graphics_capture_api::InternalCaptureControl;

use crate::Result;
use crate::clock::{self, HNS_PER_MS};
use crate::exp_av1::{Input, Recorder};
use crate::exp_av1_video::INSTANTS_PER_SECOND;
use crate::exp_cpu::Spent;
use crate::exp_measure::bgra_to_bgr;
use crate::gpu::{MapWait, ReadBack};
use crate::observer::{self, Interval, composition_time};
use crate::slots::{INSTANT_HNS, READ_MARGIN_HNS};

/// Package 0 result (M3), as in exp-corpus: 11 ms is the requested update interval.
const REQUESTED_INTERVAL_MS: f64 = 11.0;
const DRAW_CURSOR: bool = false;
/// How long the first frame may take after the capture starts.
const FIRST_FRAME_TIMEOUT_HNS: i64 = 2_000 * HNS_PER_MS;
const FIRST_FRAME_POLL: std::time::Duration = std::time::Duration::from_millis(1);
/// Frames of ended instants the handler keeps while the recorder is busy (0.5 s).
const MAX_WAITING_FRAMES: usize = 10;

/// Loss causes named in the index.
const LOST_BEHIND: &str = "recorder_behind";
const LOST_CAPTURE_FAILED: &str = "capture_failed";
const LOST_WINDOW_CLOSED: &str = "window_closed";

/// Records `seconds` of the window `hwnd` into `recorder`. Returns the source stage's
/// time: frame read-back on the capture thread plus taking and converting on this one.
pub fn record(hwnd: isize, seconds: u64, recorder: &mut Recorder) -> Result<Spent> {
    let control = observer::start_with_cursor::<LiveFrames>(
        hwnd,
        Interval::Millis(REQUESTED_INTERVAL_MS),
        DRAW_CURSOR,
        (),
    )?;
    let handler = control.callback();
    let fed = feed(&handler, seconds * u64::from(INSTANTS_PER_SECOND), recorder);
    let stopped = observer::stop(control);
    let taking = fed?;
    let read_back = stopped?.lock().read_back;
    Ok(read_back.plus(taking))
}

/// Feeds every instant to the recorder; returns the time spent taking and converting frames.
fn feed(handler: &Arc<Mutex<LiveFrames>>, instants: u64, recorder: &mut Recorder) -> Result<Spent> {
    let k0 = first_instant(handler)?;
    let mut bgr = Vec::new();
    let mut taking = Spent::default();
    for instant in 0..instants {
        let end = (k0 + i64::try_from(instant).map_err(|_| "too many instants")?) * INSTANT_HNS;
        clock::sleep_until(end + READ_MARGIN_HNS);
        let taken = taking.time(|| handler.lock().take(instant));
        let input = match &taken {
            Taken::Picture(frame) => {
                taking.time(|| bgra_to_bgr(&frame.bgra, &mut bgr));
                Input::Picture {
                    bgr: &bgr,
                    width: frame.width,
                    height: frame.height,
                }
            }
            Taken::Unchanged => Input::Unchanged,
            Taken::Lost(cause) => Input::Lost(*cause),
        };
        recorder.instant(instant, input)?;
    }
    Ok(taking)
}

/// Waits for the first delivered frame; its instant is instant 0.
fn first_instant(handler: &Arc<Mutex<LiveFrames>>) -> Result<i64> {
    let deadline = clock::now_hns() + FIRST_FRAME_TIMEOUT_HNS;
    loop {
        if let Some(k0) = handler.lock().k0 {
            return Ok(k0);
        }
        if clock::now_hns() > deadline {
            return Err("the window delivered no frame within 2 s".into());
        }
        std::thread::sleep(FIRST_FRAME_POLL);
    }
}

/// One frame read back to the processor, B,G,R,A.
struct HeldFrame {
    instant: u64,
    width: u32,
    height: u32,
    bgra: Vec<u8>,
}

/// What the handler gives for one instant.
enum Taken {
    Picture(HeldFrame),
    Unchanged,
    Lost(&'static str),
}

/// The capture handler: per instant the last frame composed at or before its end.
pub struct LiveFrames {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    readback: Option<ReadBack>,
    /// Absolute instant of the first frame.
    k0: Option<i64>,
    /// The instant of the frame now in `readback`.
    held: Option<u64>,
    /// Frames of ended instants, oldest first, not yet taken.
    ended: VecDeque<HeldFrame>,
    /// Instants whose frame was dropped because `ended` was full.
    dropped: VecDeque<u64>,
    /// The last instant the recorder took.
    taken: Option<u64>,
    failed: bool,
    closed: bool,
    /// Time spent on the capture thread reading frames back and keeping them.
    read_back: Spent,
}

impl LiveFrames {
    /// Moves the frame in `readback` to the ended frames.
    fn end_held(&mut self) {
        let (Some(instant), Some(readback)) = (self.held.take(), self.readback.as_ref()) else {
            return;
        };
        if self.ended.len() == MAX_WAITING_FRAMES
            && let Some(oldest) = self.ended.pop_front()
        {
            self.dropped.push_back(oldest.instant);
        }
        let (width, height) = readback.size();
        self.ended.push_back(HeldFrame {
            instant,
            width,
            height,
            bgra: readback.pixels().to_vec(),
        });
    }

    /// Instant `instant` has ended (its read margin passed): hands over what it showed.
    fn take(&mut self, instant: u64) -> Taken {
        self.taken = Some(instant);
        while self.dropped.front().is_some_and(|&lost| lost < instant) {
            self.dropped.pop_front();
        }
        if self.dropped.front() == Some(&instant) {
            self.dropped.pop_front();
            return Taken::Lost(LOST_BEHIND);
        }
        if self.ended.front().is_some_and(|frame| frame.instant == instant) {
            if let Some(frame) = self.ended.pop_front() {
                return Taken::Picture(frame);
            }
        }
        if self.held == Some(instant) {
            self.end_held();
            if let Some(frame) = self.ended.pop_back() {
                return Taken::Picture(frame);
            }
        }
        if self.failed {
            Taken::Lost(LOST_CAPTURE_FAILED)
        } else if self.closed {
            Taken::Lost(LOST_WINDOW_CLOSED)
        } else {
            Taken::Unchanged
        }
    }

    fn read(&mut self, frame: &mut Frame) -> Result<()> {
        let size = (frame.width(), frame.height());
        if self.readback.as_ref().is_none_or(|r| r.size() != size) {
            self.end_held();
            self.readback = Some(ReadBack::new(&self.device, size.0, size.1)?);
        }
        let Some(readback) = self.readback.as_mut() else {
            return Err("no read-back buffer".to_owned());
        };
        readback.read(&self.context, frame.as_raw_texture(), MapWait::Block)?;
        Ok(())
    }
}

impl GraphicsCaptureApiHandler for LiveFrames {
    type Flags = ();
    type Error = String;

    fn new(ctx: Context<Self::Flags>) -> Result<Self> {
        Ok(Self {
            device: ctx.device,
            context: ctx.device_context,
            readback: None,
            k0: None,
            held: None,
            ended: VecDeque::new(),
            dropped: VecDeque::new(),
            taken: None,
            failed: false,
            closed: false,
            read_back: Spent::default(),
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        _control: InternalCaptureControl,
    ) -> Result<()> {
        let mut read_back = self.read_back;
        read_back.time(|| self.keep(frame));
        self.read_back = read_back;
        Ok(())
    }

    fn on_closed(&mut self) -> Result<()> {
        self.closed = true;
        Ok(())
    }
}

impl LiveFrames {
    /// Reads `frame` back and holds it as the picture of its instant.
    fn keep(&mut self, frame: &mut Frame) {
        if self.failed {
            return;
        }
        // Instant of a composition time: the first s_j at or after it.
        let composed = composition_time(frame);
        let absolute =
            composed.div_euclid(INSTANT_HNS) + i64::from(composed.rem_euclid(INSTANT_HNS) != 0);
        let k0 = *self.k0.get_or_insert(absolute);
        let mut instant = u64::try_from(absolute - k0).unwrap_or(0);
        // Delivered after its instant was taken: it is the picture of the next instant.
        if let Some(taken) = self.taken {
            instant = instant.max(taken + 1);
        }
        if self.held.is_some_and(|held| instant > held) {
            self.end_held();
        }
        if self.read(frame).is_err() {
            // Every later instant is named lost (`take`); the capture itself keeps running.
            self.failed = true;
            self.held = None;
            return;
        }
        self.held = Some(instant);
    }
}
