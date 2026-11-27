# Building plan — 8 owner-decided issues (2026-11-27)

Branch: `fix/decisions-batch-2026-11-27` from main `46b0d144`.
Owner decisions taken 2026-11-27 (conversation). This file is the spec; builders report any deviation.

Gate per change: D1 Gate 0 (no `#[cfg(test)]`/`#[test]` surface), `cargo fmt`, `cargo clippy --workspace --all-targets -D warnings`, one release build per batch (`-j 4`, Idle priority). Live proof on port 7700 with the session's Synapse MCP tools where reachable; else code+review (Codex `gpt-6.1-sol` high, read-only). Merge verified batches to main, push as `claude`, close the issue.

Known blocker: **chrome_bridge is unavailable** (`health` → `no_active_chrome_bridge_host`); Chrome 158 removed `--load-extension`, so the bridge needs a one-time manual load (`chrome://extensions` → Developer mode → Load unpacked → `C:\Users\Adam\AppData\Local\synapse\chrome-extension\active`). #286/#328/#332 live proof depends on it; without it they merge as code+review.

---

## Batch A — daemon reporting / surface (no bridge; live-provable on 7700)

### #161 — WebView2 background click/set_field returns ok:false though applied
Decision: **fix the false `ok:false` only** (not the foreground-steal policy).
Change: when input is delivered AND the post-action readback confirms the effect, return `ok:true` with a `foreground_restore_failed` note, instead of `ok:false` / `status:verify_needed` / `ACTION_FOREGROUND_LOST`.
Source: `crates/synapse-mcp/src/server/m2_tools/mod.rs` — the `act_click` / `act_set_field` background path that emits `ACTION_FOREGROUND_LOST` after a foreground change; it already has the post-action readback that confirmed the effect (page reloaded, text set). Distinguish "delivered + verified-by-readback" from "delivered + unverified".
Proof: Finance Manager WebView2 window (per memory `finance-manager-vbv-bench`). `act click` the Refresh button and `act set_field` the search box; expect `ok:true` with the restore note, page actually reloads / filter badge 1. Never save/delete records.

### #333-H — expose the keypress "did anything change?" self-check (verify_delta)
Decision: **build it.** `verify_delta` exists only on the `stroke` object; `act {verb:"key"|"press"}` rejects it (`TOOL_PARAMS_INVALID: unknown field 'verify_delta'`).
Change: add `verify_delta` to the `key`/`press` verb schema on the public `act` facade and wire it to the same postcondition readback the stroke verb uses. No-op key → `ACTION_NO_OBSERVED_DELTA`, not a false pixel-verify.
Source: `act` facade schema for the key/press verb; shared readback in `crates/synapse-mcp/src/server/m2/postcondition.rs` (`ClickDeltaSignature`); impl `act_press`/`act_key`.
Proof: on 7700 target a native window, `act {verb:"key", key:..., verify_delta:true}`; a key that changes the active element → verified; a no-op key → `ACTION_NO_OBSERVED_DELTA`.

### #333-J — safe close of a window this session launched
Decision: **build it, security-scoped.** No facade way to close a launch-opened window today (`process` = list/launch/history only; `browser_tabs close` needs ownership; `window.close()` blocked for launch-opened tabs).
Change: add a `terminate` op to the `process` tool (`crates/synapse-mcp/src/server/m4_tools`), restricted to pids this session launched (recorded in `CF_PROCESS_HISTORY`). Graceful first: `WM_CLOSE` / `CTRL_BREAK`, bounded wait, then forced kill only if it does not exit. Refuse any pid not owned by the session. Not an arbitrary-process-kill capability.
Proof: on 7700, `process launch` a Chrome, then `process terminate` that pid → window closes; `process terminate` a non-session pid → refused.

### #343 — daemon graceful-shutdown endpoint, Dev Hub calls it first
Decision: **(b)** daemon exposes a graceful-shutdown call; Dev Hub invokes it before any forced kill. Scoped to Synapse, no cross-service blast radius.
Daemon side already done: `daemon_shutdown.rs` console-control handler writes the end marker on catchable stops (merged earlier, proven via CTRL_BREAK → `ended_reason:graceful`).
Change (this repo): add an HTTP endpoint (e.g. `POST /admin/shutdown`, loopback-only, bearer/loopback auth) that triggers the graceful drain + end marker, then exits.
Change (separate `dev-hub-rs` project, NOT this checkout): `dev-hub stop synapse` calls the endpoint and waits for exit before `taskkill /F`. File this as a dev-hub-rs task; note the cross-project dependency on #343.
Proof: install a clock-free isolated daemon (`--mode http`, scratch DB/port/token), POST the shutdown endpoint, read `daemon-run-current.json` → `ended_reason:graceful`. The 7700 real daemon stays untouched. End-to-end (Dev Hub) proof waits on the dev-hub-rs change.

---

## Batch B — browser (needs the bridge for live proof)

### #286 — screenshot capture records the browser window for a browser-tab target (match gif)
Decision: **capture records the window too.**
Change: `screenshot capture` on a browser-tab (CDP) target resolves to the browser window the same way `capture_gif_resolve_window` does, instead of refusing.
Source: `crates/synapse-mcp/src/server/capture_gif.rs:209` (`capture_gif_resolve_window`, the accepted path) and `crates/synapse-mcp/src/server/m1_tools/mod.rs` (`perception_window_hwnd`, the refusal `screenshot capture` uses). Make capture follow the gif resolution.
Proof (bridge needed): browser-tab target → `screenshot capture` writes the browser-window image. Without bridge: code + Codex review.

### #328 — route key/press for bridge tabs through the bridge cdpInput lane
Decision: **(a)** mirror how `act click` already works on bridge tabs (`chrome_debugger_bridge.cdpInput`, `Input.dispatchMouseEvent`).
Change (daemon): `act_press_cdp_background_target` (`crates/synapse-mcp/src/server/m2_tools/mod.rs:4517+`) currently resolves input only via `synapse_a11y::endpoint_for_window` → `None` on a bridge tab → `A11Y_CDP_UNREACHABLE` (`:4581`). Add a branch: on a bridge tab, route the key through the bridge `cdpInput` lane (`Input.dispatchKeyEvent`) with key→CDP mapping (`key`/`code`/`keyCode`/`text`/`modifiers`) and a post-delivery readback like the raw-CDP path (`:4646`).
Change (extension, scope first): confirm the bridge `cdpInput` handler (`chrome-extension` `service_worker.js`, deployed at `%LOCALAPPDATA%\synapse\chrome-extension\active`; source in repo) dispatches `Input.dispatchKeyEvent`, not only `typeActiveElement`. If not, add it. Scope the extension side BEFORE building the daemon branch.
Proof (bridge needed): bridge tab, `act {verb:"key", key:"Enter"}` on a search box → form submits/navigates.

### #332 — emulate "Another debugger is already attached" recovery, proven live
Decision: **build the VBV harness, prove the recovery live, then close.**
Already merged (code+review): tool descriptions corrected; emulate recovers its own prior attachment (`service_worker.js`).
Change: build a harness that reliably wedges a tab's debugger attachment (concurrent `emulate` + `evaluate` on one bridge tab, per the issue repro), then show `emulate` recovers (no lingering "Another debugger is already attached").
Proof (bridge + browser_debugger profile needed): stage the wedge, confirm recovery. Without bridge: keep open, harness documented.

---

## Batch C — docs (no runtime; code+review)

### #288 — bring all fork docs in line
Decision: **rewrite the fork docs as one batch** (plus the stale code comment).
Targets (each: make it match the fork — no tests, 40-facade surface, renamed modules, real default timeouts):
- `CONTRIBUTING.md`: drop `cargo test --workspace`; fix dead links (`CODE_OF_CONDUCT.md`, missing README sections).
- `README.md` + `docs/SYNAPSE_40_TOOL_SURFACE_MIGRATION.md`: operation lists vs `FACADE_TOOL_CONTRACTS`; the five flat tools wrongly documented with an `operation` param.
- `CHANGELOG.md`: it stops at #661 and cites gone files (`docs/shared-daemon.md`, a CI workflow, `synapse-test-utils`).
- `SECURITY.md` + README: note loopback binds skip the bearer check (don't tell users a token they set is enforced on loopback).
- `docs/systemdocs/01_system_overview.md`, `02_source_code_map.md`: 13 crates (not 14), tool counts, `server/m1_tools` is now a directory, facade modules in the map.
- `docs/systemdocs/16_*`: `tool_router` location, debug-only tool list, real defaults (`act_launch` 10000 not 30000; `act_spawn_agent` 120000 not 30000), add the 40-facade entry.
- `docs/systemdocs/17_*` + test counts in `18_*`: remove the gone tests.
- `docs/multi-agent-capability-matrix.md`: remove the claim that a now-gone test keeps it in sync.
- `crates/synapse-mcp/src/server/tool_profiles.rs:228`: stale comment naming a removed parity gate.
Proof: none live (docs). Codex read-only review that each edit matches the code it describes.

---

## Sequencing
1. Batch A (independent daemon work, live-provable) — one release build, prove each on 7700.
2. Batch C (docs, parallel-safe, no build) — can proceed alongside A.
3. Batch B (browser) — build; live proof gated on the bridge being loaded.
One build at a time (`-j 4`, Idle). Codex review after each change, read-only. Merge verified batches; #349 closes when the batch is merged.
