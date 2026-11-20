//! Native UIA ids bound to the item observed, #2362.
use std::fmt::Write as _;

use sha2::{Digest, Sha256};
#[derive(Clone, Debug)]
pub(crate) struct NativeRuntimeIdentity {
    hwnd: i64,
    runtime_hex: String,
    process_id: i32,
    role: String,
    class_name: String,
    automation_id: String,
    name: String,
}

impl NativeRuntimeIdentity {
    pub(crate) fn new(
        hwnd: i64,
        runtime_hex: &str,
        process_id: i32,
        role: &str,
        class_name: &str,
        automation_id: &str,
        name: &str,
    ) -> Result<Self, &'static str> {
        if runtime_hex.is_empty() || !runtime_hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("runtime id must be non-empty hex");
        }
        if process_id <= 0 {
            return Err("process id must be positive");
        }
        if role.is_empty() {
            return Err("role must be non-empty");
        }
        Ok(Self {
            hwnd,
            runtime_hex: runtime_hex.to_ascii_lowercase(),
            process_id,
            role: role.to_owned(),
            class_name: class_name.to_owned(),
            automation_id: automation_id.to_owned(),
            name: name.to_owned(),
        })
    }

    #[must_use]
    pub(crate) fn runtime_id_hex(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"synapse.native-runtime-identity.v1\0");
        hasher.update(self.hwnd.to_le_bytes());
        hasher.update(self.process_id.to_le_bytes());
        for value in [
            self.runtime_hex.as_str(),
            self.role.as_str(),
            self.class_name.as_str(),
            self.automation_id.as_str(),
            self.name.as_str(),
        ] {
            hasher.update((value.len() as u64).to_le_bytes());
            hasher.update(value.as_bytes());
        }
        let mut token = String::from("eded0001");
        for byte in hasher.finalize() {
            // Writing into a String cannot fail.
            let _ = write!(&mut token, "{byte:02x}");
        }
        token
    }
}

#[cfg(test)]
#[path = "native_runtime_identity_table_2362.rs"]
mod native_runtime_identity_table_2362;
