//! Native boundary injection only: production type_text owns all readiness
//! decisions, retries, and deadlines. No replacement readiness algorithm here.

use std::{
    cell::RefCell,
    time::{Duration, Instant},
};

use crate::backend::text_dispatch::TextDispatchInput;

#[derive(Clone, Debug)]
pub(super) struct Snapshot {
    pub(super) elapsed: Duration,
    pub(super) probes: Vec<(Duration, u32)>,
    pub(super) sleeps: Vec<u32>,
    pub(super) sent: Vec<TextDispatchInput>,
    pub(super) sent_at: Vec<Duration>,
    pub(super) received: Vec<TextDispatchInput>,
}

struct State {
    started: Instant,
    ready_at: Option<Duration>,
    snapshot: Snapshot,
}

thread_local! {
    static ACTIVE: RefCell<Option<State>> = const { RefCell::new(None) };
}

pub(super) struct Fixture;

impl Fixture {
    pub(super) fn install(ready_at: Option<Duration>) -> Self {
        ACTIVE.with(|active| {
            let mut active = active.borrow_mut();
            assert!(active.is_none(), "readiness fixture must not nest");
            *active = Some(State {
                started: Instant::now(),
                ready_at,
                snapshot: Snapshot {
                    elapsed: Duration::ZERO,
                    probes: Vec::new(),
                    sleeps: Vec::new(),
                    sent: Vec::new(),
                    sent_at: Vec::new(),
                    received: Vec::new(),
                },
            });
        });
        Self
    }

    pub(super) fn snapshot(&self) -> Snapshot {
        ACTIVE.with(|active| {
            let active = active.borrow();
            let Some(state) = active.as_ref() else {
                panic!("readiness fixture missing");
            };
            state.snapshot.clone()
        })
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        ACTIVE.with(|active| *active.borrow_mut() = None);
    }
}

pub(super) struct FocusReadyClock;

impl FocusReadyClock {
    pub(super) fn now() -> Instant {
        ACTIVE.with(|active| {
            active
                .borrow()
                .as_ref()
                .map_or_else(Instant::now, |state| state.started + state.snapshot.elapsed)
        })
    }
}

pub(super) fn probe(timeout_ms: u32) -> Option<bool> {
    ACTIVE.with(|active| {
        active.borrow_mut().as_mut().map(|state| {
            let elapsed = state.snapshot.elapsed;
            state.snapshot.probes.push((elapsed, timeout_ms));
            state.ready_at.is_some_and(|ready_at| elapsed >= ready_at)
        })
    })
}

pub(super) fn sleep_ms(ms: u32) -> bool {
    let injected = ACTIVE.with(|active| {
        active.borrow_mut().as_mut().map(|state| {
            if ms > 0 {
                state.snapshot.sleeps.push(ms);
                state.snapshot.elapsed += Duration::from_millis(u64::from(ms));
                // Fail an unbounded production loop rather than hang the suite.
                assert!(
                    state.snapshot.elapsed <= Duration::from_millis(251),
                    "readiness wait exceeded its 250 ms budget"
                );
            }
            false
        })
    });
    injected.unwrap_or_else(|| super::super::utils::sleep_ms(ms))
}

pub(super) fn capture(input: TextDispatchInput) -> bool {
    ACTIVE.with(|active| {
        let mut active = active.borrow_mut();
        let Some(state) = active.as_mut() else {
            return false;
        };
        let elapsed = state.snapshot.elapsed;
        state.snapshot.sent.push(input);
        state.snapshot.sent_at.push(elapsed);
        if state.ready_at.is_some_and(|ready_at| elapsed >= ready_at) {
            state.snapshot.received.push(input);
        }
        true
    })
}
