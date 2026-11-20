//! Regression tests for #225: health/fired/unavailable logs must name the hotkey
//! that was actually configured (`ctrl+alt+shift+9`), not the compiled-in
//! default (`ctrl+alt+shift+p`) — also when the override fails to parse.

use super::{DEFAULT_OPERATOR_HOTKEY, HotkeyLabelSlot, requested_operator_hotkey_label};

#[test]
fn must_pass_label_reports_the_configured_hotkey_not_the_default() {
    let slot = HotkeyLabelSlot::new();
    slot.record("ctrl+alt+shift+9");
    assert_eq!(slot.get(), "ctrl+alt+shift+9");

    // Guard (#225 variants): every health/fired/unavailable log site must print
    // the configured label (`config.label` inside the hook, or
    // `configured_operator_hotkey_label()` outside it), never the compiled-in
    // default constant.
    let hotkey_src = include_str!("../hotkey.rs");
    let safety_src = include_str!("../../../synapse-mcp/src/safety.rs");
    for (file, src) in [("hotkey.rs", hotkey_src), ("safety.rs", safety_src)] {
        for (index, line) in src.lines().enumerate() {
            let trimmed = line.trim_start();
            let is_label_field = trimmed.starts_with("hotkey =") || trimmed.starts_with("hotkey:");
            if is_label_field {
                assert!(
                    !line.contains("DEFAULT_OPERATOR_HOTKEY"),
                    "{file}:{} logs the default hotkey instead of the configured one: {line}",
                    index + 1
                );
            }
        }
    }
    let hotkey_sites = hotkey_src
        .lines()
        .filter(|line| line.trim_start().starts_with("hotkey ="))
        .collect::<Vec<_>>();
    assert!(
        !hotkey_sites.is_empty()
            && hotkey_sites
                .iter()
                .all(|line| line.contains("config.label")),
        "hook log sites must print config.label: {hotkey_sites:?}"
    );
    assert!(
        safety_src
            .matches("configured_operator_hotkey_label()")
            .count()
            >= 2,
        "the safety fired/health sites must read configured_operator_hotkey_label()"
    );
}

/// Serializes the env override below (process-global env + one-shot label).
#[cfg(windows)]
static HOTKEY_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Drives the real `install_operator_hotkey` with an unparseable override set
/// only through the compat env var: install refuses (so no hook is armed), and
/// the process-wide label is the compat value as written, not the default.
#[cfg(windows)]
#[test]
fn must_pass_compat_env_label_used_when_primary_unset() {
    let _guard = HOTKEY_ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let saved_primary = std::env::var_os(super::OPERATOR_HOTKEY_ENV);
    let saved_compat = std::env::var_os(super::OPERATOR_HOTKEY_COMPAT_ENV);
    // SAFETY: the env vars are only read by `install_operator_hotkey`, which no
    // other test in this crate calls; HOTKEY_ENV_LOCK serializes this test.
    unsafe {
        std::env::remove_var(super::OPERATOR_HOTKEY_ENV);
        std::env::set_var(super::OPERATOR_HOTKEY_COMPAT_ENV, " ctrl+alt+bogus225 ");
    }
    let result = super::install_operator_hotkey(|| {});
    // SAFETY: as above; restore the caller's environment.
    unsafe {
        match saved_primary {
            Some(value) => std::env::set_var(super::OPERATOR_HOTKEY_ENV, value),
            None => std::env::remove_var(super::OPERATOR_HOTKEY_ENV),
        }
        match saved_compat {
            Some(value) => std::env::set_var(super::OPERATOR_HOTKEY_COMPAT_ENV, value),
            None => std::env::remove_var(super::OPERATOR_HOTKEY_COMPAT_ENV),
        }
    }

    assert!(
        matches!(result, Err(crate::ActionError::BackendUnavailable { .. })),
        "an unparseable override must refuse install: {:?}",
        result.as_ref().err()
    );
    assert_eq!(
        super::configured_operator_hotkey_label(),
        "ctrl+alt+bogus225",
        "the label must be the compat override as written, not {DEFAULT_OPERATOR_HOTKEY}"
    );
}

#[test]
fn must_pass_requested_label_is_the_override_as_written_or_the_default() {
    assert_eq!(
        requested_operator_hotkey_label(Some(" ctrl+alt+shift+9 ")),
        "ctrl+alt+shift+9"
    );
    assert_eq!(
        requested_operator_hotkey_label(None),
        DEFAULT_OPERATOR_HOTKEY
    );
}

#[test]
fn must_fail_a_second_record_does_not_overwrite_the_configured_label() {
    let slot = HotkeyLabelSlot::new();
    slot.record("ctrl+alt+shift+9");
    slot.record(DEFAULT_OPERATOR_HOTKEY);
    assert_eq!(slot.get(), "ctrl+alt+shift+9");
}

#[test]
fn must_fail_unparseable_or_blank_overrides_are_never_reported_as_the_default() {
    // An invalid override is reported as written, not silently as the default.
    assert_eq!(
        requested_operator_hotkey_label(Some("ctrl+bogus")),
        "ctrl+bogus"
    );
    // A blank override means "not set": the default is what is armed.
    assert_eq!(
        requested_operator_hotkey_label(Some("   ")),
        DEFAULT_OPERATOR_HOTKEY
    );
    // Nothing recorded yet reads the default.
    assert_eq!(HotkeyLabelSlot::new().get(), DEFAULT_OPERATOR_HOTKEY);
}
