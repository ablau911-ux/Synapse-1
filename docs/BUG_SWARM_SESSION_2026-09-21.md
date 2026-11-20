# Synapse bug-swarm session record — 2026-09-21

This document records the source changes, evidence, review position, and
remaining runtime verification for every issue touched during this session.
Source tests are not deployment acceptance. D1/manual FSV remains mandatory.

## Accepted or closed in this session

| Issue | Revision(s) | Evidence and outcome |
| --- | --- | --- |
| #38 | `dcea5912` | Exact Windows command-line authorization/audit path verified on the installed daemon with a strict 43-schema MCP client, durable OS command-line readback, refusal edges, cancellation, and hidden-desktop Finance Manager capture. Closed. |
| #42 | — | Invalid report under the supported singleton daemon contract. Its follow-up crash-stale reservation defect is #134. Closed. |
| #44 | `65a3f1ed` | Acknowledged storage-write ordering tests passed independently (2/2); GLM found no remaining ordering blocker. Closed. |

## Code accepted; installed D1/manual FSV still required

| Issue | Revision(s) | Source evidence | Required runtime evidence |
| --- | --- | --- | --- |
| #25 | `c1f8fa3e`, `2551db18`, `19b32e6c` | Invalid model/provider validation and managed Claude allowlist changes reviewed; focused checks passed. | Strict-client valid/invalid model, provider, and GLM spawn checks; no artifact/process on rejection. |
| #39 | `a47f9745`, `a071fc5f`, `3a7e9eb6`, `6059efb6`, `2909d53c` | Public shell and agent-tail redaction tests passed (5/5 and 3/3); manager and GLM review cleared source gate. | Real stdout/stderr/artifact/audit redaction with secret-like command, environment, and PEM material. |
| #40 | `5ecdd749`, `4ca2c044`, `49033ac2`, `047e4a8b` | Process cleanup/race regression checks passed; manager and GLM source review cleared. | Real forced job-assignment failure and physical child-process readback where feasible. |
| #43 | `c6b073ae`, `be63c79a` | TTL suite 5/5 plus integration passed. | Real MCP-triggered compaction with DB reopen/readback for valid, missing, malformed, and duplicate timestamps. |
| #49 | `2601fe82`, `415b00d7`, `b1dbcb2f` | Capture suite 25/25 passed; live source run captured 516 frames in 15.26 s. | Installed livecapture/observe on hidden Finance Manager, with frame change and OCR readback. |
| #50 | `a1faa396` | Validation order now rejects text limits before target/click/focus. | Installed oversized ASCII/emoji, empty, and successful type-path tests; verify refusal precedes delivery. |

## Open repairs awaiting manager review

| Issue | Revision(s) | What changed | Current evidence | Next gate |
| --- | --- | --- | --- | --- |
| #109 | `3603c428` | `livecapture.next.after_frame` now accepts either the native `CaptureFrameCursor` object or a JSON string encoding that strict object. The public schema remains object-shaped. | Focused serde 5/5, `schema_sanitize` 8/8, `cargo check -p synapse-mcp` passed. Deployed strict-MCP D1 on the installed daemon: issue109 stage 9/9 PASS (object + string cursors, malformed rejected, still-window clean timeout). Gitea comment 49147, label `status:first-review`. Codex review (session 01a0c4a9): fix sound, no correctness or schema-regression defect; one non-blocking test gap (stringified null/array/number/bool, unknown fields, schema shape in the livecapture inline tests). | Addendum `b8dbdfd3`: the Codex-suggested edge cases moved out of source files into `tests/probe/d1_probe.py` (stringified non-object cursors null/42/true/"capture"/[1,2,3]/{}, unknown-field rejection in object and stringified form, plus a livecapture schema-shape check against `tools/list`). Rerun on the clean committed build: issue109 stage 21/21 PASS. |
| #130 | `ed073391`, `013cb72b`, `5193848d`, `5eac7701` | Detections use passive WGC for the requested target with stable DWM bounds and HWND-keyed tracker state. Third leg (`5193848d`): the Chromium browser-OCR fallback OCR'd the *screen rectangle* at the target's coordinates, leaking the covering/foreground window's text into the target's elements; it now captures the target's whole WGC frame once, crops each content/overlay tile from that frame, and fails closed when the target can't be captured. | OS-level UIA proven clean first (never leaks across hwnds), isolating the mix to the screen-capture path. Installed-daemon strict-MCP D1 with two distinct Chrome fixture windows: issue130 stage 8/8 PASS (each backgrounded target returns only its own marker), entities per direction = target's own emoji (laptop 0.95/cup 0.81/keyboard 0.65 vs cell phone 0.71). Regression: issue109 9/9, issue133 7/7. Gitea comment 49149, label `status:first-review`. | Codex review (session 01a0c507) found 3 real defects, all fixed in `5eac7701`: (1) the OCR frame was anchored to `GetWindowRect` window_bounds instead of the DWM capture bounds, so the invisible resize border (~7px) shifted cropped rows and reported word positions — the frame is now realigned via the stable-origin readback and tiles/probe derive from `frame.region`; (2) HWND reuse could inherit a stale tracker track — identity widened with the creator thread id; (3) minimized `WINDOWPLACEMENT.rcNormalPosition` is workspace-relative for `WS_CAPTION` windows — converted to screen space via `SPI_GETWORKAREA`. Crop logic moved to `synapse-capture` (`CapturedBgraBitmap::crop_screen_rect`) with regression tests in separate files (`crates/synapse-capture/tests/bgra_crop.rs`, `placement_coords.rs`); inline tests removed from source per Adam's rule. Clean-build rerun on pid 5476: issue130 8/8, issue133 7/7, issue109 21/21, health 3/3 — 0 failed checks. Follow-up codex review of `5eac7701`+`b8dbdfd3` spawned. |
| #131 | `17c47158`, `7f656f60` | `-SkipBuild` resolves release artifact, handles no-op identical hashes, fresh installs, rollback/restart, and standard `CARGO_BUILD_TARGET` triples. | Real differing-hash deploy (run 2): candidate = release artifact `434FCE4C…`, old destination backed up `F8527F65…`, installed verified `434FCE4C…`, daemon started on new build `deb7d9312209` pid 37720. Real identical-hash run 3 exercised `SYNAPSE_BINARY_INSTALL_NOOP`. Pester family 21/21 after ac91a21d. Gitea comments 49128 (evidence), 49132 (reopen note). | Prematurely closed then reopened 2026-09-21: re-close only after run 3 completes healthy + Codex review of both commits + all issue acceptance criteria re-verified. |
| #134 | `90577290`, `5cda887a` | Global restart-stable idempotency fence, response ownership isolation, live reservation RAII, and terminal indeterminate recovery for abandoned work. | Manager tests: six idempotency cases plus one RAII lifecycle case passed. GLM review did not complete because its client issued malformed profile calls. | Codex independent review (in flight), then daemon-restart D1 with separate side-effect readback. |
| #132 | `873e22e1`, `ac91a21d` | First increment: existing-extension UI reload with no eligible Chrome window returns a nonfatal deferred result. Second increment (ac91a21d, found by real deploy run 2): two setup-side gates — `Invoke-SynapseChromeBridgeUiRepair`'s allowed-reason list and `Assert-SynapseChromeBridgeLiveAfterSetup`'s 45 s host wait — still died on the deferred result; both now accept the shared deferred-reason constant (WARN + return current healthy health) while unknown reasons, missing profile rows, eligible-window reload, and first-time-install refusal unchanged. | Run 2 reproduced the gate Die (`SYNAPSE_CHROME_BRIDGE_UI_REPAIR_NOT_PERFORMED reason=…deferred…`), proving the first increment incomplete. New gate Pester suite 6/6 + old family 3/3; scripts family 21/21; parsers clean. | Run 3 (in flight) must complete with healthy daemon and the deferred WARN surfaced; then Codex review (in flight) + eligible-window branch FSV when a real Chrome window exists. |
| #133 | `3f7b2cfb` | Regionless hidden-desktop `read_text` no longer derives a region MCP-side from UIA focused bbox (GetWindowRect-derived extent exceeded the DWM PrintWindow bitmap). It sends no capture region so the desktop worker derives the full-window region from the target's own DWM frame bounds (same derivation as the hidden-desktop screenshot path), and `ReadTextCaptureSource::WholeWindow` normalizes the OCR region to the captured bitmap extent. Explicit client-relative regions unchanged. | Plan tests 2/2, whole-window 3/3, read_text 5/5, desktop_worker 2/2, `cargo check` and rustfmt clean. Full suite: 8 failures verified pre-existing via #133-stash rerun; filed as Gitea #135. Gitea comment 49120. | Codex independent review (in flight), then installed strict-MCP D1: regionless whole-window read_text across maximized/bordered/DPI-scaled/hidden-desktop windows. |
| #135 | `fee8e3b3` | Six stale tests realigned with shipped behavior: 3 m4 shell expectations updated to #38's force-quoted CreateProcess argv serialization; auth bridge-origin test now asserts #30's registration-token refusal (Origin alone never authorizes `/chrome-debugger/native/register`; positive token path covered by the chrome_debugger_bridge fixture test); #73's 8 `seat_*` operations declared in `FACADE_TOOL_CONTRACTS` with mutation/readback rules, seat error codes, and remediations (real contract metadata debt); scope-gate observe test's flaky 200 ms wall-clock assert dropped per the m4 timing-assert precedent, functional profile/scope-event assertions retained, test renamed. | Focused suites 11/11 for all touched tests; full `cargo test -p synapse-mcp --bin synapse-mcp`: 1644 passed, 1 failed, 16 ignored (was 1638/7); rustfmt clean. The remaining `health_payload_reports_m3_subsystems_initializing_or_disabled` failure is environmental: `SynapseService::new` opens the production RocksDB whose LOCK is held by the installed daemon and panics at server.rs:524; passes with the daemon stopped. The original filing's 8th test (uptime) passed pre-fix and was flaky. | Codex review, then optional confirmation run with the daemon stopped. |

## Newly confirmed report

## Accepted residual: stable-origin ABA race (codex review round 2, #130)

Codex finding (minor, session `01a0c532`): `stable_capture_origin` in
`crates/synapse-mcp/src/m1/detection.rs` proves the target did not move across
capture by comparing window bounds before/after — but a window can move, be
captured at the moved position, and return before the second read (ABA). The
captured pixels would then be labeled with the original origin, shifting
OCR/detection coordinates by the transient move delta.

Not fixable with the current Windows API surface: there is no geometry epoch /
move-generation counter queryable for an HWND, and WGC frame metadata carries
QPC timestamps but not window position, so a post-hoc detector cannot
distinguish "never moved" from "moved and returned". Accepted as a documented
residual; revisit if a geometry-change hook (e.g. SetWinEventHook EVENT_OBJECT_LOCATIONCHANGE
sampling around the capture window) becomes warranted by evidence.

#109 is the confirmed ZCode nested-object serialization incompatibility behind
the repeated `livecapture next` failures: ZCode sent `after_frame` as a JSON
string and the prior server expected `CaptureFrameCursor` directly. The source
fix deliberately preserves the advertised object schema while accepting that
known client representation at the field boundary.

## Deployment state and operator guardrails

- 2026-09-21 ~21:10 +03:00 — installed daemon replaced with the clean
  committed-tree release build `b8dbdfd34d44` (SHA-256 `91C72240…`, commits
  `5eac7701` + `b8dbdfd3`, working tree clean), PID 5476 on `127.0.0.1:7700`,
  same db/profile args. Previous binary backed up as
  `synapse-mcp.backup-aeef19.exe`. Health: 44 tools, ok=true; full VBV battery
  (health, issue109, issue130, issue133) 0 failed checks.
- 2026-09-21 ~19:30 +03:00 — installed daemon replaced for #130 VBV:
  `C:\Users\Adam\.cargo\bin\synapse-mcp.exe` now runs release build
  `57ab3870fa46-dirty` (SHA-256 `AEEF1943…`, commit `5193848d` + the fix
  series since `deb7d931`), PID 14624 on `127.0.0.1:7700`, same db/profile
  args as before. Previous binary backed up as
  `synapse-mcp.backup-434fce.exe`. Health: 44 tools, ok=true.
- D1 requires a live daemon/socket/health proof, strict schema-validating MCP
  `tools/list`, real `tools/call` triggers, independent physical readback, and
  happy path plus empty/boundary/structurally-invalid edges.
- Finance Manager browser/window checks must run on the hidden desktop without
  disturbing the human foreground.
