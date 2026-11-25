# Fix plan: input on a hidden raw-CDP page is dropped and reported as delivered and verified (#334)

Written 2026-11-26 01:10 IST by Claude. Branch `fix/cdp-hidden-input`, cut from main `85d5ef98`.

## 1. Why

Found by the raw-CDP sweep (#333 item A) and reproduced by Claude on the real server (Gitea issue #334 holds the calls and answers). The owner approved autonomous testing and bug fixing until 2026-11-26 06:00.

## 2. Upstream

Our main holds everything origin/main has (checked 2026-11-25 20:39). Upstream launches the automation Chrome with the same flags and reports the click the same way. No upstream solution.

## 3. Facts (observed on the real server, Chrome 158)

- `process launch` of Chrome (default) builds this command line: `chrome.exe --remote-debugging-port=0 --user-data-dir=<temp> --silent-debugger-extension-api --disable-extensions --no-first-run --no-default-browser-check <caller args>`.
- With that launch, the window opened behind the foreground app (Finance Manager). After binding the tab as a `cdp` target: `act {action:{verb:"click", selector:"a[href='/explore/users']"}}` answered `status: ok`, `delivery_state: "delivered_and_verified"`, `delegated_tool: synapse_a11y.cdp_click_node`, `method: Input.dispatchMouseEvent(mouseMoved,mousePressed,mouseReleased)`, `real_trusted_input: true`. Second read (`browser_debugger evaluate`): `/explore/repos | vis=hidden`. The click was dropped.
- Same tab after bringing the window to the front: `vis=visible`, the same click navigated to `/explore/users`.
- Launch with two more caller args, `--disable-backgrounding-occluded-windows --disable-features=CalculateNativeWinOcclusion`, window again behind Finance Manager (same bounds): second read `vis=visible` while covered; the same click navigated to `/explore/users`; the OS foreground window stayed Finance Manager.

## 4. The two rules to build

### Rule A: an automation Chrome that Synapse launches keeps its pages live when the window is covered

The place that builds the automation command line (the one that adds `--remote-debugging-port=0 … --no-default-browser-check`) also adds `--disable-backgrounding-occluded-windows` and `--disable-features=CalculateNativeWinOcclusion`. One owner: that flag list. If the caller's own args already contain a `--disable-features=` value, the two must be merged into one `--disable-features=` argument (Chrome uses only the last one), not silently overridden; if the code has no merging today, add the smallest correct merge in that same place.

### Rule B: trusted input is never reported delivered when the page is hidden

Before a raw-CDP trusted input dispatch (the mouse path `cdp_click_node`, and the key and stroke paths if they dispatch `Input.dispatch*Event` the same way; the builder lists them from the code), read the page's `document.visibilityState` through the CDP session that is already open. When it is `hidden`: do not dispatch; return an error (existing code family; pick the one the action path uses for "cannot act on the target now") whose message says the page is hidden (covered window or inactive tab), that Chrome drops input sent to a hidden page, and what to do (bring the tab to the front of its window; an automation Chrome launched through `process launch` stays live when covered). No `delivered_and_verified`, no partial dispatch.

Also establish what `delivered_and_verified` means on the click path today: what is checked after the dispatch. If nothing is checked, the answer must not say verified: use the state name the code already has for "delivered, effect not checked", and say in the report which names exist. Do not build a new verification mechanism in this fix.

Not changed: the normal bridge path (`chrome-tab:` targets); answers of dispatches on visible pages, apart from the state name if "verified" was unfounded; no new option.

## 5. Files (the builder confirms by reading)

- The automation launch flags: search for `--no-default-browser-check` and `--silent-debugger-extension-api` under `crates/`.
- The click path: `cdp_click_node` in `crates/synapse-a11y/src/cdp_action.rs` and its caller in `crates/synapse-mcp/src/`; the key path `dispatch_key_sequence_on_page`; the stroke path.

## 6. Order, commits, build rule

Two commits: Rule A, then Rule B. No Rust tests (Gate 0). The builder runs no cargo command except `cargo fmt --all`. Commit as git user `claude` with `git commit --only -m "..." -- <paths>`. No push. Claude runs the gate (one build, 4 jobs, Idle priority), installs on port 7700 and runs the proof.

## 7. Proof (run by Claude on the real server with the session's Synapse MCP tools)

| call | pass when |
|---|---|
| `process launch` Chrome (default, no extra args) | the answer's `command_line` holds both new flags |
| window behind Finance Manager, tab bound as `cdp` target: `browser_debugger evaluate` visibility | `visible` |
| `act click` by selector on that covered tab | answer ok; second read shows the new page; foreground window unchanged |
| a second, inactive tab in that window (opened with `window.open` from the first, or any way the tools allow), bound as target: `act click` | error that says the page is hidden and what to do; second read: nothing changed |
| `process launch` with caller arg `--disable-features=Translate` | `command_line` has one `--disable-features=` with both values |
| regression: `act click` + `act key` on Finance Manager | as before |

## 8. Report the builder gives back

Model id; where the flags live and how merging is done; the list of raw-CDP input paths and which got the hidden check; what `delivered_and_verified` meant on the click path and what the answer says now; commit hashes; what could not be checked without compiling or a live Chrome.
