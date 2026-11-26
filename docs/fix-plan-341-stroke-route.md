# Fix plan: public stroke route (#341)

## Decision and evidence

This is a missing public facade route. Keep raw implementation tools hidden and add a stroke verb to the existing `act` facade. Follow upstream's stable 40-tool surface and operation-level authority design; upstream has no implemented public stroke solution to copy.

Upstream inspected first with `git show origin/main:<path>` and scoped history. The fetched upstream ref is `b3a546c4fd7813ff319817471fc9203b609b5f10`:

- `origin/main:docs/SYNAPSE_40_TOOL_SURFACE_MIGRATION.md:18`: "Every scoped production HTTP profile advertises the same facade surface" and "Raw implementation tools remain hidden for scoped HTTP profiles". Relevant document history includes `a1e318e6`.
- `origin/main:crates/synapse-mcp/src/server/background_router/mod.rs:59`: `TARGET_ACT_KNOWN_VERBS` has no stroke or drag verb.
- `origin/main:crates/synapse-mcp/src/server/tool_profiles.rs:103`: `"act_stroke",` remains in the public implementation denylist.
- `origin/main:crates/synapse-mcp/src/server/m2_tools/mod.rs:1440` tries `try_act_stroke_cdp_background_target` before foreground lease acquisition; its implementation starts at line 4171.

Local evidence at baseline `bf419b5a`:

- `docs/SYNAPSE_40_TOOL_SURFACE_MIGRATION.md:18`: "Profiles gate operation-level authority"; real foreground uses audited `act operation=foreground`.
- `crates/synapse-mcp/src/server/background_router/mod.rs:10`: "thin dispatcher: each verb delegates to the existing tool method". Line 62 lists verbs without stroke.
- `crates/synapse-mcp/src/server/tool_profiles.rs:4287`: `"act_stroke" | "act_pad"` recommends target, lease, and break_glass but provides no public stroke invocation.
- `crates/synapse-mcp/src/server/m2_tools/mod.rs:1502`: the existing stroke handler tries the CDP background route before acquiring a foreground lease. The helper at line 4271 requires an explicit sampled path and refuses real-cursor fallback for an active CDP target.
- `crates/synapse-mcp/src/m2/stroke.rs:284`: the existing implementation delegates to `synapse_a11y::cdp_mouse_stroke_target`, preserving the #341 closed-target outcome.
- `crates/synapse-mcp/src/server/m2_tools/mod.rs:2541`: foreground lease acquisition itself does not check the tool profile. A new facade route must retain an explicit operation-level profile check for that lane.

## Parts and files

1. In `crates/synapse-mcp/src/server/background_router/mod.rs`, add `verb=stroke` and nested `stroke: ActStrokeParams` to avoid colliding with the existing string `path` field. Require a bound session target. Delegate to `self.act_stroke` and reuse its validation, plan, audit, dispatch, and response. Update the known-verb error list and both facade descriptions.
2. In `crates/synapse-mcp/src/server/m2_tools/mod.rs`, keep raw-CDP path dispatch background and available under normal profiles. At the actual foreground lane boundary, require break_glass/full_capability before real-cursor delivery, retaining target-claim, lease, hidden-desktop, operator-panic, and foreground-monitor guards. Reject missing-target fallback through the public route. Inspect element-aim and recording lanes before placing this check so background delivery is not unnecessarily gated.
3. Correct the stroke capability hint in `crates/synapse-mcp/src/server/tool_profiles.rs`. Update `docs/SYNAPSE_40_TOOL_SURFACE_MIGRATION.md` and the stroke rows in `docs/multi-agent-capability-matrix.md`. Check for verb snapshots or contract hashes; the inspected `act` contract lists operations rather than verbs. Do not expose a 41st tool, add dependencies, or add tests under tests/.

Commit this plan alone before implementation. Commit the fix separately as codex; never push or merge.

## Proof through public MCP tools

Run sequentially at low priority:

1. `cargo fmt --all -- --check`
2. `cargo clippy -p synapse-mcp -p synapse-a11y --all-targets -j 4 -- -D warnings`

Use Synapse against an isolated disposable raw-CDP page on a build containing the change. Bind with public `target operation=set`, claim with `target operation=claim`, and invoke public `act` with `action.verb=stroke` and nested `action.stroke` using the existing `ActStrokeParams` path/timing schema. No new transport, raw-tool exposure, or debugger-bridge drag substitution.

Verify normal-profile movement and a left-button drag delegate to `act_stroke`, report the CDP background tier and `required_foreground=false`, and produce expected page pointer-event/DOM readback. Inspect rendered state through Synapse. Repeat on a script-opened page whose mouse-down closes its own target; verify the #341 closed-target response. Check invalid input, a missing target, and native/hardware delivery without foreground authority fail closed. Exercise authorized native delivery through `act operation=foreground` with a reason and verify profile/lease cleanup. Verify the registry remains 40 tools.

A deployed old binary cannot prove this change. If a debug build cannot be run safely within the time cap, record live verification as blocked. Do not build release, restart shared MCP services, or claim UI completion without Synapse evidence.
