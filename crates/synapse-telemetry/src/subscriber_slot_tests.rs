//! Regression tests for #60: the drop metric's subscriber label is bounded and
//! no never-emitted metric stays registered.

use std::collections::BTreeSet;

use super::{M3_METRICS, SUBSCRIBER_SLOT_COUNT, subscriber_slot_label};

#[test]
fn must_pass_a_thousand_subscription_ids_map_into_at_most_64_slots() {
    let slots: BTreeSet<String> = (0..1_000)
        .map(|index| subscriber_slot_label(&format!("01a131d7-d578-77b1-91d1-{index:012}")))
        .collect();
    assert!(slots.len() as u64 <= SUBSCRIBER_SLOT_COUNT);
    assert!(slots.len() > 8, "the hash must actually spread ids");
}

#[test]
fn must_pass_the_label_is_stable_and_never_the_raw_id() {
    let id = "01a131d7-d578-77b1-91d1-e9e35307415b";
    let label = subscriber_slot_label(id);
    assert_eq!(label, subscriber_slot_label(id));
    assert!(label.starts_with("slot-"));
    assert!(!label.contains(id));
}

#[test]
fn must_fail_removed_metrics_are_not_registered() {
    for removed in [
        "events_published_total",
        "reflex_fires_total",
        "reflex_tick_jitter_us",
        "reflex_recursion_clamps_total",
        "reflex_starved_total",
        "storage_disk_pressure_level",
        "storage_cf_bytes",
        "storage_write_batch_flushes_total",
    ] {
        assert!(
            !M3_METRICS.iter().any(|spec| spec.name == removed),
            "{removed} is never emitted and must not be registered"
        );
    }
}
