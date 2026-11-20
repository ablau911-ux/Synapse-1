//! Windows registry read seam. This crate forbids `unsafe`; the daemon crate,
//! which already owns the `RegGetValueW` FFI, implements [`RegistryReader`].

/// Registry hive a row-driven read targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegistryHive {
    CurrentUser,
    LocalMachine,
}

impl RegistryHive {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::CurrentUser => "HKCU",
            Self::LocalMachine => "HKLM",
        }
    }
}

/// Reads one `REG_SZ`/`REG_EXPAND_SZ` value; the empty value name is the
/// key's default value.
pub trait RegistryReader: Send + Sync {
    fn read_string(&self, hive: RegistryHive, subkey: &str, value_name: &str) -> Option<String>;
}

/// Reader for hosts without a registry (tests, non-Windows builds).
#[derive(Clone, Copy, Debug, Default)]
pub struct NoRegistry;

impl RegistryReader for NoRegistry {
    fn read_string(&self, _hive: RegistryHive, _subkey: &str, _value_name: &str) -> Option<String> {
        None
    }
}
