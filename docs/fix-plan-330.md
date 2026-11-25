# Fix plan: browser_nav back/forward fails after navigations without user activation (#330)

Written 2026-11-25 23:58 IST by Claude. Branch `fix/nav-history`, cut from main `f57fd7e4`.

## 1. Why

Found in the live Chrome check and confirmed by the live sweep (Gitea issue #330, with the exact answers). The owner approved autonomous testing and bug fixing until 2026-11-26 06:00.

## 2. Upstream

Upstream (origin/main) has the same code; no upstream solution (main held everything upstream had on 2026-11-25 20:39).

## 3. Facts (observed on the real server, Chrome 158, normal bridge tab)

- Own bridge tab. `browser_nav navigate` to page A, `browser_nav navigate` to page B, `browser_nav back` → `cdp_navigate_tab Chrome debugger Page command/readback failed: chrome.tabs.goBack(<tab>) failed: Cannot find a next page in history.; before url="…" title="…" status="complete"`.
- `browser_debugger evaluate` on that tab: `history.length` = 4. Same failure with the tab active.
- The sweep saw the same after `act click` navigations whose answer had `native_click_method: true` (2 of 2).
- Earlier the same day `back` worked after an `act click` whose answer showed `Input.dispatchMouseEvent` (a real input event).
- Reading (inference): Chrome marks history entries that got no user activation as skippable for the Back/Forward buttons, and `chrome.tabs.goBack` / `goForward` follow the same rule.

## 4. Decision

Claude, owner away, upstream has none: the browser tools follow Playwright's contract (the schemas say "Playwright-style"), and Playwright's `page.goBack()` / `goForward()` move one entry in the session history whether or not the entry had user activation. So `browser_nav back` goes to the previous history entry and `forward` to the next one.

## 5. The rule to build

`back` moves the tab to the entry before the current one in its session history; `forward` to the entry after it. When there is no such entry, the answer is a clear error that says so in Synapse's words (not Chrome's "Cannot find a next page in history").

How (to be confirmed by experiment on the live Chrome before coding, bench first):
- Keep `chrome.tabs.goBack` / `goForward` as the first try (no debugger needed when Chrome allows it).
- When Chrome refuses with its "Cannot find … page in history" error: read the session history through the debugger lane the extension already uses for other commands (`Page.getNavigationHistory`: `currentIndex`, `entries`), and when an earlier/later entry exists go to it with `Page.navigateToHistoryEntry`. Use the existing attach/detach helpers and their ownership rules (a session this command attached is detached again; a borrowed one is left).
- Then the existing load wait and readback run as for any navigation, including the #327 rule (an entry that is a Chrome error page makes the command fail).
- Measure first: does `Page.navigateToHistoryEntry` reach a skippable entry on Chrome 158 from an extension debugger; what does the tab look like afterwards (`location.href`, `history.length`); does a debugger attach change the OS foreground (it must not).

Not changed: `navigate`, `reload`; `back`/`forward` when Chrome's own call works; no new permission in `manifest.json`; no new option.

## 6. Files (confirmed by reading)

`extensions/synapse-chrome-debugger/service_worker.js`: `handleNavigateTab` (the back/forward branch) and the helpers it uses. Rust only if the daemon maps the new "no history entry" error text; prefer reusing the existing error code so no Rust change is needed.

## 7. Order, commits, build rule

Bench first: experiments on the deployed copy, then the worktree file, deploy, reload. One commit for the fix. No cargo commands. Commit as git user `claude` with `git commit --only -m "..." -- <paths>`. No push.

## 8. Proof (Finance Manager stays the foreground window; own tabs; local Gitea pages)

| call | pass when |
|---|---|
| `navigate` A, `navigate` B, `back` | answer ok; a second read (`browser_debugger evaluate` of `location.pathname`) shows A |
| then `forward` | answer ok; second read shows B |
| `back` on a tab with one entry | error that says there is no earlier entry |
| `forward` at the newest entry | error that says there is no later entry |
| `act click` on a link (real input), then `back` | ok (Chrome's own call), second read shows the first page |
| `navigate` A, `navigate` to a name that does not resolve (fails, #327), `navigate` B, `back` | goes to the entry before B; if that entry is the error page the command fails with the #327 error; report what happens |
| OS foreground before and after every call | unchanged |

## 9. Report

Model id; the measurements of section 5; functions changed and the rule in one sentence; commit hash; the proof table as observed; what is not verified; the state of the PC.
