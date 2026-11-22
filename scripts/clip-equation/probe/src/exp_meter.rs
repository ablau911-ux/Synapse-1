//! What one experiment run measures about itself
//! (docs/clip-recorder-experiments-plan.md, sections 3 to 5): processor time
//! of the whole process (user plus kernel, `GetProcessTimes`), memory sampled
//! every 50 ms (`GetProcessMemoryInfo`: working set and private bytes) with
//! peaks per stage, wall time, and thread cycle counters for the parts of a
//! stage. Cycles become nanoseconds with the rate `clip-probe rate` measured
//! once (passed as `rate=`), so no run spends a second calibrating.
//!
//! Also the `key=value` argument reader every experiment command shares, and
//! the one `RESULT` line each run prints.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
use windows::Win32::System::Threading::GetCurrentProcess;

use crate::Result;
use crate::clock;

const SAMPLE_INTERVAL: Duration = Duration::from_millis(50);

/// `key=value` command-line arguments.
pub struct Args(BTreeMap<String, String>);

impl Args {
    pub fn from_env() -> Self {
        Self(
            std::env::args()
                .skip(2)
                .filter_map(|arg| {
                    arg.split_once('=')
                        .map(|(key, value)| (key.to_owned(), value.to_owned()))
                })
                .collect(),
        )
    }

    pub fn text(&self, key: &str) -> Result<&str> {
        self.0
            .get(key)
            .map(String::as_str)
            .ok_or_else(|| format!("missing argument {key}="))
    }

    pub fn text_or<'a>(&'a self, key: &str, default: &'a str) -> &'a str {
        self.0.get(key).map_or(default, String::as_str)
    }

    pub fn number<T: std::str::FromStr>(&self, key: &str, default: T) -> Result<T> {
        match self.0.get(key) {
            None => Ok(default),
            Some(text) => text
                .parse()
                .map_err(|_| format!("argument {key}={text} is not a number")),
        }
    }

    pub fn flag(&self, key: &str, default: bool) -> bool {
        self.0.get(key).map_or(default, |value| value == "1")
    }

    /// The cycle-counter rate in cycles per second (`clip-probe rate`).
    pub fn rate(&self) -> Result<f64> {
        let rate: f64 = self.number("rate", 0.0)?;
        if rate > 0.0 {
            Ok(rate)
        } else {
            Ok(clock::CycleRate::calibrate().per_second())
        }
    }
}

/// The `RESULT key=value ...` line a run prints for the orchestrator.
#[derive(Default)]
pub struct Report(Vec<(String, String)>);

impl Report {
    pub fn put(&mut self, key: &str, value: impl std::fmt::Display) {
        let text = value.to_string().replace([' ', '\t', '\n'], "_");
        self.0.push((key.to_owned(), text));
    }

    pub fn print(&self) {
        let line: Vec<String> = self.0.iter().map(|(k, v)| format!("{k}={v}")).collect();
        println!("RESULT {}", line.join(" "));
    }
}

/// Working set and private bytes of this process now.
pub fn memory_now() -> (u64, u64) {
    let mut counters = PROCESS_MEMORY_COUNTERS_EX::default();
    let size = u32::try_from(std::mem::size_of_val(&counters)).unwrap_or(u32::MAX);
    // SAFETY: the pseudo handle of this process and a sized out-parameter.
    let ok = unsafe {
        GetProcessMemoryInfo(GetCurrentProcess(), (&raw mut counters).cast(), size)
    }
    .is_ok();
    if ok {
        (counters.WorkingSetSize as u64, counters.PrivateUsage as u64)
    } else {
        (0, 0)
    }
}

/// The operating system's own peak working set of this process.
pub fn os_peak_working_set() -> u64 {
    let mut counters = PROCESS_MEMORY_COUNTERS_EX::default();
    let size = u32::try_from(std::mem::size_of_val(&counters)).unwrap_or(u32::MAX);
    // SAFETY: as in `memory_now`.
    let _ = unsafe {
        GetProcessMemoryInfo(GetCurrentProcess(), (&raw mut counters).cast(), size)
    };
    counters.PeakWorkingSetSize as u64
}

/// Process processor time (user plus kernel) in nanoseconds.
pub fn process_cpu_ns() -> u64 {
    u64::try_from(clock::current_process_time_hns()).unwrap_or(0) * 100
}

pub fn wall_ns() -> u64 {
    u64::try_from(clock::now_hns()).unwrap_or(0) * 100
}

/// Samples the memory of this process every 50 ms on its own thread; the
/// peaks since the last `take_peaks` are kept.
pub struct MemorySampler {
    stop: Arc<AtomicBool>,
    peak_ws: Arc<AtomicU64>,
    peak_private: Arc<AtomicU64>,
    thread: Option<JoinHandle<()>>,
}

impl MemorySampler {
    pub fn start() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let peak_ws = Arc::new(AtomicU64::new(0));
        let peak_private = Arc::new(AtomicU64::new(0));
        let (s, w, p) = (stop.clone(), peak_ws.clone(), peak_private.clone());
        let thread = std::thread::spawn(move || {
            while !s.load(Ordering::Relaxed) {
                let (ws, private) = memory_now();
                w.fetch_max(ws, Ordering::Relaxed);
                p.fetch_max(private, Ordering::Relaxed);
                std::thread::sleep(SAMPLE_INTERVAL);
            }
        });
        let sampler = Self {
            stop,
            peak_ws,
            peak_private,
            thread: Some(thread),
        };
        sampler.take_peaks();
        sampler
    }

    /// Peak working set and private bytes since the last call (one last
    /// sample is taken now, so a stage shorter than 50 ms still counts).
    pub fn take_peaks(&self) -> (u64, u64) {
        let (ws, private) = memory_now();
        let peak_ws = self.peak_ws.swap(ws, Ordering::Relaxed).max(ws);
        let peak_private = self.peak_private.swap(private, Ordering::Relaxed).max(private);
        (peak_ws, peak_private)
    }
}

impl Drop for MemorySampler {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// One stage of a run: process processor time, wall time and memory peaks.
pub struct Stage {
    cpu_start: u64,
    wall_start: u64,
}

pub struct StageFigures {
    pub cpu_ns: u64,
    pub wall_ns: u64,
    pub peak_ws: u64,
    pub peak_private: u64,
}

impl Stage {
    /// Starts a stage; the sampler's peaks are reset to now.
    pub fn start(sampler: &MemorySampler) -> Self {
        sampler.take_peaks();
        Self {
            cpu_start: process_cpu_ns(),
            wall_start: wall_ns(),
        }
    }

    pub fn finish(self, sampler: &MemorySampler) -> StageFigures {
        let (peak_ws, peak_private) = sampler.take_peaks();
        StageFigures {
            cpu_ns: process_cpu_ns().saturating_sub(self.cpu_start),
            wall_ns: wall_ns().saturating_sub(self.wall_start),
            peak_ws,
            peak_private,
        }
    }
}

impl StageFigures {
    pub fn put(&self, report: &mut Report, prefix: &str) {
        report.put(&format!("{prefix}_cpu_ns"), self.cpu_ns);
        report.put(&format!("{prefix}_wall_ns"), self.wall_ns);
        report.put(&format!("{prefix}_peak_ws"), self.peak_ws);
        report.put(&format!("{prefix}_peak_private"), self.peak_private);
    }
}

/// Thread cycles of the worker's waiting-frame read and NV12 conversion,
/// added by the marked lines of the copied `encode_once`.
pub static READ_CYCLES: AtomicU64 = AtomicU64::new(0);
pub static DRAW_CYCLES: AtomicU64 = AtomicU64::new(0);

pub fn add_worker_cycles(read: u64, draw: u64) {
    READ_CYCLES.fetch_add(read, Ordering::Relaxed);
    DRAW_CYCLES.fetch_add(draw, Ordering::Relaxed);
}

/// `clip-probe rate`: the cycle-counter rate, for `rate=` of every run.
pub fn run_rate() -> Result<()> {
    println!("RESULT rate={:.0}", clock::CycleRate::calibrate().per_second());
    Ok(())
}
