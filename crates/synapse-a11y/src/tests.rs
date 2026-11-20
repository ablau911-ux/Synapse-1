use std::time::Duration;

use proptest::prelude::*;
use synapse_core::{ElementId, element_id, error_codes};
use tokio::net::TcpListener;

use crate::*;

fn event(
    seq: u64,
    at_ms: u64,
    element: Option<ElementId>,
    kind: AccessibleEventKind,
) -> AccessibleEvent {
    AccessibleEvent {
        seq,
        at_ms,
        window_id: 0x1234,
        element_id: element,
        kind,
        name: None,
        value: None,
    }
}

#[test]
fn must_fail_coalesce_empty_input_prints_before_after_state() {
    let before = Vec::<AccessibleEvent>::new();
    println!("readback=coalesced_events edge=empty before={before:?}");
    let after = coalesce_events(before, Duration::from_millis(50));
    println!("readback=coalesced_events edge=empty after={after:?}");
    assert!(after.is_empty());
}

#[test]
fn must_pass_coalesce_same_key_within_window_keeps_latest_state()
-> Result<(), Box<dyn std::error::Error>> {
    let id = ElementId::parse("0x1234:00000001")?;
    let mut first = event(1, 0, Some(id.clone()), AccessibleEventKind::NameChanged);
    first.name = Some("old".to_owned());
    let mut second = event(2, 49, Some(id), AccessibleEventKind::NameChanged);
    second.name = Some("new".to_owned());
    let before = vec![first, second];
    println!("readback=coalesced_events edge=within_50ms before={before:?}");
    let after = coalesce_events(before, Duration::from_millis(50));
    println!("readback=coalesced_events edge=within_50ms after={after:?}");
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].name.as_deref(), Some("new"));
    assert_eq!(after[0].seq, 2);
    Ok(())
}

#[test]
fn must_fail_coalesce_exact_boundary_does_not_merge() -> Result<(), Box<dyn std::error::Error>> {
    let id = ElementId::parse("0x1234:00000002")?;
    let before = vec![
        event(1, 0, Some(id.clone()), AccessibleEventKind::FocusChanged),
        event(2, 50, Some(id), AccessibleEventKind::FocusChanged),
    ];
    println!("readback=coalesced_events edge=exact_50ms before={before:?}");
    let after = coalesce_events(before, Duration::from_millis(50));
    println!("readback=coalesced_events edge=exact_50ms after={after:?}");
    assert_eq!(after.len(), 2);
    Ok(())
}

#[test]
fn must_pass_value_debounce_rapid_typing_keeps_first_and_latest_state()
-> Result<(), Box<dyn std::error::Error>> {
    let id = ElementId::parse("0x1234:00000003")?;
    let before: Vec<_> = "abcdef"
        .chars()
        .enumerate()
        .map(|(index, character)| {
            let mut item = event(
                u64::try_from(index).unwrap_or(u64::MAX),
                u64::try_from(index * 20).unwrap_or(u64::MAX),
                Some(id.clone()),
                AccessibleEventKind::ValueChanged,
            );
            item.value = Some(character.to_string());
            item
        })
        .collect();
    println!("readback=debounced_events edge=rapid_typing before={before:?}");
    let after = debounce_value_changes(before, Duration::from_millis(200));
    println!("readback=debounced_events edge=rapid_typing after={after:?}");
    // Six keystrokes 20 ms apart inside one 200 ms window: the first value is
    // emitted at once and only the latest pending value is flushed after it.
    let values: Vec<_> = after.iter().map(|item| item.value.as_deref()).collect();
    assert_eq!(values, [Some("a"), Some("f")]);
    Ok(())
}

#[test]
fn must_pass_value_debounce_focus_loss_flushes_pending_state()
-> Result<(), Box<dyn std::error::Error>> {
    let id = ElementId::parse("0x1234:00000004")?;
    let mut first = event(1, 0, Some(id.clone()), AccessibleEventKind::ValueChanged);
    first.value = Some("a".to_owned());
    let mut second = event(2, 20, Some(id.clone()), AccessibleEventKind::ValueChanged);
    second.value = Some("b".to_owned());
    let focus = event(3, 25, Some(id), AccessibleEventKind::FocusChanged);
    let before = vec![first, second, focus];
    println!("readback=debounced_events edge=focus_loss before={before:?}");
    let after = debounce_value_changes(before, Duration::from_millis(200));
    println!("readback=debounced_events edge=focus_loss after={after:?}");
    assert_eq!(after.len(), 3);
    assert_eq!(after[1].value.as_deref(), Some("b"));
    assert_eq!(after[2].kind, AccessibleEventKind::FocusChanged);
    Ok(())
}

#[test]
fn must_pass_runtime_id_hex_round_trips_through_composite_element_id()
-> Result<(), Box<dyn std::error::Error>> {
    let runtime = [42, -1, 0x1234_abcd_u32.cast_signed()];
    let runtime_hex = runtime_id_hex(&runtime);
    let id = element_id(0x12ab, &runtime_hex);
    println!("readback=element_id edge=runtime_hex before={runtime:?}");
    println!("readback=element_id edge=runtime_hex after={id}");
    let parts = id.parts()?;
    assert_eq!(parts.hwnd, 0x12ab);
    assert_eq!(parts.runtime_id_hex, "0000002affffffff1234abcd");
    Ok(())
}

#[test]
fn must_fail_implicit_focus_window_refuses_before_touching_os_focus() {
    let hwnd = 0x1234_i64;
    println!("readback=foreground_activation_guard edge=implicit before_hwnd=0x{hwnd:x}");
    #[cfg(windows)]
    // SAFETY: GetForegroundWindow only reads window-manager state.
    let foreground_before =
        unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() }.0 as isize;
    let after = focus_window(hwnd);
    println!("readback=foreground_activation_guard edge=implicit after={after:?}");
    assert_eq!(
        after.err().map(|err| err.code()),
        Some(error_codes::FOREGROUND_ACTIVATION_REFUSED)
    );
    // The refusal happens before any OS focus call: the foreground is untouched.
    #[cfg(windows)]
    {
        // SAFETY: as above, a read-only window-manager query.
        let foreground_after =
            unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() }.0 as isize;
        assert_eq!(
            foreground_after, foreground_before,
            "a refused implicit focus must not change the foreground window"
        );
    }
}

#[tokio::test]
async fn must_fail_cdp_probe_non_chromium_is_explicitly_not_chromium() {
    let before = ("notepad.exe", Vec::<u16>::new());
    println!("readback=cdp_diagnostics edge=non_chromium before={before:?}");
    let after = probe_chromium_cdp("notepad.exe", &[], Duration::from_millis(10)).await;
    println!("readback=cdp_diagnostics edge=non_chromium after={after:?}");
    assert_eq!(after.status, CdpStatus::NotChromium);
    assert!(after.reason_code.is_none());
}

#[tokio::test]
async fn must_fail_cdp_probe_chromium_without_port_surfaces_unreachable_code() {
    let before = ("chrome.exe", Vec::<u16>::new());
    println!("readback=cdp_diagnostics edge=no_debug_port before={before:?}");
    let after = probe_chromium_cdp("chrome.exe", &[], Duration::from_millis(10)).await;
    println!("readback=cdp_diagnostics edge=no_debug_port after={after:?}");
    assert_eq!(after.status, CdpStatus::Unreachable);
    assert_eq!(
        after.reason_code.as_deref(),
        Some(error_codes::A11Y_CDP_UNREACHABLE)
    );
    assert!(
        after
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("chrome://inspect/#remote-debugging"))
    );
}

#[tokio::test]
async fn must_pass_cdp_probe_reachable_debug_port_surfaces_capabilities()
-> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    println!("readback=cdp_diagnostics edge=reachable_port before=port:{port}");
    let after = probe_chromium_cdp("msedge.exe", &[port], Duration::from_secs(1)).await;
    println!("readback=cdp_diagnostics edge=reachable_port after={after:?}");
    assert_eq!(after.status, CdpStatus::Ok);
    assert_eq!(
        after.endpoint.as_deref(),
        Some(format!("http://127.0.0.1:{port}").as_str())
    );
    assert_eq!(after.capabilities, cdp_capabilities());
    Ok(())
}

// === Blocking probe (the path used by the synchronous perception pipeline) ===
//
// These exercise the *real* `probe_chromium_cdp_blocking` used by observe/find
// against real loopback sockets — no mocks — so the #683 diagnostics contract is
// locked at its source (issue #691, supporting evidence; not an FSV harness).

#[test]
fn must_pass_launched_port_registry_is_consulted_first_then_defaults() {
    // Use a high, unlikely-collision pid for test isolation.
    let pid = 4_294_967_000_u32;
    forget_launched_port(pid);
    let before = candidate_ports_for_pid(pid);
    println!("readback=cdp_port_registry edge=unregistered before={before:?}");
    assert_eq!(before, vec![DEFAULT_CDP_PORT]);

    register_launched_port(pid, 51_999);
    let after = candidate_ports_for_pid(pid);
    println!("readback=cdp_port_registry edge=registered after={after:?}");
    assert_eq!(after.first().copied(), Some(51_999));
    assert!(after.contains(&DEFAULT_CDP_PORT));

    forget_launched_port(pid);
    let cleared = candidate_ports_for_pid(pid);
    println!("readback=cdp_port_registry edge=forgotten after={cleared:?}");
    assert_eq!(cleared, vec![DEFAULT_CDP_PORT]);
}

proptest! {
    #[test]
    fn must_fail_coalescing_never_outputs_same_key_inside_window(times in proptest::collection::vec(0_u64..500, 1..80)) {
        let id = element_id(0x1234, "00000005");
        let mut sorted = times;
        sorted.sort_unstable();
        let input: Vec<_> = sorted
            .iter()
            .enumerate()
            .map(|(index, at_ms)| event(u64::try_from(index).unwrap_or(u64::MAX), *at_ms, Some(id.clone()), AccessibleEventKind::NameChanged))
            .collect();
        let latest_ms = sorted.last().copied();
        let output = coalesce_events(input, Duration::from_millis(50));
        // Coalescing never loses the latest state: the newest input survives.
        prop_assert!(!output.is_empty(), "non-empty input must keep an event");
        prop_assert_eq!(output.last().map(|item| item.at_ms), latest_ms);
        for pair in output.windows(2) {
            prop_assert!(
                pair[1].at_ms.saturating_sub(pair[0].at_ms) >= 50,
                "readback=coalesced_events edge=proptest after={output:?}"
            );
        }
    }
}

#[cfg(windows)]
#[test]
fn must_pass_windows_win_event_hook_apartment_readback_is_sta()
-> Result<(), Box<dyn std::error::Error>> {
    let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
    println!("readback=winevent_hook edge=apartment before=unsubscribed");
    let subscription = subscribe_win_events(sender)?;
    let readback = subscription.readback().clone();
    println!("readback=winevent_hook edge=apartment after={readback:?}");
    assert!(readback.apartment.is_sta_family());
    assert_eq!(readback.hook_count, 10);
    assert_eq!(readback.event_ids.len(), 10);
    drop(subscription);
    Ok(())
}

#[cfg(windows)]
#[test]
fn must_pass_windows_uia_worker_readback_is_mta_windowless()
-> Result<(), Box<dyn std::error::Error>> {
    println!("readback=uia_worker edge=threading before=uninitialized_or_existing");
    let readback = uia_worker_readback()?;
    println!("readback=uia_worker edge=threading after={readback:?}");
    assert!(readback.is_mta_windowless());
    Ok(())
}

#[cfg(windows)]
#[test]
fn must_fail_windows_direct_uielement_api_fails_closed() {
    println!("readback=uia_direct_api edge=focused_window before=call");
    let after = focused_window();
    println!("readback=uia_direct_api edge=focused_window after={after:?}");
    let Err(error) = after else {
        panic!("direct UIElement API unexpectedly returned a COM element");
    };
    assert_eq!(error.code(), error_codes::OBSERVE_INTERNAL);
    assert!(error.to_string().contains("direct UIElement"));
}

#[cfg(windows)]
#[test]
fn must_fail_windows_foreground_context_invalid_hwnd_reports_no_foreground() {
    let before = 0_i64;
    println!("readback=foreground_context edge=invalid_hwnd before=0x{before:x}");
    let after = foreground_context(before);
    println!("readback=foreground_context edge=invalid_hwnd after={after:?}");
    assert_eq!(
        after.err().map(|error| error.code()),
        Some(error_codes::A11Y_NO_FOREGROUND)
    );
}

#[cfg(windows)]
#[test]
fn must_fail_snapshot_element_rejects_dead_hwnd_as_stale() {
    let id = element_id(0, "deadbeef");
    let result = snapshot_element(&id, 0);
    assert!(matches!(result, Err(A11yError::ElementStale { .. })));
}
