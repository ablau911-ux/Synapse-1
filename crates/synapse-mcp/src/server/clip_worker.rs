//! The clip worker mode (docs/clip-recorder-plan.md, version 4.1, section B).
//!
//! `ProcessorBudget` owns the 10 % rule (E3, E10): the live part reports its
//! measured share every second (A9) and the worker's cap is what remains.

use std::sync::atomic::{AtomicU64, Ordering};

/// U_max: the recorder never uses more than this share of the processor time
/// of the whole machine (E10).
pub(super) const CPU_LIMIT_PERCENT: f64 = 10.0;
/// A segment closes when its retained raw bytes reach this (package 0, M4/M5; S6).
pub(super) const SEGMENT_RAW_BUDGET_BYTES: u64 = 1 << 30;

/// The live share and the worker cap, in percent of the whole machine.
pub(super) struct ProcessorBudget {
    live_share_bits: AtomicU64,
    worker_cap_bits: AtomicU64,
}

static BUDGET: ProcessorBudget = ProcessorBudget {
    live_share_bits: AtomicU64::new(0),
    // 10.0_f64 as bits: the whole limit while nothing is recorded.
    worker_cap_bits: AtomicU64::new(0x4024_0000_0000_0000),
};

impl ProcessorBudget {
    pub(super) fn global() -> &'static Self {
        &BUDGET
    }

    /// Records the live part's share of the last second; the worker's cap
    /// becomes `CPU_LIMIT_PERCENT` minus it (never below zero). Returns the cap.
    pub(super) fn set_live_share(&self, percent: f64) -> f64 {
        let share = percent.max(0.0);
        let cap = (CPU_LIMIT_PERCENT - share).max(0.0);
        self.live_share_bits
            .store(share.to_bits(), Ordering::Relaxed);
        self.worker_cap_bits.store(cap.to_bits(), Ordering::Relaxed);
        cap
    }

    /// The live part ended: the worker may use the whole limit again.
    pub(super) fn clear_live_share(&self) {
        self.set_live_share(0.0);
    }

    pub(super) fn live_share(&self) -> f64 {
        f64::from_bits(self.live_share_bits.load(Ordering::Relaxed))
    }

    pub(super) fn worker_cap(&self) -> f64 {
        f64::from_bits(self.worker_cap_bits.load(Ordering::Relaxed))
    }

    /// `true` when `cpu_hns` of processor time within one second already
    /// reaches the whole limit on a machine with `threads` logical processors
    /// (A9: the live part then sheds the read-back of new frames).
    pub(super) fn live_alone_reached(cpu_hns: i64, threads: u32) -> bool {
        let second_of_machine = 10_000_000.0 * f64::from(threads.max(1));
        cpu_hns as f64 * 100.0 / second_of_machine >= CPU_LIMIT_PERCENT
    }
}
