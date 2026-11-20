//! Bounded metric label helpers shared by crates that emit metrics but do not
//! depend on `synapse-telemetry`.

/// Distinct `subscriber_slot` label values for `events_dropped_for_subscriber`.
pub const SUBSCRIBER_SLOT_COUNT: u64 = 64;

/// Bounded metric label for a subscription id: a stable FNV-1a hash bucket in
/// `0..SUBSCRIBER_SLOT_COUNT`, so the metric's cardinality stays at the
/// declared 64 however many subscriptions come and go.
#[must_use]
pub fn subscriber_slot_label(subscription_id: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in subscription_id.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("slot-{:02}", hash % SUBSCRIBER_SLOT_COUNT)
}
