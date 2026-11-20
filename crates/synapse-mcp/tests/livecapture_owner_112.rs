//! #112: a session that does not own the live capture must not be able to stop
//! or retarget it; the refusal names the owner. The owner, an unscoped caller,
//! and a caller facing a terminated owner still can.
//
// SEAM NEEDED: these drive a private copy of src/m1/capture_owner.rs, not the
// real `livecapture` stop/retarget gate. Driving that gate over two HTTP
// sessions needs a running WGC capture of a native window; the daemon has no
// synthetic capture source, so it cannot run headless or on a locked screen.
#[allow(dead_code)]
#[path = "../src/m1/capture_owner.rs"]
mod capture_owner;

use capture_owner::CaptureOwnership;

const A: &str = "session-a";
const B: &str = "session-b";
const CAP: &str = "capture-1";

fn owned_by_a() -> CaptureOwnership {
    let mut ownership = CaptureOwnership::default();
    ownership.claim(CAP, Some(A));
    ownership
}

#[test]
fn must_fail_other_session_is_refused_with_owner_named() {
    let conflict = owned_by_a()
        .check_mutation(Some(CAP), Some(B), |_| true)
        .expect_err("non-owner must be refused");
    assert_eq!(conflict.owner_session_id, A);
    assert_eq!(conflict.capture_id, CAP);
    let message = conflict.message("stop");
    assert!(message.contains(A), "{message}");
    assert!(message.contains(CAP), "{message}");
}

#[test]
fn must_pass_owner_may_mutate() {
    assert!(
        owned_by_a()
            .check_mutation(Some(CAP), Some(A), |_| true)
            .is_ok()
    );
}

#[test]
fn must_pass_terminated_owner_does_not_pin_the_capture() {
    assert!(
        owned_by_a()
            .check_mutation(Some(CAP), Some(B), |session| session != A)
            .is_ok()
    );
}

#[test]
fn must_pass_unscoped_caller_is_not_restricted() {
    assert!(
        owned_by_a()
            .check_mutation(Some(CAP), None, |_| true)
            .is_ok()
    );
}

#[test]
fn must_pass_no_active_capture_or_replaced_capture_has_no_owner() {
    let ownership = owned_by_a();
    assert!(ownership.check_mutation(None, Some(B), |_| true).is_ok());
    assert!(
        ownership
            .check_mutation(Some("capture-2"), Some(B), |_| true)
            .is_ok(),
        "ownership record of a replaced capture must not apply"
    );
    assert!(ownership.owner_of(Some("capture-2")).is_none());
}

#[test]
fn must_pass_unscoped_claim_and_clear_leave_capture_unowned() {
    let mut ownership = owned_by_a();
    ownership.claim(CAP, None);
    assert!(ownership.owner_of(Some(CAP)).is_none());
    ownership.claim(CAP, Some(A));
    assert_eq!(
        ownership.owner_of(Some(CAP)).map(|o| o.session_id.as_str()),
        Some(A)
    );
    ownership.clear();
    assert!(ownership.owner_of(Some(CAP)).is_none());
    assert!(
        ownership
            .check_mutation(Some(CAP), Some(B), |_| true)
            .is_ok()
    );
}
