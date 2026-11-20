#![forbid(unsafe_code)]
#![allow(
    clippy::collapsible_if,
    clippy::infallible_destructuring_match,
    clippy::items_after_test_module
)]
pub mod browser;
mod profile_guard;
pub mod runtime;
pub mod types;

pub use runtime::{SeatBackend, SeatBackendFactory, SeatSupervisor};
pub use types::*;
