//! Processor time of one part of an experiment run, the same for every way
//! (Z and D), so the cost equation J compares equal numbers.
//!
//! `GetProcessTimes` advances in clock ticks (15.6 ms), far coarser than one
//! frame, and read 0.5 to 0.8 of the true time in the way Z pilot. Per-frame
//! sums therefore use the process cycle counter (`QueryProcessCycleTime`, user
//! plus kernel cycles of every thread), converted to nanoseconds at the rate
//! `clip-probe exp-rate` measured (`rate=`). The `GetProcessTimes` sum is kept
//! beside it as `<prefix>_gpt_ns`.

use crate::clock;
use crate::exp_meter::{process_cpu_ns, wall_ns};

/// One counter set: process CPU (ns, `GetProcessTimes`), process cycles, the cycles of
/// the calling thread alone (exp-av1's stage breakdown) and wall time (ns).
#[derive(Default, Clone, Copy)]
pub struct Spent {
    pub cpu_ns: u64,
    pub cycles: u64,
    pub thread_cycles: u64,
    pub wall_ns: u64,
}

impl Spent {
    /// Runs `work` and adds what it cost.
    pub fn time<T>(&mut self, work: impl FnOnce() -> T) -> T {
        let (cpu, cycles, thread, wall) = (
            process_cpu_ns(),
            clock::process_cycles(),
            clock::thread_cycles(),
            wall_ns(),
        );
        let value = work();
        self.cpu_ns += process_cpu_ns().saturating_sub(cpu);
        self.cycles += clock::process_cycles().saturating_sub(cycles);
        self.thread_cycles += clock::thread_cycles().saturating_sub(thread);
        self.wall_ns += wall_ns().saturating_sub(wall);
        value
    }

    /// M2: the cost of two parts of one way's work together (the way's CPU is the sum of all its parts).
    pub fn plus(self, other: Self) -> Self {
        Self {
            cpu_ns: self.cpu_ns + other.cpu_ns,
            cycles: self.cycles + other.cycles,
            thread_cycles: self.thread_cycles + other.thread_cycles,
            wall_ns: self.wall_ns + other.wall_ns,
        }
    }

    /// Process CPU in ns from the cycle counter at `rate` cycles per second.
    pub fn cpu_from_cycles(&self, rate: f64) -> u64 {
        (self.cycles as f64 / rate * 1e9) as u64
    }

    /// `"<prefix>_cpu_ns"` (cycle counter), `_gpt_ns`, `_cycles` and `_wall_ns` as JSON fields.
    pub fn json(&self, prefix: &str, rate: f64) -> String {
        format!(
            "\"{prefix}_cpu_ns\":{},\"{prefix}_gpt_ns\":{},\"{prefix}_cycles\":{},\"{prefix}_wall_ns\":{}",
            self.cpu_from_cycles(rate),
            self.cpu_ns,
            self.cycles,
            self.wall_ns
        )
    }
}
