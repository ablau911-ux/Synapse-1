// FROZEN: manager owns this file. No sockets, OS calls, network, or clock.
use super::SocketClosePolicy;

struct Case {
    name: &'static str,
    shutdown: bool,
    capacity: bool,
    expected_abortive: bool,
}

fn check(cases: &[Case]) {
    for case in cases {
        let policy = if case.capacity {
            SocketClosePolicy::capacity_rejected()
        } else {
            SocketClosePolicy::for_drop(case.shutdown)
        };
        assert_eq!(
            policy.is_abortive(),
            case.expected_abortive,
            "{}",
            case.name
        );
    }
}

#[test]
fn p1_normal_drop_is_graceful() {
    check(&[
        Case {
            name: "P1: normal drop",
            shutdown: false,
            capacity: false,
            expected_abortive: false,
        },
        Case {
            name: "P1 CONTROL: capacity is different",
            shutdown: false,
            capacity: true,
            expected_abortive: true,
        },
    ]);
}

#[test]
fn p2_restart_drop_is_abortive() {
    check(&[
        Case {
            name: "P2: restart drop",
            shutdown: true,
            capacity: false,
            expected_abortive: true,
        },
        Case {
            name: "P2 CONTROL: no restart",
            shutdown: false,
            capacity: false,
            expected_abortive: false,
        },
    ]);
}

#[test]
fn p3_capacity_rejection_is_abortive() {
    check(&[
        Case {
            name: "P3: capacity rejected",
            shutdown: false,
            capacity: true,
            expected_abortive: true,
        },
        Case {
            name: "P3 CONTROL: admitted",
            shutdown: false,
            capacity: false,
            expected_abortive: false,
        },
    ]);
}

#[test]
fn p4_other_policy_instances_stay_independent() {
    let other = SocketClosePolicy::for_drop(false);
    let restarting = SocketClosePolicy::for_drop(true);
    let rejected = SocketClosePolicy::capacity_rejected();
    let later = SocketClosePolicy::for_drop(false);
    assert_eq!(
        [
            other.is_abortive(),
            restarting.is_abortive(),
            rejected.is_abortive(),
            later.is_abortive()
        ],
        [false, true, true, false],
        "P4: one forced policy must not poison another connection's policy"
    );
}
