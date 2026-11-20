//! NEEDS-SEAM: cached_password_status must call strict_password_variant(&Variant)
//! -> Option<bool>. Main has coercion inline. Wire as child of common.rs.
use super::*;

#[test]
fn must_pass_genuine_boolean_password_variants_preserve_their_value() {
    for value in [false, true] {
        let variant = Variant::from(value);
        assert_eq!(strict_password_variant(&variant), Some(value));
        assert_eq!(
            gated_text(strict_password_variant(&variant), || "plain".into()),
            if value { "" } else { "plain" }
        );
    }
}

#[test]
fn must_fail_coercible_nonboolean_variants_never_unlock_plaintext() {
    for variant in [
        Variant::from(0_i32),
        Variant::from(1_i32),
        Variant::from("false"),
        Variant::from("true"),
    ] {
        let reads = std::cell::Cell::new(0);
        assert_eq!(strict_password_variant(&variant), None);
        let text = gated_text(strict_password_variant(&variant), || {
            reads.set(reads.get() + 1);
            "SECRET_244".into()
        });
        assert_eq!(text, "");
        assert_eq!(reads.get(), 0, "source must not even be read");
    }
}
