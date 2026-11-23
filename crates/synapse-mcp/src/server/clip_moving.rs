//! One bounded moving encoder thread, with one sample for every interval instant.

use std::path::Path;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::Threading::{GetCurrentThread, GetThreadTimes};

use super::clip_decoder::{KeyFrame, SampleTable};
use super::clip_encoder::{ClipEncoder, EncoderGrant, MediaFoundationRuntime, Nv12Box};
use super::clip_motion::PixelRect;

pub(super) const MOVING_QUEUE_FRAMES: usize = 8;
pub(super) const MOVING_SEGMENT_MS: u64 = 60_000;
const MILLISECONDS_PER_SECOND: u64 = 1_000;
const HNS_PER_SECOND: i64 = 10_000_000;
const BGR_BYTES: usize = 3;
const EVEN: u32 = 2;
const FILETIME_HIGH_SHIFT: u32 = 32;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(from = "[u32; 2]", into = "[u32; 2]")]
pub(super) struct MovingRef {
    pub segment: u32,
    pub frame: u32,
}
impl From<[u32; 2]> for MovingRef {
    fn from([segment, frame]: [u32; 2]) -> Self {
        Self { segment, frame }
    }
}
impl From<MovingRef> for [u32; 2] {
    fn from(value: MovingRef) -> Self {
        [value.segment, value.frame]
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct MovingSegment {
    pub index: u32,
    pub file: String,
    pub first_instant: u64,
    pub last_instant: u64,
    pub rect: PixelRect,
    pub coded_width: u32,
    pub coded_height: u32,
    pub frames: u32,
    pub key_frames: Vec<KeyFrame>,
    pub complete: bool,
}
impl MovingSegment {
    pub fn from_file(
        folder: &Path,
        index: u32,
        rect: PixelRect,
        first_instant: u64,
        expected_frames: u32,
    ) -> Result<Option<Self>, String> {
        let file = format!("moving/seg-{index:04}.mp4");
        let final_path = folder.join(&file);
        let part = folder.join(format!("{file}.part"));
        let source = if part.exists() { &part } else { &final_path };
        let table = match SampleTable::read(source) {
            Ok(table) if table.len() > 0 => table,
            _ => {
                if source.exists() {
                    std::fs::remove_file(source).map_err(|e| e.to_string())?;
                }
                return Ok(None);
            }
        };
        let frames = table.len();
        if frames > expected_frames {
            return Err("moving file has more samples than submitted instants".into());
        }
        if part.exists() {
            std::fs::rename(&part, &final_path).map_err(|e| e.to_string())?;
        }
        Ok(Some(Self {
            index,
            file,
            first_instant,
            last_instant: first_instant + u64::from(frames) - 1,
            rect,
            coded_width: rect.w.div_ceil(EVEN) * EVEN,
            coded_height: rect.h.div_ceil(EVEN) * EVEN,
            frames,
            key_frames: table.key_frames(),
            complete: frames == expected_frames,
        }))
    }
}

#[derive(Clone, Debug)]
pub(super) enum MovingAvailability {
    Untried,
    Available,
    Unavailable(String),
}
static AVAILABILITY: Mutex<MovingAvailability> = Mutex::new(MovingAvailability::Untried);
impl MovingAvailability {
    pub fn get() -> Self {
        AVAILABILITY
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    pub fn set_available() {
        let mut value = AVAILABILITY.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(*value, Self::Untried) {
            *value = Self::Available;
        }
    }
    pub fn set_unavailable(reason: String) {
        *AVAILABILITY.lock().unwrap_or_else(|e| e.into_inner()) = Self::Unavailable(reason);
    }
}

#[derive(Default)]
pub(super) struct MovingStep {
    pub reference: Option<MovingRef>,
    pub began: Option<PixelRect>,
    pub behind: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct ShortSegment {
    pub segment: u32,
    pub first_instant: u64,
    pub expected: u32,
    pub held: u32,
}
#[derive(Default, Serialize)]
pub(super) struct MovingSummary {
    pub committed: Vec<MovingSegment>,
    pub short: Vec<ShortSegment>,
    pub grant: Option<EncoderGrant>,
    pub unavailable: Option<String>,
    pub creation_ms: usize,
    pub high_water: usize,
}
enum MovingJob {
    Begin {
        segment: u32,
        rect: PixelRect,
        first_instant: u64,
    },
    Frame {
        bgr: Vec<u8>,
    },
    Repeat,
    End,
}
struct OpenSegment {
    index: u32,
    rect: PixelRect,
    first: u64,
}

pub(super) struct MovingCoder {
    jobs: mpsc::Sender<MovingJob>,
    spare: mpsc::Receiver<Vec<u8>>,
    committed: mpsc::Receiver<MovingSegment>,
    queued: Arc<AtomicUsize>,
    cpu_hns: Arc<AtomicI64>,
    creation_ms: Arc<AtomicUsize>,
    high_water: usize,
    buffers: usize,
    open: Option<OpenSegment>,
    next_index: u32,
    fps: u32,
    thread: Option<JoinHandle<MovingSummary>>,
}
impl MovingCoder {
    pub fn start(folder: &Path, fps: u32) -> Result<Self, String> {
        if fps == 0 {
            return Err("moving fps must be positive".into());
        }
        std::fs::create_dir_all(folder.join("moving")).map_err(|e| e.to_string())?;
        let (jobs, receiver) = mpsc::channel();
        let (spare_tx, spare) = mpsc::channel();
        let (committed_tx, committed) = mpsc::channel();
        let queued = Arc::new(AtomicUsize::new(0));
        let cpu_hns = Arc::new(AtomicI64::new(0));
        let creation_ms = Arc::new(AtomicUsize::new(0));
        let worker_folder = folder.to_owned();
        let worker_queued = Arc::clone(&queued);
        let worker_cpu = Arc::clone(&cpu_hns);
        let worker_creation = Arc::clone(&creation_ms);
        let thread = std::thread::Builder::new()
            .name("synapse-clip-moving".into())
            .spawn(move || {
                MovingWorker {
                    folder: worker_folder,
                    fps,
                    spare: spare_tx,
                    committed: committed_tx,
                    queued: worker_queued,
                    cpu_hns: worker_cpu,
                    creation_ms: worker_creation,
                    open: None,
                    summary: MovingSummary::default(),
                }
                .run(receiver)
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            jobs,
            spare,
            committed,
            queued,
            cpu_hns,
            creation_ms,
            high_water: 0,
            buffers: 0,
            open: None,
            next_index: 0,
            fps,
            thread: Some(thread),
        })
    }

    fn send(&self, job: MovingJob) {
        if self.jobs.send(job).is_err() {
            MovingAvailability::set_unavailable("moving thread disconnected".into());
        }
    }

    pub fn follow(
        &mut self,
        instant: u64,
        rect: Option<PixelRect>,
        source: &[u8],
        source_width: u32,
        new_picture: bool,
    ) -> MovingStep {
        let rect = if matches!(
            MovingAvailability::get(),
            MovingAvailability::Unavailable(_)
        ) {
            None
        } else {
            rect
        };
        let segment_frames = MOVING_SEGMENT_MS * u64::from(self.fps) / MILLISECONDS_PER_SECOND;
        if self.open.as_ref().is_some_and(|s| {
            Some(s.rect) != rect || instant.saturating_sub(s.first) >= segment_frames
        }) {
            self.send(MovingJob::End);
            self.open = None;
        }
        let Some(rect) = rect else {
            return MovingStep::default();
        };
        let began = if self.open.is_none() {
            self.send(MovingJob::Begin {
                segment: self.next_index,
                rect,
                first_instant: instant,
            });
            self.open = Some(OpenSegment {
                index: self.next_index,
                rect,
                first: instant,
            });
            self.next_index += 1;
            Some(rect)
        } else {
            None
        };
        let reference = self.open.as_ref().map(|s| MovingRef {
            segment: s.index,
            frame: (instant - s.first) as u32,
        });
        let mut behind = false;
        if new_picture || began.is_some() {
            let mut buffer = self.spare.try_recv().ok();
            if buffer.is_none() && self.buffers < MOVING_QUEUE_FRAMES + 1 {
                self.buffers += 1;
                buffer = Some(Vec::new());
            }
            if self.queued.load(Ordering::Acquire) < MOVING_QUEUE_FRAMES && buffer.is_some() {
                let mut bgr = buffer.take().unwrap_or_default();
                bgr.clear();
                let stride = source_width as usize * BGR_BYTES;
                for row in rect.y..rect.bottom() {
                    let start = row as usize * stride + rect.x as usize * BGR_BYTES;
                    let end = start + rect.w as usize * BGR_BYTES;
                    let Some(pixels) = source.get(start..end) else {
                        MovingAvailability::set_unavailable(
                            "moving rectangle lies outside source picture".into(),
                        );
                        self.send(MovingJob::Repeat);
                        return MovingStep {
                            reference,
                            began,
                            behind: true,
                        };
                    };
                    bgr.extend_from_slice(pixels);
                }
                let count = self.queued.fetch_add(1, Ordering::AcqRel) + 1;
                self.high_water = self.high_water.max(count);
                self.send(MovingJob::Frame { bgr });
            } else {
                // A spare taken during saturation remains counted only if retained.
                if buffer.is_some() {
                    self.buffers -= 1;
                }
                self.send(MovingJob::Repeat);
                behind = true;
            }
        } else {
            self.send(MovingJob::Repeat);
        }
        MovingStep {
            reference,
            began,
            behind,
        }
    }
    pub fn take_committed(&self) -> Vec<MovingSegment> {
        self.committed.try_iter().collect()
    }
    pub fn cpu_hns(&self) -> i64 {
        self.cpu_hns.load(Ordering::Acquire)
    }
    pub fn high_water(&self) -> usize {
        self.high_water
    }
    pub fn creation_ms(&self) -> usize {
        self.creation_ms.load(Ordering::Acquire)
    }
    pub fn finish(mut self) -> MovingSummary {
        let creation_ms = self.creation_ms();
        let high_water = self.high_water();
        drop(self.jobs);
        let mut summary = match self.thread.take().map(JoinHandle::join) {
            Some(Ok(summary)) => summary,
            _ => {
                MovingAvailability::set_unavailable("moving thread panicked".into());
                MovingSummary::default()
            }
        };
        summary.committed.extend(self.committed.try_iter());
        summary.creation_ms = creation_ms;
        summary.high_water = high_water;
        if let MovingAvailability::Unavailable(reason) = MovingAvailability::get() {
            summary.unavailable = Some(reason);
        }
        summary
    }
}

struct WorkerSegment {
    index: u32,
    first: u64,
    rect: PixelRect,
    encoder: Option<ClipEncoder>,
    pixels: Nv12Box,
    expected: u32,
}
struct MovingWorker {
    folder: std::path::PathBuf,
    fps: u32,
    spare: mpsc::Sender<Vec<u8>>,
    committed: mpsc::Sender<MovingSegment>,
    queued: Arc<AtomicUsize>,
    cpu_hns: Arc<AtomicI64>,
    creation_ms: Arc<AtomicUsize>,
    open: Option<WorkerSegment>,
    summary: MovingSummary,
}
impl MovingWorker {
    fn run(mut self, jobs: mpsc::Receiver<MovingJob>) -> MovingSummary {
        let runtime = MediaFoundationRuntime::start();
        if let Err(reason) = &runtime {
            MovingAvailability::set_unavailable(reason.clone());
        }
        let start_cpu = thread_cpu_hns();
        for job in jobs {
            match job {
                MovingJob::Begin {
                    segment,
                    rect,
                    first_instant,
                } => {
                    let pixels = Nv12Box::new(rect.w, rect.h);
                    let (width, height) = pixels.coded_size();
                    let started = Instant::now();
                    let encoder = if matches!(
                        MovingAvailability::get(),
                        MovingAvailability::Unavailable(_)
                    ) {
                        None
                    } else {
                        match ClipEncoder::create(
                            &self
                                .folder
                                .join(format!("moving/seg-{segment:04}.mp4.part")),
                            width,
                            height,
                            self.fps,
                        ) {
                            Ok(encoder) => {
                                self.summary.grant = Some(encoder.grant().clone());
                                MovingAvailability::set_available();
                                Some(encoder)
                            }
                            Err(reason) => {
                                MovingAvailability::set_unavailable(reason);
                                None
                            }
                        }
                    };
                    self.creation_ms
                        .fetch_max(started.elapsed().as_millis() as usize, Ordering::AcqRel);
                    self.open = Some(WorkerSegment {
                        index: segment,
                        first: first_instant,
                        rect,
                        encoder,
                        pixels,
                        expected: 0,
                    });
                }
                MovingJob::Frame { bgr } => {
                    if let Some(open) = &mut self.open {
                        open.pixels.draw(&bgr);
                    }
                    self.write_sample();
                    let _ = self.spare.send(bgr);
                    self.queued.fetch_sub(1, Ordering::AcqRel);
                }
                MovingJob::Repeat => self.write_sample(),
                MovingJob::End => self.end(),
            }
            self.cpu_hns.store(
                thread_cpu_hns().saturating_sub(start_cpu),
                Ordering::Release,
            );
        }
        self.end();
        self.cpu_hns.store(
            thread_cpu_hns().saturating_sub(start_cpu),
            Ordering::Release,
        );
        drop(runtime);
        self.summary
    }
    fn write_sample(&mut self) {
        if let Some(open) = &mut self.open {
            let duration = HNS_PER_SECOND / i64::from(self.fps);
            if let Some(encoder) = &mut open.encoder
                && let Err(reason) = encoder.write_nv12(
                    open.pixels.bytes(),
                    i64::from(open.expected) * duration,
                    duration,
                )
            {
                MovingAvailability::set_unavailable(reason);
                if let Some(encoder) = open.encoder.take() {
                    let _ = encoder.finish();
                }
            }
            open.expected += 1;
        }
    }
    fn end(&mut self) {
        let Some(mut open) = self.open.take() else {
            return;
        };
        if let Some(encoder) = open.encoder.take()
            && let Err(reason) = encoder.finish()
        {
            MovingAvailability::set_unavailable(reason);
        }
        let segment = match MovingSegment::from_file(
            &self.folder,
            open.index,
            open.rect,
            open.first,
            open.expected,
        ) {
            Ok(segment) => segment,
            Err(reason) => {
                MovingAvailability::set_unavailable(reason);
                None
            }
        };
        let held = segment.as_ref().map_or(0, |s| s.frames);
        if held < open.expected {
            self.summary.short.push(ShortSegment {
                segment: open.index,
                first_instant: open.first,
                expected: open.expected,
                held,
            });
        }
        if let Some(segment) = segment
            && let Err(error) = self.committed.send(segment)
        {
            self.summary.committed.push(error.0);
        }
    }
}
fn thread_cpu_hns() -> i64 {
    let (mut created, mut exited, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    // SAFETY: GetCurrentThread is a valid pseudo-handle; all output pointers live through the call.
    if unsafe {
        GetThreadTimes(
            GetCurrentThread(),
            &raw mut created,
            &raw mut exited,
            &raw mut kernel,
            &raw mut user,
        )
    }
    .is_err()
    {
        return 0;
    }
    let ticks = |time: FILETIME| {
        (u64::from(time.dwHighDateTime) << FILETIME_HIGH_SHIFT) | u64::from(time.dwLowDateTime)
    };
    ticks(kernel)
        .saturating_add(ticks(user))
        .min(i64::MAX as u64) as i64
}
