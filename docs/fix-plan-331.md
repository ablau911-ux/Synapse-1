# Fix plan: emulate reset without domains fails on a domain that was never set (#331)

Written 2026-11-25 23:34 IST by Claude. Branch `fix/emulate-reset`, cut from main `f42f2616`.

## 1. Why

Found by the live sweep of the browser tools and reproduced by Claude on the real server (main `f42f2616` build, bridge extension from main, Chrome 158). The answers are quoted in Gitea issue #331. The owner approved autonomous testing and bug fixing until 2026-11-26 06:00.

## 2. Upstream

main equals origin/main plus our commits (`git rev-list --left-right --count main...origin/main` was `461 0` on 2026-11-25 20:39, before tonight's merge). Upstream has the same emulate code and no fix.

## 3. Facts (observed)

- `browser_debugger {operation:"emulate", emulate:{operation:"set", viewport:{width:500,height:900}}}` on an own bridge tab (profile `browser_debugger`) answers ok; readback `500x900`, `device_pixel_ratio: 1`.
- `browser_debugger {operation:"emulate", emulate:{operation:"reset"}}` (no `domains`) answers an error: `browser_emulate locale normal Chrome bridge emulation failed: chrome.debugger localeEmulation reset failed for tab <id>: Emulation.setLocaleOverride: {"code":-32000,"message":"Locale override is only available to trusted clients"}`. `browser_debugger evaluate` right after shows the viewport back at `1048x714 dpr=1.25`: the viewport part was done, the call still failed.
- `emulate:{operation:"reset", domains:["viewport"]}` answers ok (run twice).
- The accepted fields of the emulate spec are `cdp_target_id`, `window_hwnd`, `operation`, `domains`, `viewport`, `device`, `geolocation`, `locale`, `media`, `network` (from the tool's own error text).

## 4. The rule to build

A reset without `domains` resets what can be reset and does not fail because of a domain that Chrome refuses for an extension debugger when that domain was never set on this tab.

The builder first reads how `reset` chooses its domains when `domains` is omitted and whether the bridge or the daemon keeps a per-tab record of which emulation domains were set. Then, in this order of preference:

1. If a per-tab record of set domains exists (or the state can be read back cheaply): a reset without `domains` resets exactly the domains that are set on that tab. A tab with nothing set answers ok with an empty result list.
2. If no such record exists: keep "all domains", but a domain whose reset Chrome refuses with "only available to trusted clients" is reported in that domain's result entry as not available on a normal bridge tab (its own status field, the Chrome text kept), and the call as a whole answers ok when every other domain was reset. Any other failure of any domain stays an error of the whole call, as today.

Not changed: `set` behaviour; a `reset` with explicit `domains` that names a refused domain is still an error (the caller asked for exactly that domain); the result shape of domains that succeed.

One owner for the rule: the place that expands "no domains" into a domain list. No new option, no new configuration.

## 5. Files (the builder confirms by reading)

Search for the text `Chrome bridge emulation failed` and for `localeEmulation` under `crates/synapse-mcp/src/` and in `extensions/synapse-chrome-debugger/service_worker.js`.

## 6. Order, commits, build rule

One commit for the fix. No Rust tests (Gate 0). The builder runs no cargo command except `cargo fmt --all`. Commit as git user `claude` with `git commit --only -m "..." -- <paths>`. No push. Claude runs the gate (one build, 4 jobs, Idle priority) if Rust changed.

## 7. Proof (run by Claude on the real server with the session's Synapse MCP tools)

| call | pass when |
|---|---|
| set viewport 500x900, then `reset` without domains | answer ok; `evaluate` shows the baseline size and `dpr` again |
| `reset` without domains on a tab with nothing set | answer ok |
| `reset` with `domains:["viewport"]` after a set | answer ok (unchanged) |
| `reset` with `domains:["locale"]` on a tab where locale was never set | still an error that names the locale domain (unchanged), or ok if rule 1 makes "nothing set" a no-op: the builder says which and why |
| regression: `set` viewport + `evaluate` | `500x900` |

## 8. Report the builder gives back

Model id; which of the two rules applies and the lines that decided it; files and functions changed; commit hash; what could not be checked without compiling or without a live Chrome.
