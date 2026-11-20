//! Regression tests for #32: an `IsPassword` read ERROR must be treated as
//! "maybe password" everywhere (fail closed), including the fallback runtime id
//! name hash. One shared decision, not per-call-site `unwrap_or(false)`.

use super::{gated_text, maybe_password};

#[test]
fn must_fail_unreadable_password_flag_is_treated_as_a_password() {
    assert!(maybe_password(None), "read error must fail closed");
    assert!(maybe_password(Some(true)));
    assert!(!maybe_password(Some(false)));
}

#[test]
fn must_pass_gated_text_is_withheld_unless_the_flag_is_explicitly_false() {
    assert_eq!(gated_text(None, || "hunter2".to_owned()), "");
    assert_eq!(gated_text(Some(true), || "hunter2".to_owned()), "");
    assert_eq!(
        gated_text(Some(false), || "Username".to_owned()),
        "Username"
    );
}

#[test]
fn must_fail_gated_text_does_not_read_the_text_when_withheld() {
    let mut read = false;
    let _ = gated_text(None, || {
        read = true;
        "secret".to_owned()
    });
    assert!(
        !read,
        "the name must not even be fetched for a maybe-password"
    );
}
