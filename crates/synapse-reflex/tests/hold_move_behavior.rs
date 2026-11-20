use std::{collections::BTreeSet, sync::Arc, time::Duration};

use chrono::Utc;
use synapse_action::{
    ActionBackend, ActionEmitter, ActionEmitterSnapshotHandle, ActionHandle, ActionStateSnapshot,
    HELD_KEY_MAX_DURATION_MS, RecordedInput, RecordingBackend,
};
use synapse_core::{
    Action, Backend, ButtonAction, Event, EventFilter, EventSource, Key, KeyCode, MouseButton,
    ReflexButtonTarget, ReflexLifetime, error_codes,
};
use synapse_reflex::{
    EventBus, HoldButtonController, HoldButtonOutput, HoldButtonParams, HoldLifetimeContext,
    HoldMoveController, HoldMoveOutput, HoldMoveParams, HoldMovePhase,
    REFLEX_LIFETIME_EXPIRED_KIND, ReflexError, ReflexRuntime, ScheduledReflex,
};
use synapse_storage::Db;
use tempfile::tempdir;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

mod support;
use support::{claimed_target_gate, targeted};

#[test]
fn must_pass_hold_move_until_event_releases_key_once() -> Result<(), Box<dyn std::error::Error>> {
    let key = named_key("w");
    let lifetime = ReflexLifetime::UntilEvent {
        filter: EventFilter::Kind {
            kind: "stop".to_owned(),
        },
    };
    let mut controller =
        HoldMoveController::new("hold-event", HoldMoveParams::new(key.clone()), lifetime)?;
    let bus = EventBus::default();
    let (handle, mut rx) = ActionHandle::channel();

    let before = drain(&mut rx);
    let registered = controller.register_dispatch(&handle)?;
    let after_register = drain(&mut rx);
    let error =
        controller.step_dispatch(&context(2_000, &[event("stop", 1)], false), &handle, &bus);
    let after_event = drain(&mut rx);
    let duplicate = controller.step_dispatch(&context(1, &[], false), &handle, &bus)?;
    let after_duplicate = drain(&mut rx);

    assert!(before.is_empty());
    assert_eq!(registered, HoldMoveOutput::Registered { actions: 1 });
    assert_eq!(
        after_register,
        vec![Action::KeyDown {
            key: key.clone(),
            backend: Backend::Software,
        }]
    );
    assert!(matches!(
        error,
        Err(ReflexError::LifetimeExpired { ref reflex_id }) if reflex_id == "hold-event"
    ));
    assert_eq!(
        after_event,
        vec![Action::KeyUp {
            key,
            backend: Backend::Software,
        }]
    );
    assert_eq!(
        duplicate,
        HoldMoveOutput::Idle {
            reason: "not_holding"
        }
    );
    assert!(after_duplicate.is_empty());
    Ok(())
}

/// An event whose kind does not match the UntilEvent filter must neither
/// release the key nor end the hold.
#[test]
fn must_pass_hold_move_ignores_non_matching_event() -> Result<(), Box<dyn std::error::Error>> {
    let key = named_key("w");
    let lifetime = ReflexLifetime::UntilEvent {
        filter: EventFilter::Kind {
            kind: "stop".to_owned(),
        },
    };
    let mut controller =
        HoldMoveController::new("hold-event-other", HoldMoveParams::new(key), lifetime)?;
    let bus = EventBus::default();
    let (handle, mut rx) = ActionHandle::channel();

    controller.register_dispatch(&handle)?;
    assert_eq!(drain(&mut rx).len(), 1, "KeyDown on register");
    let output =
        controller.step_dispatch(&context(20, &[event("other", 1)], false), &handle, &bus)?;

    assert_eq!(output, HoldMoveOutput::Holding { elapsed_ms: 20 });
    assert!(
        drain(&mut rx).is_empty(),
        "no KeyUp for a non-matching event"
    );
    assert_eq!(controller.phase(), HoldMovePhase::Holding);
    Ok(())
}

#[test]
fn must_fail_hold_move_zero_duration_releases_immediately() -> Result<(), Box<dyn std::error::Error>>
{
    let key = named_key("w");
    let mut controller = HoldMoveController::new(
        "hold-zero",
        HoldMoveParams::new(key.clone()),
        ReflexLifetime::Duration { ms: 0 },
    )?;
    let bus = EventBus::default();
    let (handle, mut rx) = ActionHandle::channel();

    controller.register_dispatch(&handle)?;
    let down = drain(&mut rx);
    let result = controller.step_dispatch(&context(0, &[], false), &handle, &bus);
    let up = drain(&mut rx);

    assert!(matches!(result, Err(ReflexError::LifetimeExpired { .. })));
    assert_eq!(
        down,
        vec![Action::KeyDown {
            key: key.clone(),
            backend: Backend::Software,
        }]
    );
    assert_eq!(
        up,
        vec![Action::KeyUp {
            key,
            backend: Backend::Software,
        }]
    );
    assert_eq!(controller.phase(), HoldMovePhase::Released);
    Ok(())
}

#[test]
fn must_pass_hold_move_external_cancel_releases_until_cancelled()
-> Result<(), Box<dyn std::error::Error>> {
    let key = named_key("w");
    let mut controller = HoldMoveController::new(
        "hold-cancel",
        HoldMoveParams::new(key.clone()),
        ReflexLifetime::UntilCancelled,
    )?;
    let bus = EventBus::default();
    let (handle, mut rx) = ActionHandle::channel();

    controller.register_dispatch(&handle)?;
    let after_register = drain(&mut rx);
    let result = controller.step_dispatch(&context(100, &[], true), &handle, &bus);
    let after_cancel = drain(&mut rx);

    assert!(matches!(result, Err(ReflexError::LifetimeExpired { .. })));
    assert_eq!(
        after_register,
        vec![Action::KeyDown {
            key: key.clone(),
            backend: Backend::Software,
        }]
    );
    assert_eq!(
        after_cancel,
        vec![Action::KeyUp {
            key,
            backend: Backend::Software,
        }]
    );
    Ok(())
}

#[test]
fn must_pass_hold_move_reasserts_keydown_while_holding_when_enabled()
-> Result<(), Box<dyn std::error::Error>> {
    let key = named_key("w");
    let mut params = HoldMoveParams::new(key.clone());
    params.re_assert = true;
    let mut controller =
        HoldMoveController::new("hold-reassert", params, ReflexLifetime::UntilCancelled)?;
    let bus = EventBus::default();
    let (handle, mut rx) = ActionHandle::channel();

    controller.register_dispatch(&handle)?;
    let after_register = drain(&mut rx);
    let early = controller.step_dispatch(&context(1, &[], false), &handle, &bus)?;
    let after_early = drain(&mut rx);
    let output = controller.step_dispatch(&context(50, &[], false), &handle, &bus)?;
    let after_step = drain(&mut rx);

    assert_eq!(
        after_register,
        vec![Action::KeyDown {
            key: key.clone(),
            backend: Backend::Software,
        }]
    );
    assert_eq!(early, HoldMoveOutput::Holding { elapsed_ms: 1 });
    assert!(after_early.is_empty());
    assert_eq!(
        output,
        HoldMoveOutput::Reasserted {
            elapsed_ms: 51,
            actions: 1
        }
    );
    assert_eq!(
        after_step,
        vec![Action::KeyDown {
            key,
            backend: Backend::Software,
        }]
    );
    Ok(())
}

#[test]
fn must_fail_hold_move_safety_cap_expires_after_held_key_limit()
-> Result<(), Box<dyn std::error::Error>> {
    let key = named_key("w");
    let mut controller = HoldMoveController::new(
        "hold-cap",
        HoldMoveParams::new(key.clone()),
        ReflexLifetime::UntilCancelled,
    )?;
    let bus = EventBus::default();
    let subscriber = bus.subscribe(
        EventFilter::Kind {
            kind: REFLEX_LIFETIME_EXPIRED_KIND.to_owned(),
        },
        Vec::new(),
        false,
    )?;
    let (handle, mut rx) = ActionHandle::channel();

    controller.register_dispatch(&handle)?;
    let down = drain(&mut rx);
    let result = controller.step_dispatch(
        &context(HELD_KEY_MAX_DURATION_MS + 1_001, &[], false),
        &handle,
        &bus,
    );
    let up = drain(&mut rx);
    let events = subscriber.drain();

    assert!(matches!(result, Err(ReflexError::LifetimeExpired { .. })));
    assert_eq!(
        down,
        vec![Action::KeyDown {
            key: key.clone(),
            backend: Backend::Software,
        }]
    );
    assert_eq!(
        up,
        vec![Action::KeyUp {
            key,
            backend: Backend::Software,
        }]
    );
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].data["code"], error_codes::REFLEX_LIFETIME_EXPIRED);
    assert_eq!(events[0].data["reason"], "safety_cap");
    Ok(())
}

/// Safety cap = HELD_KEY_MAX_DURATION_MS + 1000 ms grace, inclusive: 1 ms
/// below it the key stays held; the tick that reaches it releases. Pins `>=`
/// against `>` and against a cap without the grace.
#[test]
fn must_pass_hold_move_below_safety_cap_keeps_holding() -> Result<(), Box<dyn std::error::Error>> {
    let key = named_key("w");
    let mut controller = HoldMoveController::new(
        "hold-below-cap",
        HoldMoveParams::new(key.clone()),
        ReflexLifetime::UntilCancelled,
    )?;
    let bus = EventBus::default();
    let (handle, mut rx) = ActionHandle::channel();

    controller.register_dispatch(&handle)?;
    assert_eq!(drain(&mut rx).len(), 1, "KeyDown on register");
    let below_cap_ms = HELD_KEY_MAX_DURATION_MS + 999;
    let output = controller.step_dispatch(&context(below_cap_ms, &[], false), &handle, &bus)?;
    assert_eq!(
        output,
        HoldMoveOutput::Holding {
            elapsed_ms: u128::from(below_cap_ms)
        }
    );
    assert!(drain(&mut rx).is_empty(), "no KeyUp below the safety cap");

    let at_cap = controller.step_dispatch(&context(1, &[], false), &handle, &bus);
    assert!(matches!(at_cap, Err(ReflexError::LifetimeExpired { .. })));
    assert_eq!(
        drain(&mut rx),
        vec![Action::KeyUp {
            key,
            backend: Backend::Software,
        }]
    );
    Ok(())
}

#[test]
fn must_pass_hold_button_mouse_uses_down_up_actions() -> Result<(), Box<dyn std::error::Error>> {
    let mut controller = HoldButtonController::new(
        "button-duration",
        HoldButtonParams::new(ReflexButtonTarget::Mouse {
            button: MouseButton::Left,
        }),
        ReflexLifetime::Duration { ms: 10 },
    )?;
    let bus = EventBus::default();
    let (handle, mut rx) = ActionHandle::channel();

    let registered = controller.register_dispatch(&handle)?;
    let after_register = drain(&mut rx);
    let result = controller.step_dispatch(&context(10, &[], false), &handle, &bus);
    let after_duration = drain(&mut rx);

    assert_eq!(registered, HoldButtonOutput::Registered);
    assert!(matches!(result, Err(ReflexError::LifetimeExpired { .. })));
    assert_eq!(
        after_register,
        vec![Action::MouseButton {
            button: MouseButton::Left,
            action: ButtonAction::Down,
            hold_ms: 0,
            backend: Backend::Software,
        }]
    );
    assert_eq!(
        after_duration,
        vec![Action::MouseButton {
            button: MouseButton::Left,
            action: ButtonAction::Up,
            hold_ms: 0,
            backend: Backend::Software,
        }]
    );
    Ok(())
}

/// Duration 10 ms: at 9 ms the button stays down (no Up), the 10th ms
/// releases. Pins the duration comparison against an off-by-one.
#[test]
fn must_pass_hold_button_before_duration_keeps_button_down()
-> Result<(), Box<dyn std::error::Error>> {
    let mut controller = HoldButtonController::new(
        "button-before-duration",
        HoldButtonParams::new(ReflexButtonTarget::Mouse {
            button: MouseButton::Left,
        }),
        ReflexLifetime::Duration { ms: 10 },
    )?;
    let bus = EventBus::default();
    let (handle, mut rx) = ActionHandle::channel();

    controller.register_dispatch(&handle)?;
    assert_eq!(drain(&mut rx).len(), 1, "Down on register");
    let output = controller.step_dispatch(&context(9, &[], false), &handle, &bus)?;
    assert_eq!(output, HoldButtonOutput::Holding { elapsed_ms: 9 });
    assert!(
        drain(&mut rx).is_empty(),
        "no Up before the duration elapses"
    );

    let result = controller.step_dispatch(&context(1, &[], false), &handle, &bus);
    assert!(matches!(result, Err(ReflexError::LifetimeExpired { .. })));
    assert_eq!(
        drain(&mut rx),
        vec![Action::MouseButton {
            button: MouseButton::Left,
            action: ButtonAction::Up,
            hold_ms: 0,
            backend: Backend::Software,
        }]
    );
    Ok(())
}

#[tokio::test]
async fn must_pass_hold_move_and_external_actions_share_action_emitter_state()
-> Result<(), Box<dyn std::error::Error>> {
    let cancel = CancellationToken::new();
    let recording = Arc::new(RecordingBackend::new());
    let backend: Arc<dyn ActionBackend> = recording.clone();
    let (handle, snapshot_handle, join) =
        ActionEmitter::spawn_with_backend(cancel.clone(), backend);
    let reflex_key = named_key("w");
    let external_key = named_key("a");
    let mut controller = HoldMoveController::new(
        "hold-interlock",
        HoldMoveParams::new(reflex_key.clone()),
        ReflexLifetime::UntilCancelled,
    )?;

    let before = snapshot_handle.snapshot().await?;
    println!(
        "readback=action_interlock edge=before snapshot={before:?} backend_held={:?}",
        recording.held_keys()
    );
    assert!(before.held_keys.is_empty());
    assert!(recording.held_keys().is_empty());

    controller.register_dispatch(&handle)?;
    let after_reflex = wait_for_snapshot(&snapshot_handle, |snapshot| {
        snapshot_has_exact_keys(snapshot, &[&reflex_key])
    })
    .await?;
    println!(
        "readback=action_interlock edge=after_reflex_hold snapshot={after_reflex:?} backend_held={:?}",
        recording.held_keys()
    );
    assert_eq!(
        recording.held_keys(),
        BTreeSet::from([reflex_key.code.clone()])
    );

    handle
        .execute(Action::KeyDown {
            key: external_key.clone(),
            backend: Backend::Software,
        })
        .await?;
    let after_external = wait_for_snapshot(&snapshot_handle, |snapshot| {
        snapshot_has_exact_keys(snapshot, &[&external_key, &reflex_key])
    })
    .await?;
    println!(
        "readback=action_interlock edge=after_external_tool_hold snapshot={after_external:?} backend_held={:?}",
        recording.held_keys()
    );
    assert_eq!(
        recording.held_keys(),
        BTreeSet::from([external_key.code.clone(), reflex_key.code.clone()])
    );

    handle.execute(Action::ReleaseAll).await?;
    let after_release_all =
        wait_for_snapshot(&snapshot_handle, |snapshot| snapshot.held_keys.is_empty()).await?;
    let events = recording.events();
    println!(
        "readback=action_interlock edge=after_release_all snapshot={after_release_all:?} backend_held={:?} events={events:?}",
        recording.held_keys()
    );
    assert!(recording.held_keys().is_empty());
    assert!(matches!(
        events.last(),
        Some(RecordedInput::ReleaseAll { held_keys, .. })
            if held_keys == &vec![external_key.code.clone(), reflex_key.code.clone()]
    ));

    cancel.cancel();
    let final_snapshot = join.await?;
    assert!(final_snapshot.held_keys.is_empty());
    Ok(())
}

#[test]
fn must_pass_runtime_cancel_hold_move_queues_keyup() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let db = Arc::new(Db::open(
        &temp.path().join("db"),
        synapse_core::SCHEMA_VERSION,
    )?);
    let bus = EventBus::default();
    let (handle, mut rx) = ActionHandle::channel();
    let mut runtime = ReflexRuntime::spawn(db, handle, bus)?;
    runtime.set_action_gate(Some(claimed_target_gate()));
    let key = named_key("w");
    let reflex = targeted(ScheduledReflex::hold_move(
        "runtime-cancel-hold",
        HoldMoveParams::new(key.clone()),
    ));

    runtime.register(&reflex)?;
    let down = wait_for_action(
        &mut rx,
        |action| matches!(action, Action::KeyDown { key: observed, .. } if observed == &key),
    )?;
    let outcome = runtime.cancel("runtime-cancel-hold")?;
    let up = wait_for_action(
        &mut rx,
        |action| matches!(action, Action::KeyUp { key: observed, .. } if observed == &key),
    )?;

    assert!(matches!(
        outcome,
        synapse_reflex::ReflexCancelOutcome::Cancelled { .. }
    ));
    assert_eq!(
        down,
        Action::KeyDown {
            key: key.clone(),
            backend: Backend::Software,
        }
    );
    assert_eq!(
        up,
        Action::KeyUp {
            key,
            backend: Backend::Software,
        }
    );
    Ok(())
}

const fn context(elapsed_ms: u64, events: &[Event], cancelled: bool) -> HoldLifetimeContext<'_> {
    HoldLifetimeContext {
        tick_elapsed: Duration::from_millis(elapsed_ms),
        events,
        cancelled,
    }
}

fn drain(rx: &mut mpsc::Receiver<synapse_action::ActionMessage>) -> Vec<Action> {
    let mut actions = Vec::new();
    while let Ok((action, _ack)) = rx.try_recv() {
        actions.push(action);
    }
    actions
}

fn wait_for_action(
    rx: &mut mpsc::Receiver<synapse_action::ActionMessage>,
    predicate: impl Fn(&Action) -> bool,
) -> Result<Action, Box<dyn std::error::Error>> {
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        while let Ok((action, _ack)) = rx.try_recv() {
            if predicate(&action) {
                return Ok(action);
            }
        }
        if std::time::Instant::now() >= deadline {
            return Err("timed out waiting for action".into());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

async fn wait_for_snapshot(
    snapshot_handle: &ActionEmitterSnapshotHandle,
    predicate: impl Fn(&ActionStateSnapshot) -> bool,
) -> Result<ActionStateSnapshot, Box<dyn std::error::Error>> {
    // 3 s deadline like `wait_for_action`; 200 ms was load-sensitive.
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        let snapshot = snapshot_handle.snapshot().await?;
        if predicate(&snapshot) {
            return Ok(snapshot);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        "action emitter snapshot did not reach expected held state",
    )
    .into())
}

fn snapshot_has_exact_keys(snapshot: &ActionStateSnapshot, keys: &[&Key]) -> bool {
    snapshot.held_keys.len() == keys.len()
        && keys
            .iter()
            .all(|key| snapshot.held_keys.iter().any(|held| held == *key))
}

fn event(kind: &str, seq: u64) -> Event {
    Event {
        seq,
        at: Utc::now(),
        source: EventSource::Reflex,
        kind: kind.to_owned(),
        data: serde_json::json!({}),
        correlations: Vec::new(),
    }
}

fn named_key(value: &str) -> Key {
    Key {
        code: KeyCode::Named {
            value: value.to_owned(),
        },
        use_scancode: false,
    }
}
