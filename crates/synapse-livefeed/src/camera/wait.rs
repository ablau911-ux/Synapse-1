//! Blocking waits for `changes`: wake on the first change since the cursor, or wait until
//! the screen has been quiet for a settle window ("tell me when loading finished").

use std::{
    sync::MutexGuard,
    time::{Duration, Instant},
};

use super::ScreenState;
use crate::{
    camera::lifecycle::{CameraShared, CameraState, wait_while},
    config::ONE_U64,
    error::{LivefeedError, Result},
};

type Guard<'a> = MutexGuard<'a, CameraState>;

/// Waits according to `wait_ms` / `settle_ms` and returns the guard plus whether the screen
/// settled (only meaningful when `settle_ms` was requested).
pub(super) fn wait_for_changes<'a>(
    shared: &'a CameraShared,
    mut s: Guard<'a>,
    since: u64,
    wait_ms: Option<u64>,
    settle_ms: Option<u64>,
) -> Result<(Guard<'a>, bool)> {
    let generation = s.generation;
    let max_ms = s.cfg.wait_max_ms;
    if let Some(settle) = settle_ms {
        let budget = wait_ms.unwrap_or(max_ms).min(max_ms);
        let window = settle.min(budget).max(ONE_U64);
        let (next, settled) = wait_until_settled(shared, s, generation, budget, window);
        ensure_same_run(&next, generation)?;
        return Ok((next, settled));
    }
    if let Some(req_ms) = wait_ms
        && since == s.latest_seq
    {
        let budget = req_ms.min(max_ms);
        s = wait_while(shared, s, budget, |st| {
            st.running
                && st.generation == generation
                && st.screen_state == ScreenState::Live
                && st.latest_seq == since
        });
        ensure_same_run(&s, generation)?;
    }
    Ok((s, false))
}

/// Returns once no frame has been stored for a full `settle_ms` window, or when `budget_ms`
/// runs out (then `settled == false`).
fn wait_until_settled<'a>(
    shared: &'a CameraShared,
    mut s: Guard<'a>,
    generation: u64,
    budget_ms: u64,
    settle_ms: u64,
) -> (Guard<'a>, bool) {
    let deadline = Instant::now() + Duration::from_millis(budget_ms);
    loop {
        let seq_before = s.latest_seq;
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return (s, false);
        }
        let slice = millis(remaining).min(settle_ms);
        s = wait_while(shared, s, slice, |st| {
            st.running && st.generation == generation && st.latest_seq == seq_before
        });
        if !s.running || s.generation != generation {
            return (s, false);
        }
        if s.latest_seq == seq_before {
            // Quiet for the whole slice: settled only if the slice was a full window.
            return (s, slice == settle_ms);
        }
    }
}

fn ensure_same_run(s: &CameraState, generation: u64) -> Result<()> {
    if !s.running || s.generation != generation {
        return Err(LivefeedError::NotRunning);
    }
    Ok(())
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}
