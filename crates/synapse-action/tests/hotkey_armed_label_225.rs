//! #225: with a VALID configured hotkey, the label reported and the real
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! "operator panic hotkey armed" log line name the armed chord, never the
//! default ctrl+alt+shift+p.
//!
//! Own test binary on purpose: the configured label is a one-shot process
//! global, so it must be armed exactly once with the override below. The hook
//! only observes keys (it sends none); it is armed once per process and
//! removed by the OS when the test process exits.

#![cfg(windows)]

use std::io::{self, Write};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use synapse_action::hotkey::{
    DEFAULT_OPERATOR_HOTKEY, OPERATOR_HOTKEY_COMPAT_ENV, OPERATOR_HOTKEY_ENV,
    configured_operator_hotkey_label,
};
use synapse_action::{ActionError, OperatorHotkeyGuard, install_operator_hotkey};
use tracing_subscriber::fmt::MakeWriter;

const ARMED: &str = "ctrl+alt+shift+9";
const ARMED_LOG: &str = "operator panic hotkey armed";

#[derive(Clone, Default)]
struct TraceBuffer(Arc<Mutex<Vec<u8>>>);

impl TraceBuffer {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().expect("trace buffer")).into_owned()
    }
}

impl<'a> MakeWriter<'a> for TraceBuffer {
    type Writer = TraceBuffer;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

impl Write for TraceBuffer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().expect("trace buffer").extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Serializes env mutation within this binary.
static ENV: Mutex<()> = Mutex::new(());

struct Armed {
    trace: TraceBuffer,
    _guard: OperatorHotkeyGuard,
}

/// Installs the hotkey once, with `ctrl+alt+shift+9` configured, capturing
/// every log line of the process (the armed line is logged on the hook thread).
fn armed() -> &'static Armed {
    static ARMED_ONCE: OnceLock<Armed> = OnceLock::new();
    ARMED_ONCE.get_or_init(|| {
        let trace = TraceBuffer::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(trace.clone())
            .with_ansi(false)
            .without_time()
            .finish();
        tracing::subscriber::set_global_default(subscriber).expect("global subscriber");
        let _env = ENV.lock().expect("env lock");
        // SAFETY: env writes are serialized by ENV within this test binary and
        // happen before the hotkey threads read them.
        unsafe {
            std::env::remove_var(OPERATOR_HOTKEY_COMPAT_ENV);
            std::env::set_var(OPERATOR_HOTKEY_ENV, ARMED);
        }
        assert_eq!(
            configured_operator_hotkey_label(),
            DEFAULT_OPERATOR_HOTKEY,
            "precondition: nothing armed yet in this process"
        );
        let guard = install_operator_hotkey(|| {})
            .expect("arm ctrl+alt+shift+9 (needs an interactive desktop session)");
        Armed {
            trace,
            _guard: guard,
        }
    })
}

fn armed_log_line(trace: &TraceBuffer) -> Option<String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let line = trace
            .text()
            .lines()
            .find(|line| line.contains(ARMED_LOG))
            .map(str::to_owned);
        if line.is_some() || Instant::now() >= deadline {
            return line;
        }
        std::thread::yield_now();
    }
}

#[test]
fn must_pass_armed_hotkey_is_the_reported_and_logged_label() {
    let armed = armed();

    assert_eq!(configured_operator_hotkey_label(), ARMED);
    let line = armed_log_line(&armed.trace).expect("the armed log line is written");
    assert!(
        line.contains(ARMED),
        "armed log names the armed chord: {line}"
    );
    assert!(
        !line.contains(DEFAULT_OPERATOR_HOTKEY),
        "armed log must not name the default: {line}"
    );
}

#[test]
fn must_fail_second_install_is_refused_and_keeps_the_armed_label() {
    let _armed = armed();
    let _env = ENV.lock().expect("env lock");
    let saved = std::env::var_os(OPERATOR_HOTKEY_ENV);
    // SAFETY: serialized by ENV; restored below.
    unsafe { std::env::set_var(OPERATOR_HOTKEY_ENV, "ctrl+alt+shift+7") };

    let result = install_operator_hotkey(|| {});

    // SAFETY: as above.
    unsafe {
        match saved {
            Some(value) => std::env::set_var(OPERATOR_HOTKEY_ENV, value),
            None => std::env::remove_var(OPERATOR_HOTKEY_ENV),
        }
    }
    let Err(ActionError::BackendUnavailable { detail }) = result else {
        panic!("a second install must be refused with BackendUnavailable");
    };
    assert_eq!(
        detail,
        "operator hotkey is already installed in this process"
    );
    assert_eq!(configured_operator_hotkey_label(), ARMED);
}
