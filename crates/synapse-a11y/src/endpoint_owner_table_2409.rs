//! FROZEN behaviour table of Gitea #2409, rows P1-P9 (`EndpointOwner`).
//! Written from the issue table before the code. Coders never edit this
//! file; a wrong row is reported to the manager.
//! Place: crates/synapse-a11y/src/endpoint_owner_table_2409.rs (registered
//! by the `#[path]` line at the top of endpoint_owner.rs). One test per row,
//! named after the row. A chain is the port owner first, then its parent,
//! grandparent, ...; `created` is a creation time (smaller = older).
//! CONTROL cases stop a stub that answers one constant (every port serves,
//! or none does) from passing a row.

use super::{EndpointOwner, ProcessStep};

const PORT: u16 = 9222;
/// The window's own process (a browser, an Electron app or a WebView2 host).
const WINDOW: u32 = 4100;
/// The window's parent (explorer), and its parent (System).
const EXPLORER: u32 = 900;
const SYSTEM: u32 = 4;
/// A child and a grandchild of the window's process (WebView2 browser...).
const CHILD: u32 = 5200;
const GRANDCHILD: u32 = 5300;
/// Another browser, started by explorer, unrelated to the window.
const OTHER_BROWSER: u32 = 7300;

const fn step(pid: u32, parent_pid: u32, created: u64) -> ProcessStep {
    ProcessStep::new(pid, parent_pid, created)
}

const fn system() -> ProcessStep {
    step(SYSTEM, 0, 10)
}

const fn explorer() -> ProcessStep {
    step(EXPLORER, SYSTEM, 100)
}

/// The window's process, created at 1000 by explorer.
const fn window() -> ProcessStep {
    step(WINDOW, EXPLORER, 1_000)
}

const fn child() -> ProcessStep {
    step(CHILD, WINDOW, 2_000)
}

const fn grandchild() -> ProcessStep {
    step(GRANDCHILD, CHILD, 3_000)
}

fn owner_on(port: u16, port_owner: Option<u32>, chain: Vec<ProcessStep>) -> EndpointOwner {
    EndpointOwner::new(port, WINDOW, port_owner, chain)
}

fn serves(port_owner: Option<u32>, chain: Vec<ProcessStep>) -> bool {
    owner_on(PORT, port_owner, chain).serves_window()
}

/// The window's own process listens (Chrome, Edge, Electron).
fn own_port(port: u16) -> EndpointOwner {
    owner_on(port, Some(WINDOW), vec![window(), explorer(), system()])
}

/// Another browser listens.
fn foreign_port(port: u16) -> EndpointOwner {
    owner_on(
        port,
        Some(OTHER_BROWSER),
        vec![step(OTHER_BROWSER, EXPLORER, 1_500), explorer(), system()],
    )
}

/// `before_window` unrelated steps, each the parent of the one before, the
/// last one a child of the window, then the window's own step: the window is
/// at index `before_window` of the chain. Creation order is kept everywhere.
fn long_chain(before_window: u32) -> Vec<ProcessStep> {
    let mut chain: Vec<ProcessStep> = (0..before_window)
        .map(|index| {
            let pid = 20_000 + index;
            let parent_pid = if index + 1 == before_window {
                WINDOW
            } else {
                pid + 1
            };
            step(pid, parent_pid, 1_000_000 - u64::from(index) * 10)
        })
        .collect();
    chain.push(window());
    chain
}

#[test]
fn p1_port_owned_by_the_windows_own_process_serves() {
    assert!(
        serves(Some(WINDOW), vec![window(), explorer(), system()]),
        "the window's own process listens"
    );
    // CONTROL: the same window with another browser listening does not serve.
    assert!(
        !serves(
            Some(OTHER_BROWSER),
            vec![step(OTHER_BROWSER, EXPLORER, 1_500), explorer(), system()]
        ),
        "control: another browser listens"
    );
}

#[test]
fn p2_port_owned_by_a_child_or_grandchild_of_the_window_serves() {
    assert!(
        serves(Some(CHILD), vec![child(), window(), explorer()]),
        "a child of the window's process listens (WebView2 browser)"
    );
    assert!(
        serves(
            Some(GRANDCHILD),
            vec![grandchild(), child(), window(), explorer()]
        ),
        "a grandchild of the window's process listens"
    );
    // CONTROL: a sibling (another child of explorer) is not a descendant.
    assert!(
        !serves(
            Some(5400),
            vec![step(5400, EXPLORER, 2_000), explorer(), system()]
        ),
        "control: a sibling of the window's process listens"
    );
}

#[test]
fn p3_port_owned_by_an_unrelated_browser_does_not_serve() {
    assert!(
        !foreign_port(PORT).serves_window(),
        "another browser listens"
    );
    // CONTROL: the window's own process on the same port serves.
    assert!(
        own_port(PORT).serves_window(),
        "control: own process listens"
    );
}

#[test]
fn p4_port_owned_by_the_parent_of_the_windows_process_does_not_serve() {
    assert!(
        !serves(Some(EXPLORER), vec![explorer(), system()]),
        "the window's parent listens"
    );
    // CONTROL: the window's own process serves.
    assert!(
        own_port(PORT).serves_window(),
        "control: own process listens"
    );
}

#[test]
fn p5_unreadable_owner_or_broken_chain_does_not_serve() {
    assert!(!serves(None, Vec::new()), "the port owner cannot be read");
    assert!(
        !serves(None, vec![window(), explorer()]),
        "the port owner cannot be read, whatever chain is given"
    );
    assert!(
        !serves(Some(GRANDCHILD), vec![grandchild()]),
        "the chain breaks after the owner (its parent cannot be read)"
    );
    assert!(
        !serves(Some(CHILD), vec![child()]),
        "the chain breaks before the window's own step is read"
    );
    assert!(
        !serves(Some(GRANDCHILD), vec![grandchild(), window(), explorer()]),
        "a step is missing: the grandchild's parent is not the next step"
    );
    assert!(
        !serves(Some(OTHER_BROWSER), vec![window(), explorer()]),
        "the chain does not start at the port owner"
    );
    assert!(
        !EndpointOwner::new(PORT, 0, Some(0), Vec::new()).serves_window(),
        "process number 0 (no process) never serves"
    );
    // CONTROL: the whole chain read serves.
    assert!(
        serves(
            Some(GRANDCHILD),
            vec![grandchild(), child(), window(), explorer()]
        ),
        "control: the full chain is read"
    );
}

#[test]
fn p6_a_reused_process_number_in_the_chain_does_not_serve() {
    // The step holding the window's number was created AFTER its child: the
    // child's real parent died and the number was given to the window later.
    assert!(
        !serves(
            Some(CHILD),
            vec![child(), step(WINDOW, EXPLORER, 2_500), explorer()]
        ),
        "the window's number, created after its child"
    );
    // Same for a step in the middle of the chain.
    assert!(
        !serves(
            Some(GRANDCHILD),
            vec![
                step(GRANDCHILD, CHILD, 3_000),
                step(CHILD, WINDOW, 3_500),
                window(),
            ]
        ),
        "the child's number, created after the grandchild"
    );
    // CONTROL: the right order serves, and the same creation time (one
    // clock tick) counts as "not later".
    assert!(
        serves(Some(CHILD), vec![child(), window(), explorer()]),
        "control: the window created before its child"
    );
    assert!(
        serves(
            Some(CHILD),
            vec![step(CHILD, WINDOW, 1_000), window(), explorer()]
        ),
        "control: the child created in the same tick as the window"
    );
}

#[test]
fn p7_a_chain_too_long_or_with_a_cycle_does_not_serve() {
    // The window is the 65th step (index 64): beyond the limit.
    let too_long = long_chain(64);
    assert_eq!(too_long.len(), 65);
    assert!(
        !serves(Some(20_000), too_long),
        "the window's process is found only after 64 steps"
    );
    // A cycle (A's parent is B, B's parent is A; same creation time, so only
    // the window's absence and the limit stop the walk). Returns: no hang.
    let cycle: Vec<ProcessStep> = (0..200)
        .map(|index| {
            if index % 2 == 0 {
                step(30_001, 30_002, 500)
            } else {
                step(30_002, 30_001, 500)
            }
        })
        .collect();
    assert!(!serves(Some(30_001), cycle), "a cycle in the parent chain");
    // CONTROL: the window as the 64th step (index 63) still serves.
    let at_limit = long_chain(63);
    assert_eq!(at_limit.len(), 64);
    assert!(
        serves(Some(20_000), at_limit),
        "control: the window's process is the 64th step"
    );
}

#[test]
fn p8_the_port_of_the_windows_own_browser_is_chosen_after_a_foreign_one() {
    let candidates = [foreign_port(9222), own_port(9333)];
    assert_eq!(
        EndpointOwner::serving_ports(&candidates),
        vec![9333],
        "first port: another browser; second: the window's own process"
    );
    // CONTROL: when both serve, the order of the candidates is kept.
    let both = [
        owner_on(9222, Some(CHILD), vec![child(), window(), explorer()]),
        own_port(9333),
    ];
    assert_eq!(
        EndpointOwner::serving_ports(&both),
        vec![9222, 9333],
        "control: both serve, order kept"
    );
}

#[test]
fn p9_no_candidate_serves_gives_no_port() {
    let candidates = [
        foreign_port(9222),
        owner_on(9333, None, Vec::new()),
        owner_on(9444, Some(EXPLORER), vec![explorer(), system()]),
    ];
    assert_eq!(
        EndpointOwner::serving_ports(&candidates),
        Vec::<u16>::new(),
        "another browser, an unreadable owner, the window's parent"
    );
    assert_eq!(
        EndpointOwner::serving_ports(&[]),
        Vec::<u16>::new(),
        "no candidate at all"
    );
    // CONTROL: the same list plus the window's own port gives that port.
    let with_own = [
        foreign_port(9222),
        owner_on(9333, None, Vec::new()),
        own_port(9555),
    ];
    assert_eq!(
        EndpointOwner::serving_ports(&with_own),
        vec![9555],
        "control: the window's own port is kept"
    );
}
