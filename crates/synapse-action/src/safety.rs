use std::{
    panic,
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use synapse_core::error_codes;

use crate::RELEASE_ALL_HANDLE;

const PANIC_RELEASE_ALL_TIMEOUT_MS: u64 = 10;

static PANIC_HOOK_INSTALLED: OnceLock<()> = OnceLock::new();
static RELEASE_CLOCK_EPOCH: OnceLock<Instant> = OnceLock::new();
/// Nanoseconds since [`RELEASE_CLOCK_EPOCH`] (plus one, so zero means "unset")
/// at which the most recent `ReleaseAll` was enqueued.
static RELEASE_ALL_ENQUEUED_AT: AtomicU64 = AtomicU64::new(0);

fn release_clock_now_marker(epoch: Instant) -> u64 {
    u64::try_from(epoch.elapsed().as_nanos())
        .unwrap_or(u64::MAX)
        .saturating_add(1)
}

/// Records the enqueue instant of a `ReleaseAll` so the emitter can report how
/// long it waited in the queue (#95 per-phase timing).
pub(crate) fn mark_release_all_enqueued() {
    let epoch = *RELEASE_CLOCK_EPOCH.get_or_init(Instant::now);
    RELEASE_ALL_ENQUEUED_AT.store(release_clock_now_marker(epoch), Ordering::Release);
}

/// Consumes the enqueue mark and returns the queue wait, or `None` when the
/// `ReleaseAll` did not come through a marking enqueue path.
pub(crate) fn take_release_all_queue_wait() -> Option<Duration> {
    let marked = RELEASE_ALL_ENQUEUED_AT.swap(0, Ordering::AcqRel);
    if marked == 0 {
        return None;
    }
    let epoch = *RELEASE_CLOCK_EPOCH.get()?;
    Some(Duration::from_nanos(
        release_clock_now_marker(epoch).saturating_sub(marked),
    ))
}

#[tracing::instrument(skip_all)]
pub fn install_panic_hook() {
    PANIC_HOOK_INSTALLED.get_or_init(|| {
        let previous_hook = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            if let Some(handle) = RELEASE_ALL_HANDLE.get() {
                let timeout = Duration::from_millis(PANIC_RELEASE_ALL_TIMEOUT_MS);
                match handle.fire_release_all_blocking_with_timeout(timeout) {
                    Ok(()) => {
                        tracing::warn!(
                            code = error_codes::SAFETY_RELEASE_ALL_FIRED,
                            reason = "panic",
                            timeout_ms = PANIC_RELEASE_ALL_TIMEOUT_MS,
                            result = "ok",
                            "panic hook fired release_all before previous panic hook"
                        );
                    }
                    Err(error) => {
                        tracing::warn!(
                            code = error_codes::SAFETY_RELEASE_ALL_FIRED,
                            reason = "panic",
                            timeout_ms = PANIC_RELEASE_ALL_TIMEOUT_MS,
                            result = "error",
                            error_code = error.code(),
                            detail = error.detail(),
                            "panic hook attempted release_all before previous panic hook"
                        );
                    }
                }
            }

            previous_hook(info);
        }));
    });
}
