//! CDP node-budget coverage (#77): web nodes beyond the gather budget are never
//! returned, so the observe/find coverage verdict must say the view is partial.

use synapse_core::SensorStatus;
use synapse_perception::ObservationInput;

use super::sources::a11y_status_for_snapshot;

/// True when the node cap was reached and the page exposed more accessible
/// nodes than were returned. Nodes the mapper skips for other reasons (no
/// backend id, duplicates) leave `returned < max_nodes` and do not count.
#[must_use]
pub(super) fn cdp_budget_truncated(returned: usize, total_ax_nodes: u32, max_nodes: usize) -> bool {
    returned >= max_nodes && usize::try_from(total_ax_nodes).is_ok_and(|total| total > returned)
}

/// Degrades a healthy accessibility status to the shared truncation verdict
/// when the Chromium renderer UIA supplement dropped nodes (cap or read error).
/// An already-degraded reason is kept.
pub(super) fn apply_renderer_supplement_coverage(input: &mut ObservationInput, truncated: bool) {
    if truncated && matches!(input.a11y_status, SensorStatus::Healthy) {
        input.a11y_status = a11y_status_for_snapshot(true);
    }
}

/// Degrades a healthy accessibility status to the shared truncation verdict
/// when the CDP budget cut the tree. An already-degraded reason is kept.
pub(super) fn apply_cdp_budget_coverage(
    input: &mut ObservationInput,
    returned: usize,
    total_ax_nodes: u32,
    max_nodes: usize,
) {
    if matches!(input.a11y_status, SensorStatus::Healthy)
        && cdp_budget_truncated(returned, total_ax_nodes, max_nodes)
    {
        input.a11y_status = a11y_status_for_snapshot(true);
    }
}
