# Fix plan: a raw-CDP key press that closes its own target is a delivered press, not a failure (#329)

Written 2026-11-26 02:30 IST by Claude. Branch `fix/raw-cdp-batch3`, worktree `C:\Users\Adam\Synapse-b3` (cut from `fix/cdp-init-script`, which holds the #335 fix).

## 1. Why

Gitea issue #329, reproduced live by Claude (comment 3542): `act {verb:"key", key:"Enter"}` on a raw-CDP popup whose key-down handler closes the window answers `ok:false`, code `A11Y_CDP_AXTREE_FAILED`, "one or more keyUp acknowledgements were not proven; primary_error=none; release_error=… CDP input dispatch failed: send failed because receiver is gone; unresolved_raw_cdp_input_owner_count=1". The key was delivered and did its job. The PostMessage tier got this rule in #325. The owner approved autonomous testing and bug fixing until 2026-11-26 06:00.

## 2. Upstream

Our main holds everything origin/main has; upstream has the same code and no fix. The rule is the one of #325 (merged, main `f42f2616`).

## 3. Facts (read by an Opus 4.8 investigation at `c37b3586`; line numbers from that tree, they move a little after the #335 merge)

- `cdp_press_key_sequence` (`crates/synapse-a11y/src/cdp_action.rs` 1455) → `run_owned_cdp_input` → `with_target_page` (6737) → `dispatch_key_sequence_on_page` (6347). Key-downs 6383-6398 (`delivered = true` on acknowledgement); key-ups 6405-6415: a failed `page.execute(KeyUp)` is pushed to `release_failures` (6412). A non-empty `release_failures` (6417-6425) skips `resolve_persisted_cdp_mutation_owner` and calls `record_unresolved_input_release_detail` (5619): `UNRESOLVED_RAW_CDP_INPUT_OWNERS.fetch_add(1)` and the error text. The persisted owner (`persist_cdp_mutation_owner`, 6367) stays unresolved.
- `act_press_cdp_target` (`crates/synapse-mcp/src/m2/press.rs` 93-133) maps any `Err` (111-122) to `A11Y_CDP_AXTREE_FAILED`, `ok:false`.
- A sure target check exists: `cdp_target_present_inventory(endpoint, target_id)` (`cdp_action.rs` 2553) → `cdp::cdp_list_targets` (323, `Target.getTargets`). `close_cdp_target_with_absence_readback` (2558) shows the present/absent idiom. `endpoint` and `target_id` are in scope of the closure in `cdp_press_key_sequence` (1475-1480).
- The #325 model: `PostKeySequenceOutcome::{Readback, TargetWindowClosed}` (`m2/press/postmessage.rs` 23-33), `PostKeyDelivery`, `ActPressResponse.key_up_not_posted_window_closed` (`m2/press/schema.rs` 123, doc comment 117-122 speaks of WM_KEYUP), `postcondition_target_window_closed(tool, delivered_action, detail)` (`m2/postcondition.rs` 63).

## 4. The rule to build

When a key-down was delivered and a key-up then fails, the code asks Chrome whether the target still exists.
- Target absent: the press is delivered. The persisted owner is resolved (no unresolved-owner count), and the function returns an outcome that says "key-up not sent, target closed" with a short detail (target id, the key-up error). `act_press_cdp_target` answers `ok: true` with the `postcondition_target_window_closed` postcondition and `key_up_not_posted_window_closed: true`. The field's doc comment is widened to cover a CDP target (one field for one fact: "the key-up was not needed because its target no longer exists").
- Target present (a connection problem while the target lives), or the check itself fails: today's failure stays exactly as it is. A really held key must never be reported as released.
- A press whose key-down was not delivered is not touched.

Shape: `dispatch_key_sequence_on_page` and `cdp_press_key_sequence` return a small outcome enum instead of `()` (mirror `PostKeySequenceOutcome`; no unread fields or variants: clippy rejects them). Other callers of `cdp_press_key_sequence` are adapted (find them all).

Not in this fix (listed in #329 by the investigation, same defect, own issue later if wanted): mouse click, mouse stroke, touch tap release after a self-closing target.

## 5. Files

`crates/synapse-a11y/src/cdp_action.rs` (the two functions, the outcome type), `crates/synapse-a11y/src/lib.rs` if the type must be exported, `crates/synapse-mcp/src/m2/press.rs` (`act_press_cdp_target`), `crates/synapse-mcp/src/m2/press/schema.rs` (doc comment only), other callers of `cdp_press_key_sequence`.

This builder owns ONLY these files. Another builder works at the same time in this worktree on `crates/synapse-a11y/src/cdp_emulation.rs` and `crates/synapse-mcp/src/server/browser_emulate.rs` (#337): do not edit, stage or commit those.

## 6. Order, commits, build rule

One commit. No Rust tests (Gate 0). No cargo command except `cargo fmt --all` right before the commit. Commit only your own paths with `git commit --only -m "… (#329)" -- <paths>`. No push. Claude runs the gate, installs on port 7700 and runs the proof.

## 7. Proof (run by Claude on the real server with the session's Synapse MCP tools; automation Chrome with `--disable-popup-blocking`, popups from `window.open`)

| call | pass when |
|---|---|
| `act {verb:"key", key:"Enter"}` on a popup that closes itself on key-down | `ok: true`, `key_up_not_posted_window_closed: true`, postcondition says the target closed; the popup is gone; no `unresolved_raw_cdp_input_owner_count` text |
| `act {verb:"key", key:"a"}` on a popup that stays | `ok: true`, `observed_delta`; page shows `keydown:a keyup:a`, value `a` (as today) |
| regression: `act key` on Finance Manager (PostMessage tier) | ok as today |

## 8. Report the builder gives back

Model id; the outcome type and where it lives; the decision in `dispatch_key_sequence_on_page` in three sentences; every caller adapted; the answer fields for the closed-target case; commit hash; what could not be checked without compiling or a live Chrome.
