# Fix plan: four bugs found in the live Chrome check (#324, #325, #326, #327)

Written 2026-11-25 20:42 IST by Claude. Branch `fix/chrome-live-batch`, cut from main `c59e0bbe`.

## 1. Why

The live check of the Chrome bridge group (G10) on the real server (port 7700, main `c59e0bbe` build) found four defects. Each one was seen in a real tool answer; the answers are quoted in the Gitea issues. The owner approved autonomous testing and bug fixing until 06:00 (message of 2026-11-25, about 20:33 IST).

## 2. Upstream

Rule: read origin's (ChrisRoyse/Synapse) solution first. `git fetch origin` on 2026-11-25 20:39: `git rev-list --left-right --count main...origin/main` = `461 0`. Our main holds everything upstream has, and `origin/main:extensions/synapse-chrome-debugger/service_worker.js` has the same four placements in `moveTabToRenderWindow`. Upstream has no solution for any of the four. The fixes below are ours.

## 3. The four fixes

Each fix changes one rule in the one place that owns it. No new features, no new settings.

### #326 `act focus_window` reports `A11Y_NO_FOREGROUND` although the window became foreground

- Seen: `act_focus_window set_foreground_window failed: no foreground window is available: GetForegroundWindow returned null`, `delivery_state: failed_before_delivery`; six seconds later `target list` showed the requested window as foreground.
- Fact: the text `GetForegroundWindow returned null` is built in `crates/synapse-a11y/src/platform/windows/window.rs` (lines 46 and 56 on `c59e0bbe`). Windows returns a null foreground window for a short time while it switches the foreground window.
- Rule to fix: the foreground readback that follows a foreground switch requested by `act_focus_window` must not take one null read as the final answer. It reads again for a short bounded time (the existing stability wait of the focus verification is the model; reuse its constants if there are any) and only then reports `A11Y_NO_FOREGROUND`.
- Where: the function `act_focus_window` uses to set and verify the foreground window. Find it from the error text `set_foreground_window failed`. Do not change the general `foreground_window()` reader used by perception: a null foreground on a locked desktop is a real state there.
- Not changed: when the foreground window stays null for the whole bounded time, the answer is the same error as today.

### #325 `act key` reports `ACTION_BACKEND_UNAVAILABLE` when the key-down closes the window that receives the key-up

- Seen: `PostMessageW act_press keyboard message 0x101 failed for hwnd 0x1804a4: Invalid window handle. (0x80070578)`, `status: verify_needed`, `ok: false`; the key (Enter in Chrome's address bar) did take effect.
- Fact: the text is built in `crates/synapse-mcp/src/m2/press/postmessage.rs` (line 672 on `c59e0bbe`). `0x101` is `WM_KEYUP`.
- Rule to fix: when the key-down message of a key was posted to a window and the matching key-up cannot be posted because that same window no longer exists (`ERROR_INVALID_WINDOW_HANDLE`, and `IsWindow` is false), the key press is delivered. The answer must not be a backend failure. It records, in the existing result structure, that the key-up was not posted because the key-down closed its window. Keys that are still held down by an earlier key-down in the same chord (modifiers) need no key-up in a destroyed window, so nothing else has to be released; check this in the code and say so in the report.
- Not changed: a failed key-down, a failed key-up to a window that still exists, and any failure with another error code stay errors.

### #324 `browser_capture screenshot` of a hidden tab fails with "Invalid value for bounds"

- Seen: `browser_screenshot background Chrome bridge capture failed: Invalid value for bounds. Bounds must be at least 50% within visible screen space.` Chrome window left 13, width 1325 on a 1920-wide single display.
- Fact: `moveTabToRenderWindow` in `extensions/synapse-chrome-debugger/service_worker.js` builds four placements beside the origin window (right, left, below, above). The first one is passed to `chrome.windows.create` before the loop; Chrome throws for bounds that are less than 50% on screen, so the other placements are never tried. With one display and a wide Chrome window all four are mostly off screen.
- Rule to fix: every placement the function tries lies inside the work area of the display that holds the tab's window, and a placement Chrome rejects is recorded and skipped, not fatal.
  - Read the work area from the page of the tab itself (`screen.availLeft`, `screen.availTop`, `screen.availWidth`, `screen.availHeight`) with the existing `quarantineGuardedExecuteScript` helper, the same way `pageVisibilityState` reads `document.visibilityState`. The manifest has no `system.display` permission and gets none.
  - Keep the "beside the origin window" placements first (they are right when a second display is there), but only those that Chrome accepts. Then add placements inside the work area: the window size limited to the work area, placed at the four corners of the work area.
  - The window is created with the first placement that Chrome accepts; `chrome.windows.create` and `chrome.windows.update` are wrapped so a rejected placement is pushed to `record.placements` with its error text and the loop goes on.
  - If the work area cannot be read (script fails), use the beside-origin placements only, as today, but still skip rejected ones.
- Not changed: `focused: false` everywhere; the tab is returned to its window, index and active state by `restoreTabFromRenderWindow`; when no placement gives a visible page the function restores the tab and throws the existing "found no uncovered display spot" error, which now lists every placement with its result.
- The extension's declared build id / sha: the daemon compares `extension_declared_build_sha256` with an expected value (seen in `/health`). Find how that value is produced (search `declared_build_sha256` and `EXTENSION_BUILD_ID` or similar in `crates/synapse-mcp/src/chrome_debugger_bridge/` and in `scripts/install-synapse-chrome-debugger.ps1`) and update whatever must change together with `service_worker.js`, exactly the way earlier commits that changed `service_worker.js` did it (look at `git show --stat 6d9f8b8e 618cea9d`). Report what you changed and why.

### #327 `browser_nav navigate` reports a normal result for a page that failed to load

- Seen: `navigate` to a name that does not resolve answered `ready_state: "complete"`, `after_url` = the requested URL, no error; the tab showed Chrome's error page (`location.href` = `chrome-error://chromewebdata/`, `DNS_PROBE_FINISHED_NXDOMAIN`).
- Decision (Claude, owner away, upstream has none): Synapse's browser tools follow Playwright's contract (the schemas say "Playwright-style"), and Playwright's `page.goto` fails when the main frame navigation fails. So `navigate`, `reload`, `back` and `forward` return an error when the main frame of the tab ends on a Chrome error page. The error uses the existing browser/bridge error code that fits best (look at the codes the navigation path already returns; do not invent a new family if one fits) and its message names the requested URL and Chrome's network error (for example `net::ERR_NAME_NOT_RESOLVED`) when Chrome gives it.
- How to detect: the extension has the `webNavigation` permission. In the bridge's navigate command, listen to `chrome.webNavigation.onErrorOccurred` for the tab's main frame (`frameId === 0`) from before the navigation starts until the load readback, and remove the listener afterwards in every path. `ERR_ABORTED` caused by a download or by a newer navigation is not a failed page; keep today's `is_download` behaviour.
- The daemon side (Rust) maps the bridge's new failure to the tool error. Keep the tab owned and bound as it is; the error text says the tab now shows Chrome's error page.
- Not changed: a page that loads (any HTTP status) answers as today.

## 4. Files (expected; the builder confirms by reading)

| fix | files |
|---|---|
| #326 | the focus/verify function reached from `act_focus_window` (crate `synapse-action` or `synapse-a11y`), found from the text `set_foreground_window failed` |
| #325 | `crates/synapse-mcp/src/m2/press/postmessage.rs` |
| #324 | `extensions/synapse-chrome-debugger/service_worker.js` (+ the build id/sha file if one must change) |
| #327 | `extensions/synapse-chrome-debugger/service_worker.js`, the Rust navigation path under `crates/synapse-mcp/src/` that reads the bridge's navigate answer |

## 5. Order and commits

One commit per fix, in this order: #326, #325, #324, #327. Formatting, if `cargo fmt` changes anything else, in its own commit. The repository has no Rust tests (Gate 0); do not add any. Commit as git user `claude`. Do not push; Claude pushes after the gate.

## 6. Build rule

Parallel cargo builds froze this PC. The builder runs no `cargo build`, no `cargo clippy`, no `cargo check`. It may run `cargo fmt --all` (no compile). Claude runs the one gate build afterwards (`CARGO_BUILD_JOBS=4`, Idle priority).

## 7. Proof (run by Claude on the real server with the session's Synapse MCP tools)

| fix | call | pass when |
|---|---|---|
| #326 | Chrome in front, target + claim Finance Manager, `act foreground focus_window`; then the same back to Chrome; 3 switches each way | every answer `status: ok` and names the requested window; `target list` agrees |
| #325 | Chrome address bar: `set_field` a local URL, `act foreground key Enter` | `status: ok`; the page navigated (window title read back); the answer says the key-up was not posted because its window closed, if that happened |
| #324 | hidden background tab on a local page, Finance Manager in front: `browser_capture screenshot` | a picture of that page; `render_window.placements` lists the tried spots; `browser_tabs list` shows the tab back at its index, not active; foreground window before and after = Finance Manager |
| #327 | `browser_nav navigate` to `https://name-that-does-not-resolve.invalid/` | an error that names the URL and the network error; then `navigate` to the local page answers normally |

Regression checks after the build: `act key` and `act focus_window` on Finance Manager, `browser_nav navigate` + `browser_capture screenshot` on an active tab.

## 8. Report the builder gives back

For each fix: files and functions changed, the rule in one sentence, anything in this plan that turned out wrong or could not be done (deviation), and what was not checked.
