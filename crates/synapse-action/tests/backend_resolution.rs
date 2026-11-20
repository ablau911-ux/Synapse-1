use synapse_action::{
    BackendResolutionPolicy, ResolvedBackend, resolve_backend, resolve_backend_with_policy,
};
use synapse_core::{Action, Backend, ButtonAction, Key, KeyCode, MouseButton, PadButton};

#[test]
fn must_pass_explicit_backends_are_honoured() {
    // An explicit backend is used as requested, for any action class; only
    // `Backend::Auto` consults the action class and policy (covered below).
    for action in [
        key_down_action(),
        mouse_button_action(),
        pad_button_action(),
    ] {
        assert_backend(
            "explicit_software",
            Backend::Software,
            &action,
            ResolvedBackend::Software,
        );
        assert_backend(
            "explicit_vigem",
            Backend::Vigem,
            &action,
            ResolvedBackend::Vigem,
        );
        assert_backend(
            "explicit_hardware",
            Backend::Hardware,
            &action,
            ResolvedBackend::Hardware,
        );
    }
}

#[test]
fn must_pass_auto_backend_routes_keyboard_mouse_and_pad_actions() {
    assert_backend(
        "auto_keyboard",
        Backend::Auto,
        &key_down_action(),
        ResolvedBackend::Software,
    );
    assert_backend(
        "auto_mouse",
        Backend::Auto,
        &mouse_button_action(),
        ResolvedBackend::Software,
    );
    assert_backend(
        "auto_pad",
        Backend::Auto,
        &pad_button_action(),
        ResolvedBackend::Vigem,
    );
}

#[test]
fn must_pass_profile_default_hardware_policy_routes_auto_to_hardware() {
    let policy = BackendResolutionPolicy {
        default_backend: Backend::Hardware,
        keyboard_default: Backend::Auto,
        mouse_default: Backend::Auto,
        pad_default: Backend::Auto,
    };

    assert_backend_with_policy(
        "profile_hardware_keyboard",
        Backend::Auto,
        &key_down_action(),
        policy,
        ResolvedBackend::Hardware,
    );
    assert_backend_with_policy(
        "profile_hardware_mouse",
        Backend::Auto,
        &mouse_button_action(),
        policy,
        ResolvedBackend::Hardware,
    );
    assert_backend_with_policy(
        "profile_hardware_pad",
        Backend::Auto,
        &pad_button_action(),
        policy,
        ResolvedBackend::Hardware,
    );
    assert_backend_with_policy(
        "profile_hardware_release_all",
        Backend::Auto,
        &Action::ReleaseAll,
        policy,
        ResolvedBackend::Hardware,
    );
}

#[test]
fn must_pass_class_defaults_override_profile_default_for_auto_only() {
    let policy = BackendResolutionPolicy {
        default_backend: Backend::Hardware,
        keyboard_default: Backend::Software,
        mouse_default: Backend::Auto,
        pad_default: Backend::Vigem,
    };

    assert_backend_with_policy(
        "class_keyboard_software",
        Backend::Auto,
        &key_down_action(),
        policy,
        ResolvedBackend::Software,
    );
    assert_backend_with_policy(
        "class_pad_vigem",
        Backend::Auto,
        &pad_button_action(),
        policy,
        ResolvedBackend::Vigem,
    );
    // `mouse_default: Auto` sets no class override, so mouse falls through to
    // the profile default (Hardware).
    assert_backend_with_policy(
        "class_mouse_auto_falls_through_to_profile_hardware",
        Backend::Auto,
        &mouse_button_action(),
        policy,
        ResolvedBackend::Hardware,
    );
    assert_backend_with_policy(
        "explicit_software_ignores_profile_hardware",
        Backend::Software,
        &key_down_action(),
        policy,
        ResolvedBackend::Software,
    );
}

fn assert_backend(edge: &str, requested: Backend, action: &Action, expected: ResolvedBackend) {
    let resolved = resolve_backend(requested, action)
        .unwrap_or_else(|err| panic!("{edge} should resolve backend, got {err}"));
    assert_eq!(resolved, expected);
    println!(
        "readback=backend_resolution edge={edge} before_backend={requested:?} after_backend={} result_value={:?}",
        resolved.as_str(),
        resolved
    );
}

fn assert_backend_with_policy(
    edge: &str,
    requested: Backend,
    action: &Action,
    policy: BackendResolutionPolicy,
    expected: ResolvedBackend,
) {
    let resolved = resolve_backend_with_policy(requested, action, policy)
        .unwrap_or_else(|err| panic!("{edge} should resolve backend, got {err}"));
    assert_eq!(resolved, expected);
    println!(
        "readback=backend_resolution edge={edge} policy={policy:?} before_backend={requested:?} after_backend={} result_value={resolved:?}",
        resolved.as_str()
    );
}

fn key_down_action() -> Action {
    Action::KeyDown {
        key: Key {
            code: KeyCode::Named {
                value: "a".to_owned(),
            },
            use_scancode: false,
        },
        backend: Backend::Auto,
    }
}

const fn mouse_button_action() -> Action {
    Action::MouseButton {
        button: MouseButton::Left,
        action: ButtonAction::Down,
        hold_ms: 0,
        backend: Backend::Auto,
    }
}

const fn pad_button_action() -> Action {
    Action::PadButton {
        pad: 0,
        button: PadButton::A,
        action: ButtonAction::Down,
        hold_ms: 0,
    }
}
