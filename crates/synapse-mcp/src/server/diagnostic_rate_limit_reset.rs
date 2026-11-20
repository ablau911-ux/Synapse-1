//! Generation-checked TTL reset for the rate-limit diagnostic override (#199).
//!
//! Each override takes a new generation from the emitter's own limiter
//! ([`BackendRateLimitControl::override_backend`]); the reset scheduled by an
//! override only fires if no newer override of that limiter replaced it, so
//! overlapping overrides cannot end each other early. The check and the reset
//! happen under the limiter's write lock
//! ([`BackendRateLimitControl::reset_backend_if_current`]).

use std::time::Duration;

use synapse_action::{BackendRateLimitControl, ResolvedBackend};

/// Restore `backend`'s default limiter after `ttl_ms`, unless a newer override
/// (a higher generation) replaced the one this reset belongs to.
pub(super) fn schedule_reset(
    control: BackendRateLimitControl,
    backend: ResolvedBackend,
    ttl_ms: u64,
    generation: u64,
) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(ttl_ms)).await;
        match control.reset_backend_if_current(backend, generation) {
            Ok(Some(readback)) => {
                tracing::info!(
                    code = "ACTION_DIAGNOSTIC_RATE_LIMIT_RESET",
                    backend = readback.backend.as_str(),
                    before_capacity = readback.before.capacity,
                    before_tokens = readback.before.tokens,
                    before_refill_rate_per_s = readback.before.refill_rate_per_s,
                    after_capacity = readback.after.capacity,
                    after_tokens = readback.after.tokens,
                    after_refill_rate_per_s = readback.after.refill_rate_per_s,
                    "action diagnostic rate limit reset completed"
                );
            }
            Ok(None) => {
                tracing::info!(
                    code = "ACTION_DIAGNOSTIC_RATE_LIMIT_RESET_SUPERSEDED",
                    backend = backend.as_str(),
                    generation,
                    "action diagnostic rate limit reset skipped: a newer override is active"
                );
            }
            Err(error) => {
                tracing::error!(
                    code = error.code(),
                    backend = backend.as_str(),
                    detail = error.detail(),
                    "action diagnostic rate limit reset failed"
                );
            }
        }
    });
}
