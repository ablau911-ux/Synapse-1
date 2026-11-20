//! Declared multi-browser bridge vocabulary.
//!
//! Every browser Synapse can bridge is one row in `browsers/<id>.json`
//! (Iron Law 9: names, paths, registry keys and wire ids live in data, not in
//! scattered literals). This crate loads those rows once ([`BrowserRegistry`])
//! and owns the shared types the daemon, seats, the raw-CDP lane and the
//! installer agree on: the kind id, tab-target ids, bridge endpoints,
//! executable discovery and per-family profile inspection.
//!
//! The crate is `unsafe`-free: Windows registry reads go through the
//! [`RegistryReader`] seam, implemented by the daemon crate that already owns
//! the registry FFI.

pub mod chromium;
mod discovery;
mod endpoint;
mod error;
pub mod gecko;
mod inspector;
mod kind;
mod registry;
mod registry_win;
mod row;
mod stamp;
mod target_id;

pub use discovery::{
    BrowserExecutableLocator, LocatedExecutable, expand_env_vars, expand_process_env_vars,
    process_env_var,
};
pub use endpoint::{BridgeEndpoint, BridgeLane};
pub use error::{BrowserBridgeError, BrowserRegistryError};
pub use inspector::{BridgeInstallState, BrowserProfileInspector, OriginRule, inspector_for};
pub use kind::{BrowserFamily, BrowserKindId, LEGACY_DEFAULT_KIND};
pub use registry::BrowserRegistry;
pub use registry_win::{NoRegistry, RegistryHive, RegistryReader};
pub use row::{BrowserRow, ExtensionSpec, InstallUiSpec, PolicySpec, ProfileLayout};
pub use stamp::{parse_code_stamp, sha256_hex_lower};
pub use target_id::{BridgeElementTarget, BridgeTabTarget};
