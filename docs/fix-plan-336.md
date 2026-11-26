# Fix plan: an emulation override on a bridge tab must hold until it is reset (#336)

Written 2026-11-26 02:10 IST by Claude. Branch `fix/emulate-override-held`, cut from main `4c705596`, worktree `C:\Users\Adam\Synapse-ext`.

## 1. Why

Gitea issue #336 (calls and answers there, comment 3541 has the table). `browser_debugger emulate set viewport` on a normal Chrome bridge tab:
- visible tab: fails nearly always ("viewportEmulation readback did not settle within 10000 ms", last viewport = the natural size);
- hidden tab: the first set after a reload reads the requested size, every later set reads that old size and fails.
The owner approved autonomous testing and bug fixing until 2026-11-26 06:00.

## 2. Upstream

The path was written once, upstream: commit `4d70b624` "fix(mcp): add normal bridge viewport emulation (#1173 #1215) [skip ci]" (2026-06-23). No stated lifetime of an override, no live proof. Our main holds everything origin/main has; upstream has no fix.

## 3. Facts (read in `extensions/synapse-chrome-debugger/service_worker.js` on main `4c705596` by an Opus 4.8 investigation; line numbers from that tree)

- `handleViewportEmulation` set branch (8560-8583): `dispatchViewportEmulationSet` (8571) returns only after its own `finally` detach (`chrome.debugger.detach`, 9909); then `applyViewportDprShim` (8572); then `waitForViewportReadback` (8581-8583). So the readback polls a tab whose debugger is already detached.
- `dispatchViewportEmulation` (9870): `chrome.debugger.attach` (9874) → `Emulation.setDeviceMetricsOverride` (9925) → detach (9909). Raw attach and detach, not `attachDebuggerForCommand`.
- Chrome drops a device-metrics override when the debugger that set it detaches (inference from Chrome's behaviour, and it fits every live row: a visible page is back at its natural size at once; a hidden page keeps its last layout, so the first override looks as if it held).
- Nothing holds the override: `persistentDebuggerSessionIsAttached` (13067-13071) is true only for init-script, binding, dialog and file-chooser sessions (registry model: `INIT_SCRIPT_DEBUGGER_SESSIONS`, 133). `attachDebuggerForCommand` (12978-12991) borrows such a session (`shouldDetach:false`, 12981); the comment at 5537-5539 states the must-never-detach rule for a borrowed session.
- The ledger expects the override to be live later: `DURABLE_OWNER_LEDGER.viewportOverrides` (950, merged from `VIEWPORT_BASELINE_BY_TAB` at 1995), reconnect cleanup at 13823-13829 (`dispatchViewportEmulationReset` + baseline restore), reset at 8588-8612.
- `dispatchDeviceEmulationSet` (9459) sends `Emulation.setDeviceMetricsOverride` (9469) and detaches the same way.
- The Rust side (`crates/synapse-mcp/src/server/browser_emulate.rs`) passes through; nothing to change there.

## 4. The rule to build

An override set with `emulate` on a bridge tab holds until `emulate reset` of that domain, the tab closes, or the bridge reloads. It holds while other tools work on the tab. The readback of a set is taken while the override is live.

How: the debugger attachment that set the override stays attached and is registered as a persistent session, the way the init-script sessions are (`INIT_SCRIPT_DEBUGGER_SESSIONS` and `persistentDebuggerSessionIsAttached`). One owner: an emulation-session registry keyed by tab, which records which emulation domains currently need the attachment.

- `set`: attach through `attachDebuggerForCommand` (so a session held for another reason is borrowed, never detached), send the override, register the domain in the registry, do not detach. Then the readback.
- `reset` of a domain: send the clearing commands on the held session, unregister the domain; when no emulation domain is left for the tab and no other persistent reason holds the session, detach. The existing baseline restore and ledger clear stay.
- A failed set leaves nothing behind: if the override could not be applied or the readback failed, the domain is unregistered and the session released under the same rule as reset. The error stays the existing one.
- Tab closed, debugger detached by Chrome or by the user (the "cancel" of the debugging bar, `chrome.debugger.onDetach`): the registry entry is dropped and the ledger entry cleared, so nothing later believes an override is live. Follow how the init-script sessions handle `onDetach` and `onRemoved`.
- Bridge reload / service worker restart: follow the init-script sessions and the existing reconnect cleanup (13823-13829); after it, either the override is really live and registered, or it is really cleared and forgotten. Not a third state.
- Every domain of `emulate` whose override lives in the debugger session gets the same rule through the same registry (one mechanism, not one per domain). Read each domain's set path and decide from the code which ones depend on the attachment (device metrics in `dispatchDeviceEmulationSet` certainly; user agent, touch, locale, timezone, geolocation, media, network conditions: check each, they are CDP `Emulation.*`/`Network.*` overrides that Chrome ties to the session). Domains implemented with a MAIN-world shim only are not touched.
- The answer keeps its shape. Do not add fields unless a caller needs them.

Not changed: raw-CDP tabs; the Rust side; the screenshot and navigation paths except that they must keep borrowing through `attachDebuggerForCommand` (if one of them does a raw detach on a tab, say so and fix it only if it would drop a held override).

Known consequence, accepted: while an override is live Chrome shows its "started debugging this browser" bar on that tab, as it does today for init scripts.

## 5. Files

`extensions/synapse-chrome-debugger/service_worker.js` only (the builder confirms). No cargo build is needed for this fix.

## 6. Order, commits

One commit per concern: (1) the held emulation session and the viewport path on it; (2) the other session-bound domains moved onto it, only if step 1's mechanism needs more than a registration call per domain, otherwise all in commit 1. No tests in the repo's test paths. The builder runs no cargo command. `node --check extensions/synapse-chrome-debugger/service_worker.js` must pass (syntax). Commit as git user `claude` with `git commit --only -m "..." -- <paths>`. No push. Never open the deployed extension copy under `%LOCALAPPDATA%\synapse` (it holds a token).

## 7. Proof (run by Claude on the real server with the session's Synapse MCP tools, bridge deployed from this worktree; a tab opened by `browser_tabs new` in the owner's Chrome, profile `browser_debugger`)

| state | calls | pass when |
|---|---|---|
| active tab, visible | `emulate set viewport 500x900`; separate `evaluate` of `innerWidth+'x'+innerHeight` | set ok 500x900; evaluate `500x900` |
| same | `emulate set viewport 600x800`; `evaluate` | ok 600x800; `600x800` |
| same | `browser_nav reload`; `evaluate` | `600x800` (held across a navigation and other tools) |
| same | `emulate reset viewport`; `evaluate` | natural size; a later set works again |
| inactive tab (hidden) | set 500x900, set 600x800, each followed by `evaluate` | each either ok with the true size, or one clear error that says what is wrong; never ok with a stale size |
| each other session-bound domain the builder names | set, separate `evaluate` of the page value, reset, `evaluate` | value held after the set call, back after reset |
| close the test tab | `browser_tabs list`, `health` | tab gone, bridge healthy |

## 8. Report the builder gives back

Model id; the registry and where it lives; set/reset/failed-set/onDetach/tab-close/bridge-reload in one sentence each; the list of emulate domains with "session-bound: yes/no" and the code line that decides it, and for each the exact `emulate` call and the page expression that proves it; commit hash(es); `node --check` result; what could not be checked without a live Chrome.
