# Fix plan: a raw-CDP init script is registered on a session that closes with the call (#335)

Written 2026-11-26 02:00 IST by Claude. Branch `fix/cdp-init-script`, cut from main `4c705596`.

## 1. Why

Found by the raw-CDP sweep (#333 item D) and reproduced by Claude on the real server (Gitea issue #335 holds the calls and answers): `browser_debugger add_init_script` on a raw-CDP tab answers ok with `identifier "1"`, and after a reload the script has not run. The owner approved autonomous testing and bug fixing until 2026-11-26 06:00.

## 2. Upstream

Our main holds everything origin/main has (checked 2026-11-25 20:39); upstream has the same code. No upstream solution.

## 3. Facts (read in the code by an Opus 4.8 investigation on main `4c705596`; line numbers from that tree)

- Raw branch of the handler: `browser_add_init_script_impl`, `crates/synapse-mcp/src/server/m1_tools/mod.rs` 8657-8705; the add calls `synapse_a11y::cdp_add_init_script_target` (8667).
- `crates/synapse-a11y/src/cdp_action.rs`: `cdp_add_init_script_target` (3105) → `cdp_add_init_script_target_owned` (3136) runs inside `with_target_page` (3150). `with_target_page` (6653) does a fresh `Browser::connect(endpoint)` on every call (6664) and ends with `finish_chromiumoxide_handler` (6678), which aborts the handler task and drops the browser (6558). So the `Page.addScriptToEvaluateOnNewDocument` registration, which belongs to that CDP session, ends when the call returns. The identifier is `"1"` each time because every call is a new session.
- `cdp_remove_init_script_target` (3282) also opens a new session; the identifier means nothing there.
- `durable_init_script_registry` (65; key endpoint + target + identifier, 3263) is a cleanup ledger for the panic/shutdown drain (3321), not a re-registration mechanism.
- Long-lived raw-CDP sessions exist for other features, each a static registry slot that keeps `_browser` and `handler_task`: binding capture ("keeps one long-lived CDP session per target", `cdp_binding.rs` 1-4), `NetworkOverrideSlot` and `FetchInterceptionSlot` (`cdp_network.rs` 785, 772; `network_override_ensure` 2203-2258; `override_registry()` 831), dialog (`cdp_dialog.rs`), console (`cdp_console.rs` 179), lifecycle (`cdp_lifecycle.rs` 284). They survive between calls and across reloads because their session stays open.

## 4. The rule to build

An init script added on a raw-CDP target stays registered until it is removed, the target goes away, or the daemon drains: it runs in every new document of that target.

How: a dedicated long-lived CDP session per target for init scripts, built the way `cdp_binding.rs` keeps its session (same registry shape, same lock discipline, same drain hooks). One owner: a slot type that holds the kept-open browser, its handler task and the page, plus the scripts registered on it (identifier → source length or hash, whatever the answer needs).

- `add`: get or create the target's slot, send `Page.addScriptToEvaluateOnNewDocument` on the slot's session, record the identifier Chrome returns, answer as today. The identifier is now unique per target while the slot lives.
- `remove`: find the slot, send `Page.removeScriptToEvaluateOnNewDocument` on the same session; an unknown identifier is a clear error (not a silent ok); when the last script of a slot is removed the slot is closed.
- Target gone or connection lost: the slot is dropped and its scripts forgotten; a later `remove` says the script is no longer registered.
- Drain / panic cleanup: the slots join the existing drain the way the binding and network slots do; `durable_init_script_registry` either becomes this slot registry or is fed from it. There must be one source of truth for "which init scripts are registered", not two.
- The answer keeps its shape (`identifier`, `operation`, `source_len`, `readback_backend`, …). If a field can now honestly say that the registration is held by a persistent session, use the existing field for the backend description; do not add new fields unless a caller needs them.

Not changed: the bridge path (`chrome-tab:` targets); ownership checks of the handler; other tools.

Check while there (report, change only if it is the same defect and small): the investigation inferred that an `emulate` path on raw-CDP that goes through `with_target_page` would lose its override the same way. Read it and say what is true.

## 5. Files (the builder confirms by reading)

`crates/synapse-a11y/src/cdp_action.rs` (the two init-script functions and the registry), a new module in `crates/synapse-a11y/src/` only if the slot does not fit in an existing file cleanly (model: `cdp_binding.rs`), the drain hooks (search where the binding/network slots are drained), `crates/synapse-mcp/src/server/m1_tools/mod.rs` only if the call signature changes.

## 6. Order, commits, build rule

One commit for the fix (a second one only for formatting noise). No Rust tests (Gate 0). The builder runs no cargo command except `cargo fmt --all`. Commit as git user `claude` with `git commit --only -m "..." -- <paths>` (new files: `git add` them first, then commit only those paths). No push. Claude runs the gate (one build, 4 jobs, Idle priority), installs on port 7700 and runs the proof.

## 7. Proof (run by Claude on the real server with the session's Synapse MCP tools; automation Chrome from `process launch`, tab bound as `cdp` target, profile `browser_debugger`)

| call | pass when |
|---|---|
| `add_init_script {source:"window.__synapseInitRan = (window.__synapseInitRan \|\| 0) + 1;"}` then `browser_nav reload`, then `evaluate` of `String(window.__synapseInitRan)` | `1` |
| `browser_nav navigate` to another local page, `evaluate` | `1` (runs in the new document) |
| a second `add_init_script` with another variable | a different identifier; after reload both variables are set |
| remove the first script (the remove form the tool's schema gives), reload, `evaluate` | first variable `undefined`, second still set |
| remove with an unknown identifier | a clear error |
| close the automation Chrome, then `health` | daemon ok (no leaked task noise in the answer) |

## 8. Report the builder gives back

Model id; the slot type and where it lives; how add/remove/drain work in one sentence each; whether `durable_init_script_registry` was kept, replaced or fed; what is true about raw-CDP `emulate`; commit hash; the expected answer for each proof row; what could not be checked without compiling or a live Chrome.
