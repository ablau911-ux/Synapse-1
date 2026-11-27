# Synapse 40-Tool Surface Migration

Updated: 2026-11-27 (operation lists re-synced with `FACADE_TOOL_CONTRACTS`, #288)

Parent issue: #1374
Task issue: #1394

## Current Surface

The default production MCP surface is facade-first:

- Public registry limit: 40 tools maximum.
- Public registry source of truth: `crates/synapse-mcp/src/server/tool_profiles.rs` `PUBLIC_TOOL_NAMES`.
- Normal-agent visible source of truth: `NORMAL_ALLOWED_EXACT`.
- Facade contract source of truth: `FACADE_TOOL_CONTRACTS`.
- Live profile row source of truth: `CF_SESSIONS mcp/tool-profile/v1/<session_id>`.

The current normal-agent profile exposes all 40 stable public facade names. Every scoped production HTTP profile advertises the same facade surface so strict/static MCP clients never need a schema refresh to gain a capability route. Profiles gate operation-level authority: debugger-backed `browser_debugger` operations require `profile operation=set profile=browser_debugger confirm_break_glass=true reason=<why debugger authority is required>`, and real-foreground actions use the audited `act operation=foreground` route. Raw implementation tools remain hidden for scoped HTTP profiles. Trusted unscoped stdio intentionally retains the full raw admin surface, and `SYNAPSE_DEBUG_TOOLS=1` explicitly enables the diagnostic implementation surface.

Default normal-agent tools:

`health`, `profile`, `session`, `subscribe`, `observe`, `find`, `read_text`, `screenshot`, `target`, `act`, `shell`, `process`, `browser_tabs`, `browser_nav`, `browser_dom`, `browser_form`, `browser_wait`, `browser_capture`, `browser_storage`, `browser_debugger`, `workspace`, `agent`, `task`, `approval`, `escalation`, `timeline`, `episode`, `routine`, `assist`, `reality`, `verification`, `storage`, `model`, `cost`, `hygiene`, `audit`, `replay`, `privacy`, `setup`, `telemetry`.

Public facade with profile-gated operations:

`browser_debugger`.

## Deprecation Policy

Normal agents use facade tools only. Old implementation names are not default public tools.

35 facades take a strict `operation` enum. Five single-purpose facades — `health`, `subscribe`, `observe`, `find`, `read_text` — have no `operation` parameter at all; their contract carries the `FLAT_FACADE_OPERATION_ENUM` sentinel. Unknown operations fail closed with structured errors. There is no alias fallback from an old tool name to a new facade operation.

Mutating operations must name the physical readback source of truth: file path, RocksDB CF/key, process id, tab id, target id, event cursor, or profile row.

Raw browser debugger capability is explicit: switch to `browser_debugger` (or hold `break_glass`/`full_capability`) before using raw CDP/chrome.debugger operations. Only `evaluate`, `console_messages` and `network` are admitted under every profile, because they act only on a tab the session owns or has bound (`browser_debugger_operation_admissible_on_owned_target` in `crates/synapse-mcp/src/server/browser_facades.rs`).

Raw human OS foreground capability is explicit: acquire the needed foreground/control lease, switch to `break_glass`, give a non-empty reason, and read the post-action source of truth. Normal-agent routes must prefer target-scoped action through `act` and target/session state through `target`.

Removed status means removed from the normal public surface. It does not always mean implementation code was deleted. Maintenance, debugger, break-glass, or full-capability profiles may still retain implementation routes for controlled use.

## Facade Operation Index

Source: `FACADE_TOOL_CONTRACTS` in `crates/synapse-mcp/src/server/tool_profiles.rs`. Nothing checks this table against the code automatically; update it by hand when the contracts change.

| Facade | Operations |
| --- | --- |
| `health` | none (single-purpose, no `operation` parameter) |
| `profile` | `status`, `set`, `grant_reality_write`, `revoke_reality_write` |
| `session` | `list` |
| `subscribe` | none (single-purpose, no `operation` parameter) |
| `observe` | none (single-purpose, no `operation` parameter) |
| `find` | none (single-purpose, no `operation` parameter) |
| `read_text` | none (single-purpose, no `operation` parameter) |
| `screenshot` | `capture`, `gif`, `record`, `record_status`, `record_stop`, `record_extend`, `clip_overview`, `clip_frames`, `clip_frame`, `clip_diff` |
| `target` | `get`, `list`, `set`, `clear`, `claim`, `status`, `adopt`, `release` |
| `act` | `invoke`, `foreground`, `lease_acquire`, `lease_status`, `lease_release` |
| `shell` | `run`, `start`, `status`, `cancel` |
| `process` | `list`, `launch`, `history` |
| `browser_tabs` | `list`, `select`, `activate`, `new`, `close` |
| `browser_nav` | `navigate`, `reload`, `back`, `forward` |
| `browser_dom` | `content`, `locate`, `inspect`, `aria_snapshot` |
| `browser_form` | `set_value`, `fill` |
| `browser_wait` | `for_condition` |
| `browser_capture` | `screenshot`, `downloads` |
| `browser_storage` | `get`, `set`, `clear`, `save_state`, `load_state` |
| `browser_debugger` | `evaluate`, `console_messages`, `reload_bridge`, `pdf`, `file_upload`, `dialog`, `add_init_script`, `add_script_tag`, `add_style_tag`, `network`, `network_har`, `network_overrides`, `route`, `emulate`, `expose_binding`, `drag`, `drop` |
| `workspace` | `get`, `put`, `list`, `subscribe`, `exists`, `delete`, `wait` |
| `agent` | `spawn`, `query`, `send`, `inbox`, `wait`, `broadcast`, `receipts`, `stats`, `template_put`, `template_get`, `template_list`, `template_delete`, `task_started`, `interrupt`, `kill`, `steer`, `pause`, `resume`, `respawn` |
| `task` | `create`, `get`, `update`, `claim`, `cancel`, `list`, `next`, `reconcile`, `dispatch_once` |
| `approval` | `request`, `list`, `decide`, `gate`, `ask_operator` |
| `escalation` | `config_get`, `config_set`, `list`, `ack` |
| `timeline` | `get`, `search`, `stats` |
| `episode` | `list`, `get` |
| `routine` | `mine`, `list`, `inspect`, `update`, `feedback`, `label`, `automate`, `armed_tick` |
| `assist` | `intent`, `detect`, `suggestion_tick`, `suggestion_list`, `suggestion_accept` |
| `reality` | `baseline`, `delta`, `audit` |
| `verification` | `inbox`, `poll`, `audit`, `bind`, `sources` |
| `storage` | `inspect`, `summary`, `gc_once` |
| `model` | `list`, `status`, `probe`, `register`, `update`, `remove` |
| `cost` | `summarize`, `price_list`, `price_put`, `price_delete` |
| `hygiene` | `scan_text`, `scan_storage`, `flags`, `report` |
| `audit` | `command_query`, `lifecycle_events`, `lifecycle_exits`, `profile_intelligence`, `export_bundle` |
| `replay` | `record`, `demo_status`, `demo_start`, `demo_stop`, `artifact_inspect` |
| `privacy` | `pause`, `resume`, `exclusions`, `redact`, `purge` |
| `setup` | `status`, `doctor`, `repair` |
| `telemetry` | `status` |

## Common Workflow Examples

Daemon/profile check:

```text
tool: health
args: {}

tool: profile
args: { "operation": "status" }
```

Targeted action:

```text
tool: target
args: { "operation": "list", "title_contains": "GameEditor" }

tool: target
args: { "operation": "set", "target": { "kind": "window", "window_hwnd": 123456 } }

tool: act
args: { "operation": "invoke", "action": { "verb": "click", "element_id": "known-element" } }
```

### Mouse strokes and drags (#341)

Use public `act` with `action.verb=stroke` and nested `action.stroke` parameters. The facade delegates to the existing `act_stroke` implementation, including its validation, target-claim checks, audit, operator-panic checks, and lane guards. A bound session target is required; an absent target never selects the human foreground implicitly. `act_stroke` itself remains hidden under every scoped HTTP profile, and the public surface remains 40 tools.

On a bound raw-CDP target, an explicit `stroke.path` with `backend=auto` or `software` dispatches background `Input.dispatchMouseEvent` samples under `normal_agent`, without a foreground input lease or break_glass. Path coordinates are viewport CSS pixels. Omit `stroke.button` for movement; set it to `left`, `right`, or `middle` for a drag. The raw-CDP path does not support modifiers or `verify_delta=true` and does not fall back to the real cursor if the target or endpoint is unavailable. Read the target DOM/canvas separately to verify the effect. The response reports `delegated_tool=act_stroke`, `backend_tier_used=cdp`, `required_foreground=false`, and `delivery_state=delivered_unverified` when no effect readback ran, including the #341 closed-target outcome.

For a drag from element A to element B, first observe both elements in the bound target and use their current viewport centers in an explicit path. CDP element IDs in `stroke.from`/`stroke.to` are not supported for a drag; the existing element-target form supports pointer aim only, with no button or starting point. The following public `act` arguments assume A's center is `(120, 160)` and B's center is `(420, 160)`:

```json
{
  "operation": "invoke",
  "action": {
    "verb": "stroke",
    "stroke": {
      "path": {
        "kind": "line",
        "from": { "x": 120, "y": 160 },
        "to": { "x": 420, "y": 160 }
      },
      "button": "left",
      "duration_or_speed": { "kind": "duration_ms", "duration_ms": 500 },
      "backend": "auto"
    }
  }
}
```

A pure mouse-move path uses the same route without a button:

```json
{
  "operation": "invoke",
  "action": {
    "verb": "stroke",
    "stroke": {
      "path": {
        "kind": "polyline",
        "points": [
          { "x": 120, "y": 160 },
          { "x": 260, "y": 220 },
          { "x": 420, "y": 160 }
        ]
      },
      "duration_or_speed": { "kind": "duration_ms", "duration_ms": 500 },
      "backend": "auto"
    }
  }
}
```

Native real-cursor strokes use screen coordinates. Real-cursor and hardware delivery require `break_glass`/`full_capability` plus the foreground input lease. Prefer `act operation=foreground` with the same `action` and a non-empty `reason` to acquire the lease, escalate, run the stroke, and restore the profile/lease through the existing audited transaction. Recording and CDP element aim retain their existing handling.

Already-open Chrome tab workflow:

```text
tool: browser_tabs
args: { "operation": "list" }

tool: browser_tabs
args: { "operation": "new", "url": "https://example.com" }

tool: browser_nav
args: { "operation": "navigate", "url": "https://example.com/status" }

tool: browser_dom
args: { "operation": "locate", "selector": "main" }
```

Workspace state with separate readback:

```text
tool: workspace
args: { "operation": "put", "put": { "key": "issue/1394/example", "value": { "state": "written" } } }

tool: workspace
args: { "operation": "get", "get": { "key": "issue/1394/example" } }
```

Shell job with durable status:

```text
tool: shell
args: { "operation": "start", "start": { "command": "pwsh", "args": ["-NoProfile", "-Command", "Get-Date"] } }

tool: shell
args: { "operation": "status", "status": { "job_id": "<returned job id>" } }
```

## Old-Name Coverage Table

Historical source: `git show 8ca2667a^:crates/synapse-mcp/src/server/tool_profiles.rs` `NORMAL_ALLOWED_EXACT`, plus the historical `NORMAL_ALLOWED_PREFIXES` expansion for `agent_template_*` and `task_*`.

The historical audit saw 152 live visible tools. The historical exact-plus-prefix inventory below contains 153 names because prefix expansion records every dynamic route that the profile allowed. Every name below is either routed to a current facade, an explicit advanced profile route, or removed from the normal public surface.

| Old public name(s) | Current route | Status |
| --- | --- | --- |
| `health` | `health` | Kept as public facade. |
| `tool_profile_status`, `tool_profile_set` | `profile operation=status`, `profile operation=set` | Condensed. |
| `session_list`, `session_status` | `session operation=list` | Condensed readback. |
| `session_end` | Advanced maintenance/session lifecycle route | Removed from normal public surface. |
| `profile_list`, `profile_authoring_generate`, `profile_authoring_inspect`, `profile_authoring_list` | Advanced profile-maintenance route | Removed from normal public surface. |
| `subscribe` | `subscribe` (no `operation` parameter) | Kept as public facade. |
| `subscribe_cancel` | Client/session subscription lifecycle | Removed from normal public surface. |
| `observe` | `observe` (no `operation` parameter) | Kept as public facade. |
| `observe_delta` | `reality operation=delta` | Condensed into reality facade. |
| `find` | `find` (no `operation` parameter) | Kept as public facade. |
| `read_text` | `read_text` (no `operation` parameter) | Kept as public facade. |
| `capture_screenshot` | `screenshot operation=capture` | Condensed. |
| `capture_gif` | `screenshot operation=gif` | Condensed. |
| `window_list`, `get_target`, `set_target`, `clear_target` | `target operation=list/get/set/clear` | Condensed. |
| `set_capture_target`, `set_perception_mode` | `target operation=set` plus per-call `observe`/`screenshot` parameters | Removed as normal public global mutators. |
| `target_claim`, `target_claim_status`, `target_claim_adopt`, `target_release` | `target operation=claim/status/adopt/release` | Condensed. |
| `target_act` | `act operation=invoke` | Condensed through action facade. |
| `act_foreground` | `act operation=foreground` | Condensed; explicit reason required; the whole keyed authority transaction remains supervised through exact profile/lease cleanup and final audit after caller cancellation, and the dedicated physical operator-panic epoch supersedes stored agent lease snapshots. |
| `control_lease_acquire`, `control_lease_release`, `control_lease_status` | `act operation=lease_acquire/lease_release/lease_status` (foreground input lease), or `act operation=foreground` for one audited foreground action | Condensed into the action facade; no longer standalone tools. |
| `control_lease_handoff` | `target operation=claim/status/release`, or explicit `break_glass` profile | Removed from normal public surface. |
| `act_run_shell`, `act_run_shell_start`, `act_run_shell_status`, `act_run_shell_cancel` | `shell operation=run/start/status/cancel` | Condensed. |
| `act_launch` | `process operation=launch` | Condensed. |
| `act_spawn_agent` | `agent operation=spawn` | Condensed. |
| `browser_tabs`, `browser_adopt_active_tab`, `cdp_open_tab`, `cdp_close_tab` | `browser_tabs operation=list/select/new/close` | Condensed. |
| `cdp_navigate_tab` | `browser_nav operation=navigate/reload/back/forward` | Condensed. |
| `cdp_activate_tab` | `browser_tabs operation=activate` (Chrome bridge `tabs.update(active=true)` + `tabs.query` readback) | Condensed. |
| `cdp_target_info` | `browser_tabs operation=list` and `target operation=status` | Condensed. |
| `cdp_bridge_reload` | Switch to `browser_debugger`, then `browser_debugger operation=reload_bridge` | Profile-gated advanced route. |
| `browser_content`, `browser_locate`, `browser_inspect`, `browser_aria_snapshot` | `browser_dom operation=content/locate/inspect/aria_snapshot` | Condensed. |
| `browser_frames`, `browser_scroll_into_view`, `browser_set_content` | `browser_dom`, `browser_wait`, or advanced browser-control profile route | Removed from normal public surface as direct implementation seams. |
| `browser_wait_for`, `browser_assert` | `browser_wait operation=for_condition` | Condensed. |
| `browser_batch` | Use ordered calls to `browser_tabs`, `browser_nav`, `browser_dom`, `browser_form`, `browser_wait`, `browser_capture` | Removed from normal public surface to avoid ambiguous multi-side-effect batches. |
| `browser_clock`, `browser_page_events` | `browser_wait operation=for_condition`, `audit operation=command_query`, or advanced browser-control profile route | Removed from normal public surface. |
| `browser_set_value`, `browser_fill_form` | `browser_form operation=set_value/fill` | Condensed. |
| `browser_screenshot` | `browser_capture operation=screenshot` | Condensed. |
| `browser_downloads` | `browser_capture operation=downloads` | Condensed. |
| `browser_storage`, `browser_cookies` | `browser_storage operation=get/set/clear/save_state/load_state` | Condensed. |
| `browser_file_upload` | Switch to `browser_debugger`, then `browser_debugger operation=file_upload` | Profile-gated advanced route. |
| `workspace_get`, `workspace_put`, `workspace_list`, `workspace_subscribe` | `workspace operation=get/put/list/subscribe`; new `exists/delete` also live | Condensed. |
| `agent_query`, `agent_inbox`, `agent_wait`, `agent_send`, `agent_send_broadcast`, `agent_receipts`, `agent_stats` | `agent operation=query/inbox/wait/send/broadcast/receipts/stats` | Condensed. |
| `agent_interrupt`, `agent_kill`, `agent_pause`, `agent_resume`, `agent_respawn`, `agent_steer` | `agent operation=interrupt/kill/pause/resume/respawn/steer` | Condensed. |
| `agent_template_put`, `agent_template_get`, `agent_template_list`, `agent_template_delete` | `agent operation=template_put/template_get/template_list/template_delete` | Condensed. |
| `agent_ask_operator` | `approval operation=ask_operator` | Condensed. |
| `agent_cost` | `cost operation=summarize` | Condensed. |
| `agent_spawn_task_started` | `agent operation=task_started` | Condensed; spawned-agent readiness stays in the visible agent facade without adding a public tool. |
| `fleet_stop` | `agent operation=kill` scoped by explicit ids, or advanced maintenance route for fleet-wide stops | Removed from normal public surface as broad destructive control. |
| `task_create`, `task_get`, `task_update`, `task_claim`, `task_cancel`, `task_list`, `task_next`, `task_reconcile`, `task_dispatch_once` | `task operation=create/get/update/claim/cancel/list/next/reconcile/dispatch_once` | Condensed. |
| `approval_request`, `approval_list`, `approval_decide`, `approval_gate` | `approval operation=request/list/decide/gate` | Condensed. |
| `escalation_list`, `escalation_ack` | `escalation operation=list/ack` | Condensed. |
| `timeline_get`, `timeline_search`, `timeline_stats` | `timeline operation=get/search/stats` | Condensed. |
| `timeline_digest` | `timeline operation=search/stats` plus agent-side summary, or future timeline summary operation | Removed from normal public surface as standalone route. |
| `timeline_pause`, `timeline_resume`, `timeline_exclusions`, `timeline_redact`, `timeline_purge` | `privacy operation=pause/resume/exclusions/redact/purge` | Condensed into privacy facade. |
| `episode_list`, `episode_get` | `episode operation=list/get` | Condensed. |
| `episode_segment` | Advanced episode/mining route | Removed from normal public surface. |
| `routine_mine`, `routine_list`, `routine_inspect`, `routine_update`, `routine_feedback`, `routine_label_export` | `routine operation=mine/list/inspect/update/feedback/label` | Condensed. |
| `armed_routine_tick` | `routine operation=armed_tick` for controlled routine profile, not normal-agent scheduling | Removed from normal public surface. |
| `intent_current`, `intent_detect_tick` | `assist operation=intent/detect` | Condensed; scheduler tick is no longer a standalone normal tool. |
| `suggestion_tick`, `suggestion_list`, `suggestion_accept` | `assist operation=suggestion_tick/suggestion_list/suggestion_accept` | Condensed; tick is controlled by assist/routine routing. |
| `reality_baseline`, `reality_audit` | `reality operation=baseline/audit` | Condensed. |
| `verification_inbox`, `verification_poll`, `verification_audit`, `verification_bind`, `verification_sources` | `verification operation=inbox/poll/audit/bind/sources` | Condensed. |
| `storage_inspect` | `storage operation=inspect` | Condensed. |
| `storage_gc_once` | `storage operation=gc_once` | Condensed; mutating maintenance route. |
| `storage_put_probe_rows` | No facade operation; hidden implementation tool only | Removed from the public surface. |
| `local_model_list`, `local_model_probe` | `model operation=list/probe` and `model operation=status` | Condensed. |
| `local_model_register`, `local_model_update`, `local_model_remove` | `model operation=register/update/remove` under explicit maintenance policy | Removed from normal public surface as registry mutators. |
| `hygiene_flags`, `hygiene_report`, `hygiene_scan_storage`, `hygiene_scan_text` | `hygiene operation=flags/report/scan_storage/scan_text` | Condensed. |
| `audit_intelligence_query` | `audit operation=profile_intelligence` | Condensed and narrowed. |
| `demo_record_start`, `demo_record_stop` | `replay operation=demo_start/demo_stop`; status via `replay operation=demo_status` | Condensed and no longer default demo controls. |

## Edge Rules

Old tool missing from mapping: treat as documentation failure and update this file before closing a migration issue.

Removed tool still public in `normal_agent`: treat as profile regression. Fix `NORMAL_ALLOWED_EXACT` or the profile router before claiming the 40-tool surface is live.

Doc example uses hidden route from `normal_agent`: treat as a documentation regression. Examples must use current facade tools only.

Public count above 40: treat as release blocker for #1374. The source of truth is production-client `tools/list`, not a local constant alone.
