//! Crate-wide serialization for unit tests that touch the process-global
//! operator release epoch (`hotkey::OPERATOR_RELEASE_EPOCH`).
//!
//! Every unit test in this crate that READS the epoch and depends on its value,
//! or BUMPS it (directly through `request_release_interrupt`, or indirectly by
//! sending `Action::ReleaseAll` through an `ActionHandle`, which calls
//! `request_release_interrupt` on enqueue), must hold this one lock for the
//! whole test. File-local locks do not serialize against each other, so an
//! interrupt raised by one file's test could land inside another file's run.
//!
//! It is a `tokio` mutex so `#[tokio::test]` bodies can hold the guard across
//! `.await` points (`clippy::await_holding_lock` is denied for `std` guards).
//! Synchronous tests take it with `blocking_lock()`; async tests with
//! `lock().await`.

pub(crate) static RELEASE_EPOCH_TEST_LOCK: tokio::sync::Mutex<()> =
    tokio::sync::Mutex::const_new(());
