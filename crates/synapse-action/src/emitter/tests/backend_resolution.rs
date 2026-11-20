use std::sync::{Arc, RwLock};

use super::*;
use crate::BackendResolutionPolicy;

#[tokio::test]
async fn must_pass_actor_uses_profile_backend_resolution_policy_for_auto_keydown() {
    let cancel = CancellationToken::new();
    let recording = Arc::new(RecordingBackend::new());
    let backend = Arc::clone(&recording) as Arc<dyn ActionBackend>;
    let policy = Arc::new(RwLock::new(BackendResolutionPolicy {
        default_backend: Backend::Hardware,
        keyboard_default: Backend::Auto,
        mouse_default: Backend::Auto,
        pad_default: Backend::Auto,
    }));
    let (handle, snapshot_handle, emitter) =
        ActionEmitter::channel_with_backends_and_policy(Backends::all_routed_to(backend), policy);
    let join = tokio::spawn(emitter.run(cancel.clone()));
    let key = key_named("profile-hardware");

    let before = snapshot_or_panic(&snapshot_handle).await;
    println!(
        "readback=actor_backend_resolution edge=before snapshot={before:?} backend_held={:?}",
        recording.held_keys()
    );
    assert!(before.held_keys.is_empty());

    handle
        .execute(Action::KeyDown {
            key: key.clone(),
            backend: Backend::Auto,
        })
        .await
        .unwrap_or_else(|error| {
            panic!("auto keydown should execute through recording backend: {error}")
        });

    let after = snapshot_or_panic(&snapshot_handle).await;
    println!(
        "readback=actor_backend_resolution edge=after_auto_keydown snapshot={after:?} backend_held={:?}",
        recording.held_keys()
    );
    assert!(
        after
            .held_keys_by_backend
            .get(&ResolvedBackend::Hardware)
            .is_some_and(|keys| keys.contains(&key))
    );
    assert!(
        !after
            .held_keys_by_backend
            .contains_key(&ResolvedBackend::Software)
    );
    assert!(recording.held_keys().contains(&key.code));

    cancel.cancel();
    let final_snapshot = join_actor_or_panic(join).await;
    assert!(final_snapshot.held_keys.is_empty());
}

#[tokio::test]
async fn must_fail_poisoned_backend_resolution_policy_refuses_auto_keydown_without_side_effects() {
    let cancel = CancellationToken::new();
    let recording = Arc::new(RecordingBackend::new());
    let backend = Arc::clone(&recording) as Arc<dyn ActionBackend>;
    let policy = Arc::new(RwLock::new(BackendResolutionPolicy::auto()));
    let poisoner = Arc::clone(&policy);
    let poisoned = std::thread::spawn(move || {
        let _guard = poisoner
            .write()
            .unwrap_or_else(|_poison| panic!("fresh policy lock should not be poisoned"));
        panic!("poison the backend resolution policy lock");
    })
    .join();
    assert!(poisoned.is_err(), "poisoning thread must panic");
    assert!(policy.is_poisoned());

    let (handle, snapshot_handle, emitter) =
        ActionEmitter::channel_with_backends_and_policy(Backends::all_routed_to(backend), policy);
    let join = tokio::spawn(emitter.run(cancel.clone()));
    let key = key_named("poisoned-policy");

    let result = handle
        .execute(Action::KeyDown {
            key: key.clone(),
            backend: Backend::Auto,
        })
        .await;

    match result {
        Err(crate::ActionError::BackendUnavailable { detail }) => {
            assert!(
                detail.contains(error_codes::ACTION_BACKEND_UNAVAILABLE),
                "detail={detail}"
            );
        }
        other => panic!("poisoned policy must fail closed with BackendUnavailable, got {other:?}"),
    }
    let after = snapshot_or_panic(&snapshot_handle).await;
    assert!(after.held_keys.is_empty(), "snapshot={after:?}");
    assert!(after.held_keys_by_backend.is_empty(), "snapshot={after:?}");
    assert!(
        recording.held_keys().is_empty(),
        "backend must not receive the refused keydown"
    );

    cancel.cancel();
    let final_snapshot = join_actor_or_panic(join).await;
    assert!(final_snapshot.held_keys.is_empty());
}

#[tokio::test]
async fn must_fail_explicit_software_request_is_not_overridden_by_hardware_profile_policy() {
    let cancel = CancellationToken::new();
    let recording = Arc::new(RecordingBackend::new());
    let backend = Arc::clone(&recording) as Arc<dyn ActionBackend>;
    let policy = Arc::new(RwLock::new(BackendResolutionPolicy {
        default_backend: Backend::Hardware,
        keyboard_default: Backend::Hardware,
        mouse_default: Backend::Hardware,
        pad_default: Backend::Hardware,
    }));
    let (handle, snapshot_handle, emitter) =
        ActionEmitter::channel_with_backends_and_policy(Backends::all_routed_to(backend), policy);
    let join = tokio::spawn(emitter.run(cancel.clone()));
    let key = key_named("explicit-software");

    handle
        .execute(Action::KeyDown {
            key: key.clone(),
            backend: Backend::Software,
        })
        .await
        .unwrap_or_else(|error| panic!("explicit software keydown should execute: {error}"));

    let after = snapshot_or_panic(&snapshot_handle).await;
    assert!(
        !after
            .held_keys_by_backend
            .contains_key(&ResolvedBackend::Hardware),
        "profile policy must not reroute an explicit backend: {after:?}"
    );
    assert!(
        after
            .held_keys_by_backend
            .get(&ResolvedBackend::Software)
            .is_some_and(|keys| keys.contains(&key)),
        "snapshot={after:?}"
    );

    cancel.cancel();
    let final_snapshot = join_actor_or_panic(join).await;
    assert!(final_snapshot.held_keys.is_empty());
}
