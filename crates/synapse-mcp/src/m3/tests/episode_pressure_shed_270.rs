//! #270: episode_segment's pressure gate refuses a write with
//! `STORAGE_WRITE_SHED` like `routine_mine` (#165), same data shape, nothing
//! written; a dry run writes nothing and is not refused by the gate.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};

use synapse_action::ActionHandle;
use synapse_core::SCHEMA_VERSION;
use synapse_reflex::{EventBus, ReflexRuntime};
use synapse_storage::Db;

use super::{EpisodeSegmentParams, segment_episodes};
use crate::m3::tests::pressure_shed_support_270::{assert_shed, force_pressure, snapshot};

fn runtime_over_db() -> (tempfile::TempDir, Arc<Db>, Arc<Mutex<ReflexRuntime>>) {
    let temp = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(Db::open(&temp.path().join("db"), SCHEMA_VERSION).expect("db"));
    let (action, _rx) = ActionHandle::channel();
    let runtime =
        ReflexRuntime::spawn(Arc::clone(&db), action, EventBus::default()).expect("reflex runtime");
    (temp, db, Arc::new(Mutex::new(runtime)))
}

#[test]
fn must_fail_episode_segment_pressure_refusal_is_shed_and_writes_nothing_270() {
    let (_temp, db, _runtime) = runtime_over_db();
    force_pressure(&db);
    let before = snapshot(&db);
    let Err(error) = segment_episodes(&db, &EpisodeSegmentParams::default()) else {
        panic!("pressure must refuse the episode write");
    };
    assert_shed(&error, "episode_segment");
    assert_eq!(snapshot(&db), before, "nothing was deleted or written");
}

/// Guard: a dry run mutates nothing, so the pressure gate must not refuse it.
#[test]
fn must_pass_episode_segment_dry_run_is_not_refused_by_the_pressure_gate_270() {
    let (_temp, db, _runtime) = runtime_over_db();
    force_pressure(&db);
    let before = snapshot(&db);
    let params = EpisodeSegmentParams {
        dry_run: true,
        ..EpisodeSegmentParams::default()
    };
    if let Err(error) = segment_episodes(&db, &params) {
        assert!(
            !error.message.contains("disk pressure"),
            "a dry run must not be shed: {error:?}"
        );
    }
    assert_eq!(snapshot(&db), before);
}
