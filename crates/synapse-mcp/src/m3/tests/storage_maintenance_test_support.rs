//! Test-only readbacks for `storage_maintenance` regression tests (#60).

use super::StorageMaintenance;

impl StorageMaintenance {
    /// Whether the periodic GC task is registered.
    pub(super) const fn gc_running(&self) -> bool {
        self._gc.is_some()
    }

    /// Whether the periodic disk-pressure task is registered.
    pub(super) const fn pressure_running(&self) -> bool {
        self._pressure.is_some()
    }
}
