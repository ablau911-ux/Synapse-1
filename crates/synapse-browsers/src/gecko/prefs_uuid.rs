//! Per-profile extension UUID from `prefs.js`.
//!
//! Firefox maps each extension id to a random per-profile UUID in the pref
//! `extensions.webextensions.uuids` (JSON object, `Extension.sys.mjs`
//! `UUIDMap`); the extension origin is `moz-extension://<uuid>`. `prefs.js`
//! stores it as `user_pref("extensions.webextensions.uuids", "<escaped JSON>");`.

use serde_json::Value;

/// Firefox engine layout file name and pref name.
pub const PREFS_JS_FILE: &str = "prefs.js";
pub const UUID_MAP_PREF: &str = "extensions.webextensions.uuids";

const USER_PREF_CALL: &str = "user_pref(";

/// The validated UUID Firefox assigned to `extension_id`, if any.
#[must_use]
pub fn extension_uuid(prefs_js: &str, extension_id: &str) -> Option<String> {
    let map_text = prefs_js.lines().find_map(uuid_map_literal)?;
    let map: Value = serde_json::from_str(&map_text).ok()?;
    let uuid = map.get(extension_id)?.as_str()?;
    is_uuid(uuid).then(|| uuid.to_owned())
}

/// The decoded string value of the `UUID_MAP_PREF` line, if this is that line.
fn uuid_map_literal(line: &str) -> Option<String> {
    let args = line
        .trim()
        .strip_prefix(USER_PREF_CALL)?
        .strip_suffix(");")?
        .trim();
    let name_literal = serde_json::to_string(UUID_MAP_PREF).ok()?;
    let value_literal = args.strip_prefix(name_literal.as_str())?.trim_start();
    let value_literal = value_literal.strip_prefix(',')?.trim();
    serde_json::from_str::<String>(value_literal).ok()
}

/// Lowercase canonical `8-4-4-4-12` hex UUID (the form Firefox generates).
fn is_uuid(raw: &str) -> bool {
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
    let parts: Vec<&str> = raw.split('-').collect();
    parts.len() == GROUPS.len()
        && parts.iter().zip(GROUPS).all(|(part, len)| {
            part.len() == len
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
}
