//! Deterministic test-only entry to the real bounded expiry operations.
use super::*;

pub(in crate::server::agent_mailbox) fn cleanup_expired_mailbox_rows(
    db: &Arc<Db>,
    now_unix_ms: u64,
) -> Result<MailboxSweep, ErrorData> {
    // Keep the mutation lock through both operations. Calling cleanup while
    // holding this lock would fail its try_lock and silently skip the sweep.
    let _mutation = MAILBOX_DRAINS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut states = INDEX_STATES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let states = states.get_or_insert_with(std::collections::HashMap::new);
    states.retain(|_, (owner, _)| owner.strong_count() > 0);
    let (_, state) = states
        .entry(Arc::as_ptr(db) as usize)
        .or_insert_with(|| (Arc::downgrade(db), IndexState::default()));
    index_legacy_page(db, state)?;
    sweep_due_page(db, state, &|| now_unix_ms)
}
