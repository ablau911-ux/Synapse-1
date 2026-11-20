//! Windows registry implementation of `synapse_browsers::RegistryReader`.
//!
//! `synapse-browsers` forbids `unsafe`; this crate already owns the
//! `RegGetValueW` FFI, so the row-driven reads (App Paths, policy roots) come
//! through here. Moved from the chrome bridge's HKCU-only reader so every hive
//! and every browser row shares one implementation.

use synapse_browsers::{RegistryHive, RegistryReader};

/// Live registry reader.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct WindowsRegistryReader;

impl RegistryReader for WindowsRegistryReader {
    fn read_string(&self, hive: RegistryHive, subkey: &str, value_name: &str) -> Option<String> {
        read_registry_string(hive, subkey, value_name)
    }
}

/// Reads one `REG_SZ`/`REG_EXPAND_SZ` value (unexpanded); the empty value
/// name reads the key's default value.
#[cfg(windows)]
pub(crate) fn read_registry_string(
    hive: RegistryHive,
    subkey: &str,
    value_name: &str,
) -> Option<String> {
    use windows::{
        Win32::{
            Foundation::ERROR_SUCCESS,
            System::Registry::{
                HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, REG_VALUE_TYPE, RRF_RT_REG_EXPAND_SZ,
                RRF_RT_REG_SZ, RegGetValueW,
            },
        },
        core::PCWSTR,
    };

    let root = match hive {
        RegistryHive::CurrentUser => HKEY_CURRENT_USER,
        RegistryHive::LocalMachine => HKEY_LOCAL_MACHINE,
    };
    let subkey_wide = wide_null(subkey);
    let value_wide = wide_null(value_name);
    let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ;
    let mut value_type = REG_VALUE_TYPE::default();
    let mut byte_len = 0_u32;
    // SAFETY: `subkey_wide` and `value_wide` are live NUL-terminated UTF-16 buffers (an empty
    // value name selects the default value); `value_type` and `byte_len` are live locals; the null
    // data pointer only asks for the size.
    let status = unsafe {
        RegGetValueW(
            root,
            PCWSTR(subkey_wide.as_ptr()),
            PCWSTR(value_wide.as_ptr()),
            flags,
            Some(&raw mut value_type),
            None,
            Some(&raw mut byte_len),
        )
    };
    if status != ERROR_SUCCESS || byte_len == 0 {
        return None;
    }

    let mut buffer = vec![0_u16; (byte_len as usize).div_ceil(2)];
    // SAFETY: same live pointers as the size query; `buffer` holds `byte_len.div_ceil(2)` u16s,
    // enough for the byte count the size query returned, and `byte_len` reports that capacity.
    let status = unsafe {
        RegGetValueW(
            root,
            PCWSTR(subkey_wide.as_ptr()),
            PCWSTR(value_wide.as_ptr()),
            flags,
            Some(&raw mut value_type),
            Some(buffer.as_mut_ptr().cast()),
            Some(&raw mut byte_len),
        )
    };
    if status != ERROR_SUCCESS {
        tracing::warn!(
            code = "BROWSER_BRIDGE_REGISTRY_READ_FAILED",
            hive = hive.label(),
            subkey,
            value_name,
            status = status.0,
            "registry value read failed after size query"
        );
        return None;
    }

    let units = (byte_len as usize).div_ceil(2).min(buffer.len());
    buffer.truncate(units);
    let nul = buffer
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(buffer.len());
    Some(String::from_utf16_lossy(&buffer[..nul]))
}

#[cfg(not(windows))]
pub(crate) fn read_registry_string(
    _hive: RegistryHive,
    _subkey: &str,
    _value_name: &str,
) -> Option<String> {
    None
}

#[cfg(windows)]
fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
