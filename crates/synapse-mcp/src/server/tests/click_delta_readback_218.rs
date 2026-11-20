//! #218 regression: the `foreground_hwnd` in a target-scoped verify_delta
//! signature is the TARGET window (the signature is observed through the target
//! window), not the OS foreground. A PostMessage/background click readback that
//! shows only that field made a background click look like it ran in the
//! foreground. The delta readback now also names the real OS foreground, outside
//! the hashed signatures so a human switching windows never counts as a delta.

use super::*;

#[test]
fn must_pass_readback_names_the_real_os_foreground_separately_from_the_target() {
    let before = tests::click_signature(0x100, 10, "app.exe", "App", 1);
    let after = tests::click_signature(0x100, 10, "app.exe", "App", 1);
    let readback = click_delta_readback(&before, &after, Some(0x9999));
    assert_eq!(readback["os_foreground_hwnd"], json!(0x9999));
    assert_eq!(readback["after"]["foreground_hwnd"], json!(0x100));
    assert_eq!(readback["observed_window_hwnd"], json!(0x100));
}

#[test]
fn must_fail_readback_reports_null_when_the_os_has_no_foreground() {
    let before = tests::click_signature(0x100, 10, "app.exe", "App", 1);
    let after = tests::click_signature(0x100, 10, "app.exe", "App", 1);
    let readback = click_delta_readback(&before, &after, None);
    assert!(readback["os_foreground_hwnd"].is_null());
}

#[test]
fn must_fail_os_foreground_is_not_part_of_the_compared_signature() {
    // Changing which window the human has in front must not alter the compared
    // signature or the delta verdict, or an unrelated focus switch would be
    // reported as an action delta.
    let before = tests::click_signature(0x100, 10, "app.exe", "App", 1);
    let after = tests::click_signature(0x100, 10, "app.exe", "App", 1);

    // Guard: the hashed/compared signature type carries no OS-foreground field.
    let wire = serde_json::to_value(&before).expect("signature serializes");
    let keys: Vec<&String> = wire
        .as_object()
        .expect("signature is an object")
        .keys()
        .collect();
    assert!(
        keys.iter().all(|key| !key.contains("os_foreground")),
        "the compared signature must not carry the OS foreground: {keys:?}"
    );

    // Only the out-of-signature `os_foreground_hwnd` field may differ between
    // two readbacks of the same signatures under different OS foregrounds.
    let mut with_a = click_delta_readback(&before, &after, Some(1));
    let mut with_b = click_delta_readback(&before, &after, Some(2));
    assert_eq!(with_a["os_foreground_hwnd"], json!(1));
    assert_eq!(with_b["os_foreground_hwnd"], json!(2));
    assert_eq!(with_a["focus_only_change"], json!(false));
    for readback in [&mut with_a, &mut with_b] {
        readback
            .as_object_mut()
            .expect("readback is an object")
            .remove("os_foreground_hwnd");
    }
    assert_eq!(
        with_a, with_b,
        "the OS foreground must not leak into any other readback field"
    );
}
