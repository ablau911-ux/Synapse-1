use std::sync::LazyLock;

use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

pub(crate) const HEADER: &str = "X-Synapse-Local-Agent-Capability";
pub(crate) const ENV: &str = "SYNAPSE_LOCAL_AGENT_CAPABILITY";

static TOKEN: LazyLock<String> = LazyLock::new(|| {
    std::env::var(ENV)
        .ok()
        .filter(|value| value.len() >= 32)
        .unwrap_or_else(|| {
            format!(
                "{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            )
        })
});

pub(crate) fn token() -> &'static str {
    &TOKEN
}

pub(crate) fn matches(value: &str) -> bool {
    let supplied: [u8; 32] = Sha256::digest(value.as_bytes()).into();
    let expected: [u8; 32] = Sha256::digest(token().as_bytes()).into();
    bool::from(supplied.ct_eq(&expected))
}
