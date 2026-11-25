//! Wall time and processor time, the two clocks every probe reads.
//!
//! Wall time comes from the performance counter in 100 ns units. That is the
//! unit and origin of `Frame::timestamp()` (`SystemRelativeTime`), so a frame's
//! composition time and "now" subtract directly.
//!
//! Processor time comes from the thread and process cycle counters, which
//! count user and kernel time at a fixed rate. The rate is measured on a
//! spinning thread (`CycleRate::calibrate`); `info` cross-checks it with
//! `GetThreadTimes`.

use std::sync::OnceLock;

use windows::Win32::Foundation::{FILETIME, HANDLE};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThread, GetProcessTimes, GetThreadTimes,
};
use windows::Win32::System::WindowsProgramming::{QueryProcessCycleTime, QueryThreadCycleTime};

pub const HNS_PER_SECOND: i64 = 10_000_000;
pub const HNS_PER_MS: i64 = 10_000;

fn counter_frequency() -> i64 {
    static FREQUENCY: OnceLock<i64> = OnceLock::new();
    *FREQUENCY.get_or_init(|| {
        let mut frequency = 0;
        // SAFETY: writes one i64.
        let _ = unsafe { QueryPerformanceFrequency(&raw mut frequency) };
        frequency.max(1)
    })
}

/// Performance-counter time in 100 ns units, same origin as `SystemRelativeTime`.
pub fn now_hns() -> i64 {
    let mut ticks = 0;
    // SAFETY: writes one i64.
    let _ = unsafe { QueryPerformanceCounter(&raw mut ticks) };
    let hns = i128::from(ticks) * i128::from(HNS_PER_SECOND) / i128::from(counter_frequency());
    i64::try_from(hns).unwrap_or(i64::MAX)
}

pub fn hns_to_ms(hns: i64) -> f64 {
    hns as f64 / HNS_PER_MS as f64
}

/// Sleeps until the performance counter reaches `deadline_hns`.
pub fn sleep_until(deadline_hns: i64) {
    let left = deadline_hns - now_hns();
    if left > 0 {
        std::thread::sleep(std::time::Duration::from_nanos(left.unsigned_abs() * 100));
    }
}

/// Cycles this thread has used, user and kernel.
pub fn thread_cycles() -> u64 {
    let mut cycles = 0;
    // SAFETY: the pseudo handle of the current thread; writes one u64.
    let _ = unsafe { QueryThreadCycleTime(GetCurrentThread(), &raw mut cycles) };
    cycles
}

/// Cycles every thread of this process has used, user and kernel.
pub fn process_cycles() -> u64 {
    let mut cycles = 0;
    // SAFETY: the pseudo handle of the current process; writes one u64.
    let _ = unsafe { QueryProcessCycleTime(GetCurrentProcess(), &raw mut cycles) };
    cycles
}

fn filetime_hns(time: FILETIME) -> i64 {
    (i64::from(time.dwHighDateTime) << 32) | i64::from(time.dwLowDateTime)
}

/// User plus kernel time of a process, in 100 ns units.
pub fn process_time_hns(process: HANDLE) -> Option<i64> {
    let mut times = [FILETIME::default(); 4];
    let [created, exited, kernel, user] = &mut times;
    // SAFETY: four FILETIME out-parameters.
    unsafe { GetProcessTimes(process, created, exited, kernel, user) }.ok()?;
    Some(filetime_hns(*kernel) + filetime_hns(*user))
}

pub fn current_process_time_hns() -> i64 {
    // SAFETY: the pseudo handle of the current process.
    process_time_hns(unsafe { GetCurrentProcess() }).unwrap_or(0)
}

/// User plus kernel time of the current thread from `GetThreadTimes`, in 100 ns units.
pub fn current_thread_time_hns() -> i64 {
    let mut times = [FILETIME::default(); 4];
    let [created, exited, kernel, user] = &mut times;
    // SAFETY: the pseudo handle of the current thread; four FILETIME out-parameters.
    if unsafe { GetThreadTimes(GetCurrentThread(), created, exited, kernel, user) }.is_err() {
        return 0;
    }
    filetime_hns(*kernel) + filetime_hns(*user)
}

/// The rate of the cycle counters, measured on this PC.
#[derive(Clone, Copy, Debug)]
pub struct CycleRate {
    per_second: f64,
}

impl CycleRate {
    /// Spins this thread three times for 300 ms; a thread that runs the whole
    /// time gains cycles at the counter rate. The largest of the three is kept,
    /// because a preempted run can only read low.
    pub fn calibrate() -> Self {
        let mut best = 0.0_f64;
        for _ in 0..3 {
            let (cycles_start, wall_start) = (thread_cycles(), now_hns());
            while now_hns() - wall_start < 300 * HNS_PER_MS {
                std::hint::spin_loop();
            }
            let (cycles_end, wall_end) = (thread_cycles(), now_hns());
            let seconds = (wall_end - wall_start) as f64 / HNS_PER_SECOND as f64;
            best = best.max((cycles_end - cycles_start) as f64 / seconds);
        }
        Self { per_second: best }
    }

    pub const fn per_second(self) -> f64 {
        self.per_second
    }

    pub fn seconds(self, cycles: u64) -> f64 {
        cycles as f64 / self.per_second
    }

    pub fn micros(self, cycles: u64) -> f64 {
        self.seconds(cycles) * 1e6
    }

    pub fn millis(self, cycles: u64) -> f64 {
        self.seconds(cycles) * 1e3
    }
}
