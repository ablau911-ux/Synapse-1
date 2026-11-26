# Fix plan: an emulate override on a raw-CDP target must hold, and a set that did not take effect is an error (#337)

Written 2026-11-26 02:30 IST by Claude. Branch `fix/raw-cdp-batch3`, worktree `C:\Users\Adam\Synapse-b3` (cut from `fix/cdp-init-script`, which holds the #335 fix).

## 1. Why

Gitea issue #337, reproduced live by Claude: on a raw-CDP tab `browser_debugger emulate set viewport 500x400` answers ok with `requested 500x400` and `readback 704x510`; a later read is `704x510`. `emulate set media {color_scheme:"light"}` answers ok with readback `color_scheme_dark: true`. Raw-CDP sibling of #335 (init script, fixed with a long-lived per-target session) and #336 (bridge tab). The owner approved autonomous testing and bug fixing until 2026-11-26 06:00.

## 2. Upstream

Our main holds everything origin/main has; upstream has the same code and no fix.

## 3. Facts (read by an Opus 4.8 investigation at `c37b3586`; line numbers from that tree)

- `browser_emulate_impl`, raw-CDP part: `crates/synapse-mcp/src/server/browser_emulate.rs` 281-533; domains: viewport 303, device 342, geolocation 360, locale 391, media 411, network 435; reset branch 443-516; answer built 519-531; `domain_result` (1244) only serialises, nothing compares readback with request.
- `crates/synapse-a11y/src/cdp_emulation.rs`: viewport `cdp_set_viewport_size` (298) → `cdp_set_viewport_size_with_mobile`; media `cdp_set_media_override` (561) → `run_media_command` (1359); device `run_device_metrics_command` (982); network `run_network_conditions_command` (1410); geolocation and locale runners likewise. Every `run_*` does a fresh `Browser::connect(endpoint)` (media 1367, device 997, network 1424), spawns a handler task, `get_target_page_with_discovery`, `page.execute(override)`, then `handler_task.abort()`; the browser drops and the CDP client disconnects. The readback is another new connection (`media_readback` 1646 → `cdp_evaluate_expression`). Chrome ends a session's emulation overrides when the session closes (the premise the `InitScriptSlot` doc states, `cdp_action.rs` 58-63; it fits the live rows).
- Reset: each `run_*(…Reset)` opens its own connection and sends the clear (media 1392, device `ClearDeviceMetricsOverride` 1023, network 1455).
- No emulate domain holds today. The long-lived slot pattern exists for other features: `InitScriptSlot` + registry (`cdp_action.rs` 58-164, target-gone = `handler_task.is_finished()` in `lookup_live_init_script_slot` 94, empty-close 166, drain `durable_init_scripts_disable_and_drain_all`), `NetworkOverrideSlot` (`cdp_network.rs` 785, `network_override_ensure` 2196, `override_registry()`).
- Readback types per domain: `CdpViewportReadback` (inner size), `CdpMediaReadback` (color scheme, reduced motion, media type), `CdpDeviceReadback`, `CdpLocaleTimezoneReadback`, `CdpGeolocationReadback` (position or error), `CdpNetworkConditionsReadback` (weakly observable). `finite_f64_eq` exists in `cdp_emulation.rs`.

## 4. The rule to build

Rule 1 (holds): an override set with `emulate` on a raw-CDP target holds until `emulate reset` of that domain, the target goes away, or the daemon drains. It holds while other tools work on the target.

How: an emulation slot per target in `cdp_emulation.rs`, modelled on `InitScriptSlot`: the kept-open browser, its handler task, the page, and which domains currently hold an override. Its own registry (init scripts and emulation stay separate owners: different identity and different drain meaning). Every `run_*` set sends on the slot's page instead of a fresh connection; the readback of a set may stay a separate connection (the override is live while the slot lives). Reset of a domain sends the clear on the slot's session and forgets the domain; when no domain is left the slot closes. A reset for a domain with no live slot keeps working as today (clear on a fresh connection is harmless) and answers as today. Target gone or connection lost: the slot is forgotten (same detection as the init-script slot). Daemon drain: the slots join the existing drain where the init-script and network slots are drained.

Rule 2 (honest answer): after a set, when the domain's readback can be compared with the request and does not match, the set is an error that names the domain, the requested and the read values; the override that did not take effect is cleared and its domain forgotten. Compare only what is truly comparable: viewport inner size (width and height; device scale factor only if the readback carries it reliably), media (each requested feature), locale and timezone (if the readback carries them), device (what its readback carries). Geolocation and network: no comparison unless the readback is exact; say what you decided and why. Use the error type the neighbours use for a failed emulate command; do not invent a new error code unless none fits.

Not changed: the bridge path (`chrome-tab:` targets, #336 is fixed elsewhere); the answer's field names.

## 5. Files

`crates/synapse-a11y/src/cdp_emulation.rs` (slot, registry, every `run_*`, the `cdp_set_*` / `cdp_reset_*` functions), the drain hook (where `durable_init_scripts_disable_and_drain_all` is called, `crates/synapse-a11y/src/cdp_network.rs` about 1988), `crates/synapse-a11y/src/lib.rs` if an export is needed, `crates/synapse-mcp/src/server/browser_emulate.rs` only if the error needs mapping.

This builder owns ONLY these files. Another builder works at the same time in this worktree on `crates/synapse-a11y/src/cdp_action.rs`, `crates/synapse-mcp/src/m2/press.rs` and `crates/synapse-mcp/src/m2/press/schema.rs` (#329): read them if you need, do not edit, stage or commit them. If you must change one line in `cdp_network.rs` for the drain, that file is yours.

## 6. Order, commits, build rule

Commit 1: the slot and Rule 1. Commit 2: Rule 2. No Rust tests (Gate 0). No cargo command except `cargo fmt --all` right before a commit. Commit only your own paths with `git commit --only -m "… (#337)" -- <paths>`. No push. Claude runs the gate, installs on port 7700 and runs the proof.

## 7. Proof (run by Claude on the real server with the session's Synapse MCP tools; automation Chrome from `process launch`, tab bound as `cdp` target, profile `browser_debugger`)

| call | pass when |
|---|---|
| `emulate set viewport 500x400`, then a separate `evaluate` of `innerWidth+'x'+innerHeight` | set ok with readback 500x400; evaluate `500x400` |
| `emulate set viewport 640x360`, `evaluate` | ok; `640x360` |
| `browser_nav reload`, `evaluate` | `640x360` (held across a navigation) |
| `emulate set media {color_scheme:"light"}` on a page whose own value is dark, `evaluate` of `matchMedia('(prefers-color-scheme: light)').matches` | ok with `color_scheme_light: true`; `true` |
| `emulate reset` (viewport, media), `evaluate` both | natural size; `dark` again |
| a set that cannot take effect (the builder names one honest case, or says none can be produced live) | an error that names domain, requested and read values |
| close the automation Chrome, `health` | daemon ok |

## 8. Report the builder gives back

Model id; the slot type and where it lives; set / reset / target-gone / drain in one sentence each; per domain: held yes/no, compared yes/no and why; commit hashes; the expected answer for each proof row; what could not be checked without compiling or a live Chrome.
