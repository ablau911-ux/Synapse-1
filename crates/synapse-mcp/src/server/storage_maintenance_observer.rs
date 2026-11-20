//! Service fixture forwarding into the actual per-DB maintenance observer.
type Callback = std::sync::Arc<dyn Fn(&'static str) + Send + Sync>;

impl super::SynapseService {
    pub(crate) fn set_storage_maintenance_observer_for_test(&self, observer: Callback) {
        if let Ok(db) = self.m3_storage() {
            db.set_storage_maintenance_observer_for_test(observer);
        }
    }
}
