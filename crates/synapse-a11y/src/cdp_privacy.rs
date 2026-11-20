//! Shared password redaction for typed CDP DOM serializers.

const PASSWORD_READBACK_JS: &str = include_str!("cdp_password_readback.js");

/// Bind the privacy helper inside an expression's execution context.
#[must_use]
pub fn password_safe_expression(expression: &str) -> String {
    format!("(() => {{ {PASSWORD_READBACK_JS}\nreturn ({expression}); }})()")
}

/// Bind the privacy helper in the same realm as the inspected element.
#[must_use]
pub fn password_safe_function(function: &str) -> String {
    format!("function(...args) {{ {PASSWORD_READBACK_JS}\nreturn ({function})(...args); }}")
}
