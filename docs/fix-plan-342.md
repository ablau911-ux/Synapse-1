# Fix plan: raw-CDP tap sends a valid touch-end (#342), the mouse stroke follows the closed-target rule (#341), the bridge geolocation answer tells who grants the permission (#344)

Written 2026-11-26 05:42 IST by Claude. Branch `fix/raw-cdp-touch-end-stroke-geo`, cut from main `1029f6c3`, worktree `C:\Users\Adam\Synapse-stress`.

## 1. Why

Three open Gitea issues, all found tonight on the real server (build of main `1029f6c3`):

- **#342**: `act {verb:"tap"}` on a raw-CDP page that stays open always answers `ok:false`: "touchEnd acknowledgement was not proven … Error -32602: Invalid parameters; unresolved_raw_cdp_input_owner_count=1". The page gets `touchstart` and never `touchend`.
- **#341 remainder**: the click and the touch tap got the rule "press delivered, release failed, target gone = delivered". The raw mouse stroke did not. The answer of a tap whose target closed still lists `touchEnd` in `dispatched_events` and says `delivered_and_verified`.
- **#344**: `browser_debugger emulate set geolocation` on a bridge tab answers `permission_state: "granted"` and `permission_setting: "granted"` while `Browser.setPermission` failed and only the page shim grants.

## 2. Upstream

`origin/main` (b3a546c4) is contained in our main (`git merge-base --is-ancestor origin/main HEAD`, checked 05:41). The code of all three items is the same there; upstream has no fix. Nothing to copy.

## 3. Facts (each read or measured by Claude)

- `touch_event` (`crates/synapse-a11y/src/cdp_action.rs` line 6471) builds `DispatchTouchEventParams::new(kind, touch_points)` with `Vec::new()` when no point is given.
- The library drops the empty list: `chromiumoxide_cdp-0.9.1/src/cdp.rs` line 63203-63205: `#[serde(rename = "touchPoints")] #[serde(skip_serializing_if = "Vec::is_empty")] pub touch_points: Vec<TouchPoint>`.
- Chrome 158 measured with a direct CDP session (`cdptouch.mjs` in the session scratchpad): `{type:"touchEnd", touchPoints:[]}` → ok, the page shows `touchstart touchend`; `{type:"touchEnd"}` → `-32602 … Failed to deserialize params.touchPoints - BINDINGS: mandatory field missing`.
- Users of `touch_event` with no point: line 6115 (`serde_json::to_value(touch_event(TouchEnd, None)?)`, the persisted touch release) and line 6543 (the touch-end of `dispatch_touch_tap_on_page`).
- `settle_failed_release_after_press` (line 5807) is the shared helper of #341; its callers are at lines 6445 (click), 6559 (touch tap) and 6666. `dispatch_cdp_mouse_stroke_raw` (line 5598) still ends a failed release with `combine_raw_input_failures` (line 5760).
- Bridge geolocation answer: struct field `permission_setting` at `crates/synapse-mcp/src/chrome_debugger_bridge/mod.rs` line 3177, the JSON answer is built near line 10033. The extension (`extensions/synapse-chrome-debugger/service_worker.js`) already computes `emulated_by_shim` (added in 7ea75d83); no Rust file mentions it.

## 4. Rules to build

### Part A (#342): a touch-end always carries `"touchPoints": []` on the wire
One owner of the rule: the place that builds a touch event for sending. Both users (the tap and the persisted release) must get it from that one place. The typed library command cannot say it, so the touch-end needs either its own small command type for `Input.dispatchTouchEvent` that always serialises the list, or the JSON value with the key set, sent through the raw-command path the file already uses for persisted releases. Use whichever the file already has a pattern for; do not add a second way to send commands. `touchCancel`, if the file sends one, has the same rule.

### Part B (#341): the mouse stroke follows the closed-target rule
In `dispatch_cdp_mouse_stroke_raw`, when the button press was delivered and the release fails, call `settle_failed_release_after_press` like the click does. Target absent: the stroke is delivered, the owner is resolved, the result says "release not sent, target closed" in the same field name the click uses (`release_not_sent_target_closed`) if the stroke result type can carry it; one field for one fact. Target present or check inconclusive: today's failure, unchanged. A press that was not delivered: unchanged. Carry the field to the tool answer on every lane that answers a stroke.

Also for tap and click answers when the target closed: `dispatched_events` must list only events that were sent, and the delivery state must not say `verified` when no readback ran. Follow how the key press answers this case (#329).

### Part C (#344): the bridge geolocation answer states the real permission
The Rust answer carries what the extension reports: the flag `emulated_by_shim` and the real permission state. `permission_state` / `permission_setting` must not say "granted" alone when only the shim grants. Smallest correct form: keep the existing keys with their true values and add `emulated_by_shim` (bool) to the Rust answer type and the JSON answer; if the extension's own value for the real permission has a name, pass it through under that same name. No extension change unless the extension does not send what the Rust side needs; if it must change, say so in the report (it needs a redeploy).

### Not in this plan
The long wait (about 40 s) when Chrome never answers the release of a closed target. It stays; the answer is right. It is listed on #341.

## 5. Files (the builder confirms by reading)

`crates/synapse-a11y/src/cdp_action.rs`; the synapse-mcp callers that answer a stroke, a tap and a click (`crates/synapse-mcp/src/server/background_router/mod.rs`, `crates/synapse-mcp/src/server/m2_tools/mod.rs`, `crates/synapse-mcp/src/m2/`); `crates/synapse-mcp/src/chrome_debugger_bridge/mod.rs`; examples under `crates/*/examples` that read a changed result type.

## 6. Order, commits, build rule

One commit per part: A, then B, then C. No Rust tests (Gate 0). No cargo command except `cargo fmt --all` right before a commit. Clippy runs with `-D warnings` on pedantic and nursery: no `pub(crate)`/`pub(super)` in private modules of synapse-a11y (use a documented `pub` or keep it private), `#[must_use]` on new public value-returning functions, no needless `async`, the file's `#[allow(clippy::too_many_arguments)]` convention. Commit as git user `claude` with `git commit --only -m "… (#NNN)" -- <paths>`. No push. Claude runs the gate, installs on port 7700 and runs the proof.

## 7. Proof (run by Claude on the real server with the session's Synapse MCP tools; automation Chrome, pages in the session scratchpad `r329/`)

| call | pass when |
|---|---|
| `act {verb:"tap", selector:"#b"}` on `popup-touch-stay.html` | `ok: true`; the page's `#out` reads `touchstart touchend`; no unresolved-owner text |
| `act tap` on `popup-touch-close.html` (regression #341) | `ok: true`, `release_not_sent_target_closed` present, `dispatched_events` without `touchEnd`, popup gone |
| the facade call that reaches the stroke (the builder names it) on a page that closes on mouse-down | `ok: true`, the closed-target field present, popup gone, no unresolved-owner text |
| the same stroke call on a page that stays | ok as today |
| `act click` on `popup-mousedown-close.html` (regression) | ok as before, `dispatched_events` only what was sent |
| bridge tab: `browser_debugger emulate set geolocation` | the answer shows `emulated_by_shim: true` and the real permission state; page readback still gets the coordinates |

## 8. Report the builder gives back

Model id; per part: what changed, where, the commit hash; the exact facade call that reaches `dispatch_cdp_mouse_stroke_raw`; the answer fields a caller reads for each case; whether the extension changed; what could not be checked without compiling or a live Chrome.
