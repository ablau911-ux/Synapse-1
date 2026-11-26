# Fix plan: a raw-CDP click, stroke or touch tap that closes its own target is a delivered press, not a failure (#341)

Written 2026-11-26 05:02 IST by Claude. Branch `fix/raw-cdp-release-closed-target`, cut from main `ee811ded`, worktree `C:\Users\Adam\Synapse-stress`.

## 1. Why

Gitea issue #341, click path reproduced live by Claude (comment 3643): `act {verb:"click", selector:"#b"}` on a raw-CDP popup whose `mousedown` handler closes the window answers, after about 40 s, `ok:false`, code `A11Y_CDP_AXTREE_FAILED`, "mouse click release acknowledgement was not proven; primary_error=none; release_error=… CDP input dispatch failed: Request timed out.; unresolved_raw_cdp_input_owner_count=1". The press was delivered and did its job. The key press got this rule in #329 (merged, main `59af2a1d`), the PostMessage key in #325. The owner approved autonomous testing and bug fixing until 2026-11-26 06:00.

## 2. Upstream

Our main holds everything origin/main has; upstream has the same code and no fix. The rule is the one of #329.

## 3. Facts

- Read by the #329 investigation (tree c37b3586; line numbers moved since): single click `dispatch_mouse_click_on_page` (`crates/synapse-a11y/src/cdp_action.rs`, then 6178; release failure then at 6223); raw mouse stroke `dispatch_cdp_mouse_stroke_raw` (then 5420; release failure 5532-5534, `combine_raw_input_failures` 5582); touch tap `dispatch_touch_tap_on_page` (then 6327-6331, "touchEnd acknowledgement was not proven"); `cdp_type_node` inherits the click through its embedded click.
- The #329 model, in main now: `CdpKeySequenceOutcome::{Delivered, TargetClosed{detail}}` above `cdp_press_key_sequence`; in `dispatch_key_sequence_on_page` a failed key-up after a delivered key-down asks `cdp_target_present_inventory(endpoint, target_id)` (now a documented `pub` fn); a sure absence resolves the persisted owner (`resolve_persisted_cdp_mutation_owner`) and returns `TargetClosed`; a live target, an inconclusive inventory or an undelivered press keeps the old failure through `record_unresolved_input_release_detail`. On the MCP side `act_press_cdp_target` (`crates/synapse-mcp/src/m2/press.rs`) answers ok with `postcondition_target_window_closed` and `key_up_not_posted_window_closed`, and `act_press_cdp_background_target` (`crates/synapse-mcp/src/server/m2_tools/mod.rs`) skips the verify readback.
- The click answer today comes from the `synapse_a11y.cdp_click_node` lane (`target_act` in synapse-mcp); `target_act_delivery_state_from_result` gives `delivered_unverified` for it (#334).
- Live: for the click the release error is "Request timed out" (the mouseReleased command waits for a target that is gone), and the call takes about 40 s.

## 4. The rule to build

When a press (mousePressed, touchStart) was delivered and its release (mouseReleased, touchEnd) then fails, the code asks Chrome whether the target still exists.
- Target absent: the action is delivered. The persisted owner is resolved (no unresolved-owner count), and the dispatch returns an outcome that says "release not sent, target closed" with a short detail (target id, the release error). The tool answers ok and says so in its result: reuse the answer's existing shape and add one field that states the fact, named for what it is on this lane (follow how `key_up_not_posted_window_closed` was added for the press; one field for one fact per lane; if the click result type already has a place for a note, use it instead of a new field).
- Target present, or the check fails: today's failure stays exactly as it is. A really held button must never be reported as released.
- A press that was not delivered is not touched.
- Time: when the target is already gone, the release must not wait for the full command timeout if the code can know earlier. Smallest correct form: before sending the release, or as soon as its send fails or stalls, ask the inventory; do not add a new timeout constant unless one existing constant fits. If no clean way exists tonight, keep the wait and say so; the answer must still be right.

One shared helper for "release failed after a delivered press: is the target gone?" used by the key path (already there), click, stroke and touch, instead of three copies. `cdp_type_node` gets the rule through its embedded click; check what it must return.

## 5. Files (the builder confirms by reading)

`crates/synapse-a11y/src/cdp_action.rs` (the three dispatch functions, the shared helper, outcome types), the synapse-mcp callers of the click / stroke / touch functions (`target_act` click lane and wherever stroke and tap are answered), their result types.

## 6. Order, commits, build rule

One commit for click (+ type), one for stroke and touch (or one commit if they are one change). No Rust tests (Gate 0). No cargo command except `cargo fmt --all` right before a commit. Commit as git user `claude` with `git commit --only -m "… (#341)" -- <paths>`. No push. Claude runs the gate, installs on port 7700 and runs the proof.

## 7. Proof (run by Claude on the real server with the session's Synapse MCP tools; automation Chrome with `--disable-popup-blocking`, popups from `window.open`; pages in the session scratchpad `r329/`)

| call | pass when |
|---|---|
| `act {verb:"click", selector:"#b"}` on a popup that closes itself on mouse-down | `ok: true`, the result says the target closed after the press; the popup is gone; no `unresolved_raw_cdp_input_owner_count` text |
| `act click` on a link of a page that stays (`a[href='/explore/users']` on local Gitea) | ok as today, the page navigates |
| regression: `act click` on Finance Manager (a harmless control) and `act key` on the self-closing key popup (#329) | ok as today |

Stroke and touch tap have no live row tonight (no page prepared): say so in the report.

## 8. Report the builder gives back

Model id; the shared helper and the outcome type(s); per path (click, type, stroke, touch) what it returns now for "press delivered, target gone"; the answer field that tells the caller; what happens to the 40 s wait; commit hashes; what could not be checked without compiling or a live Chrome.
