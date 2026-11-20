//! #73: a seat startup that hit its deadline is reported with its own typed MCP code.
//! Only a confirmed teardown readback may use the "torn down" code; a pending teardown
//! gets its own code and keeps the physical path in the message.

use super::map_seat_error;
use synapse_seats::{SeatError, SeatId, TeardownStatus};

fn code_and_message(error: SeatError) -> (Option<String>, String) {
    let mapped = map_seat_error("seat_create", error);
    let code = mapped.data.and_then(|data| {
        data.get("code")
            .and_then(|code| code.as_str())
            .map(ToOwned::to_owned)
    });
    (code, mapped.message.into_owned())
}

#[test]
fn must_pass_confirmed_startup_timeout_maps_to_seat_startup_timeout_73() {
    let (code, _) = code_and_message(SeatError::StartupTimeout {
        timeout_ms: 25_000,
        teardown: TeardownStatus::Confirmed,
    });
    assert_eq!(code.as_deref(), Some("SEAT_STARTUP_TIMEOUT"));
}

#[test]
fn must_fail_pending_teardown_is_not_reported_as_torn_down_73() {
    let path = r"C:\runtime\browser-seat-73";
    let (code, message) = code_and_message(SeatError::StartupTimeout {
        timeout_ms: 25_000,
        teardown: TeardownStatus::Pending(format!("private profile {path} present=true")),
    });
    assert_eq!(code.as_deref(), Some("SEAT_STARTUP_CLEANUP_PENDING"));
    assert!(
        message.contains(path),
        "the physical path must reach the caller: {message}"
    );
}

#[test]
fn must_fail_request_timeout_is_not_reported_as_startup_timeout_73() {
    let (code, _) = code_and_message(SeatError::Timeout(SeatId::new()));
    assert_eq!(code.as_deref(), Some("SEAT_TIMEOUT"));
}
