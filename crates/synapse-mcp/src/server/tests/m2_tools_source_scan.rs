//! Shared test support: reads `impl` method bodies out of m2_tools.rs so a test
//! can pin which helper a real call site uses (#191, #218).

const SOURCE: &str = include_str!("../m2_tools.rs");

/// The body of the `impl` method `fn <name>` in m2_tools.rs, up to its closing
/// brace (a `}` at method indentation). Panics when the method is missing.
pub(super) fn method_body(name: &str) -> &'static str {
    let start = SOURCE
        .find(&format!("fn {name}("))
        .unwrap_or_else(|| panic!("{name} not found in m2_tools.rs"));
    let rest = &SOURCE[start..];
    // The checkout may use LF or CRLF line endings.
    let end = ["\n    }\n", "\n    }\r\n"]
        .iter()
        .filter_map(|closing| rest.find(closing))
        .min()
        .unwrap_or(rest.len());
    &rest[..end]
}
