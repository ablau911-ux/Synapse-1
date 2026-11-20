use super::*;
use windows::Win32::UI::WindowsAndMessaging::ENDSESSION_CRITICAL;

#[test]
fn must_fail_duplicate_requests_share_one_completion_and_deadline() -> anyhow::Result<()> {
    let latch = ExitLatch::default();
    let (sender, receiver) = mpsc::channel();
    let barrier = std::sync::Barrier::new(3);
    std::thread::scope(|scope| -> anyhow::Result<()> {
        let first = scope.spawn(|| {
            barrier.wait();
            latch
                .request(&sender, "os_logoff", Duration::from_secs(2))
                .wait()
        });
        let second = scope.spawn(|| {
            barrier.wait();
            latch
                .request(&sender, "os_shutdown", Duration::from_secs(2))
                .wait()
        });
        barrier.wait();
        receiver.recv_timeout(Duration::from_secs(1))?;
        assert!(receiver.try_recv().is_err());
        let attempt = latch.request(&sender, "os_logoff", Duration::ZERO);
        assert!(!attempt.completed.load(Ordering::Acquire));
        attempt.completed.store(true, Ordering::Release);
        assert!(
            first
                .join()
                .map_err(|_| anyhow::anyhow!("first waiter panicked"))?
        );
        assert!(
            second
                .join()
                .map_err(|_| anyhow::anyhow!("second waiter panicked"))?
        );
        assert!(attempt.wait());
        assert!(receiver.try_recv().is_err());
        Ok(())
    })
}

#[test]
fn must_fail_duplicate_request_cannot_restart_expired_budget() {
    let latch = ExitLatch::default();
    let (sender, receiver) = mpsc::channel();
    let first = latch.request(&sender, "os_logoff", Duration::ZERO);
    assert!(!first.wait());
    let later = latch.request(&sender, "os_shutdown", Duration::from_secs(10));
    assert!(std::ptr::eq(first, later));
    assert!(!later.wait());
    assert_eq!(receiver.try_recv().ok(), Some("os_logoff"));
    assert!(receiver.try_recv().is_err());
}

#[test]
fn must_pass_console_events_map_without_intercepting_ctrl_c_or_break() {
    assert_eq!(console_reason(CTRL_SHUTDOWN_EVENT), Some("os_shutdown"));
    assert_eq!(console_reason(CTRL_CLOSE_EVENT), Some("console_close"));
    assert_eq!(console_reason(CTRL_LOGOFF_EVENT), Some("os_logoff"));
    for event in [0, 1, 3, u32::MAX] {
        assert_eq!(console_reason(event), None);
    }
}

#[test]
fn must_fail_session_end_requires_confirmation_and_treats_logoff_as_a_bitmask() {
    assert_eq!(session_reason(true, 0), Some("os_shutdown"));
    assert_eq!(
        session_reason(true, ENDSESSION_CLOSEAPP),
        Some("restart_manager")
    );
    assert_eq!(
        session_reason(true, ENDSESSION_CRITICAL),
        Some("os_shutdown")
    );
    assert_eq!(session_reason(true, ENDSESSION_LOGOFF), Some("os_logoff"));
    assert_eq!(
        session_reason(true, ENDSESSION_LOGOFF | ENDSESSION_CLOSEAPP),
        Some("os_logoff")
    );
    assert_eq!(session_reason(false, 0), None);
    assert_eq!(session_reason(false, ENDSESSION_LOGOFF), None);
}
