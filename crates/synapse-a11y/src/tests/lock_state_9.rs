//! #9: lock-surface process classification is exact and case-insensitive.

use super::is_lock_surface_process;

#[test]
fn must_pass_lock_screen_and_logon_ui_are_lock_surfaces() {
    assert!(is_lock_surface_process(
        r"C:\Windows\SystemApps\Microsoft.LockApp_cw5n1h2txyewy\LockApp.exe"
    ));
    assert!(is_lock_surface_process("LockApp.exe"));
    assert!(is_lock_surface_process(r"c:\windows\system32\LOGONUI.EXE"));
}

#[test]
fn must_fail_ordinary_and_lookalike_processes_are_not() {
    assert!(!is_lock_surface_process(r"C:\Windows\notepad.exe"));
    assert!(!is_lock_surface_process("NotLockApp.exe"));
    assert!(!is_lock_surface_process(r"C:\LockApp.exe.d\app.exe"));
    assert!(!is_lock_surface_process(""));
}
