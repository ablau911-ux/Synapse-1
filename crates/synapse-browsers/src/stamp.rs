//! Installed-worker identity shared by every browser: the #237 code stamp and
//! the service-worker SHA-256.

use sha2::{Digest, Sha256};

const STAMP_DECLARATION: &str = "const BRIDGE_CODE_STAMP = \"";

/// The `BRIDGE_CODE_STAMP` value declared in worker source, if well formed.
#[must_use]
pub fn parse_code_stamp(source: &str) -> Option<String> {
    source.lines().find_map(|line| {
        let value = line.trim().strip_prefix(STAMP_DECLARATION)?;
        let stamp = value.strip_suffix("\";")?;
        (!stamp.is_empty() && !stamp.contains('"')).then(|| stamp.to_owned())
    })
}

/// Lowercase hex SHA-256.
#[must_use]
pub fn sha256_hex_lower(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}
