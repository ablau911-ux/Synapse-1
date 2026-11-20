//! #2409: the OS readers behind `ports_serving_window`, on real sockets.
//! Who listens on a port (byte order, address filter) and the parent chain
//! with creation times are read from Windows, not from a mock.
#![allow(clippy::expect_used)]

use super::{listening_port_owner, ports_serving_window};
use std::io::{BufRead, BufReader};
use std::net::{Ipv4Addr, TcpListener};
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A child process of this test that listens on 127.0.0.1; killed on drop.
struct ListeningChild {
    child: Child,
    port: u16,
}

impl ListeningChild {
    fn start() -> Self {
        let mut child = Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "$l = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0); \
                 $l.Start(); [Console]::Out.WriteLine($l.LocalEndpoint.Port); \
                 [Console]::Out.Flush(); Start-Sleep -Seconds 60",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .expect("start a listening child");
        let mut line = String::new();
        BufReader::new(child.stdout.take().expect("child stdout"))
            .read_line(&mut line)
            .expect("read the child's port");
        let port = line.trim().parse().expect("the child prints its port");
        Self { child, port }
    }
}

impl Drop for ListeningChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn listener_on(address: Ipv4Addr) -> (TcpListener, u16) {
    let listener = TcpListener::bind((address, 0)).expect("bind a test listener");
    let port = listener.local_addr().expect("listener address").port();
    (listener, port)
}

#[test]
fn a_loopback_listener_of_this_process_is_owned_by_it_and_serves_it() {
    let (_listener, port) = listener_on(Ipv4Addr::LOCALHOST);
    assert_eq!(listening_port_owner(port), Some(std::process::id()));
    assert_eq!(
        ports_serving_window(std::process::id(), &[port]),
        vec![port]
    );
}

#[test]
fn an_any_address_listener_counts_and_a_closed_port_has_no_owner() {
    let (listener, port) = listener_on(Ipv4Addr::UNSPECIFIED);
    assert_eq!(listening_port_owner(port), Some(std::process::id()));
    drop(listener);
    assert_eq!(listening_port_owner(port), None);
    assert!(ports_serving_window(std::process::id(), &[port]).is_empty());
}

#[test]
fn a_childs_port_serves_this_process_but_this_process_port_does_not_serve_the_child() {
    let child = ListeningChild::start();
    let (_listener, own_port) = listener_on(Ipv4Addr::LOCALHOST);
    assert_eq!(listening_port_owner(child.port), Some(child.child.id()));
    // P2 on the OS: the listener is a child of the window's process.
    assert_eq!(
        ports_serving_window(std::process::id(), &[child.port, own_port]),
        vec![child.port, own_port]
    );
    // P4 on the OS: the listener is the PARENT of the window's process.
    assert!(ports_serving_window(child.child.id(), &[own_port]).is_empty());
}
