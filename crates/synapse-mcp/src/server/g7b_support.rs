//! Extends the read-only authored fixture with the shared real rmcp context.
#[path = "tests/g7b_support_57.rs"]
mod fixture;
pub(crate) use crate::test_support::request_context_for_session;
pub(crate) use fixture::*;
