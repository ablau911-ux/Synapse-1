// Frozen behaviour table (manager-owned, work method v3; roadmap item #583,
// issue #295, with #317 row 4h; 2026-11-20). Chrome runs no input and paints
// nothing for a hidden page (covered or minimized window, background tab).
// The bridge opens a BackgroundPageSession (focus emulation + screencast) for
// click, dblclick, hover, drag and wheel and never switches the tab; tap keeps
// today's path (manager 2026-11-20 10:15: no frame-wait flag in the plan: the
// running screencast is enough for drag, the wheel settles a fixed time). With the render switch off (the default) a hidden tab is
// captured in place under the screencast; with the switch on the D7 decisions
// are unchanged. Supersedes the switch-off rows of
// page_screenshot_hidden_tab_switch_table_d7off.test.mjs (hidden_tab_render_off).
// Codex and Opus never edit this file. One case per L row of the table in #295.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const source = readFileSync(new URL("../service_worker.js", import.meta.url), "utf8");

function block(startMarker, endMarker) {
  const start = source.indexOf(startMarker);
  const end = source.indexOf(endMarker, start);
  return start >= 0 && end > start ? source.slice(start, end) : "";
}

const inputCode = block("// --- background cdp input decisions", "// --- end background cdp input decisions ---");
const shotCode = block("// --- background page screenshot decisions", "// --- end background page screenshot decisions ---");

function load() {
  const context = vm.createContext({});
  vm.runInContext(
    `${inputCode}
     ${shotCode}
     globalThis.cdpInputPlan = typeof cdpInputPlan === "function" ? cdpInputPlan : undefined;
     globalThis.hiddenTabCaptureDecision =
       typeof hiddenTabCaptureDecision === "function" ? hiddenTabCaptureDecision : undefined;`,
    context
  );
  return context;
}

const plan = (action) => (ctx) => {
  if (typeof ctx.cdpInputPlan !== "function") {
    return { ok: "missing cdpInputPlan" };
  }
  const got = ctx.cdpInputPlan(action);
  return {
    ok: got?.ok,
    reason: got?.reason ?? null,
    activateTab: got?.activateTab,
    backgroundSession: got?.backgroundSession
  };
};

const base = {
  renderEnabled: false,
  visibility: "hidden",
  originTabCount: 3,
  originWindowState: "normal",
  pinned: false,
  grouped: false,
  alreadyRendering: false
};
const capture = (overrides) => (ctx) => {
  if (typeof ctx.hiddenTabCaptureDecision !== "function") {
    return { action: "missing hiddenTabCaptureDecision", reason: "missing" };
  }
  const input = { ...base, ...overrides };
  for (const [key, value] of Object.entries(overrides)) {
    if (value === undefined) {
      delete input[key];
    }
  }
  const got = ctx.hiddenTabCaptureDecision(input);
  return { action: got?.action, reason: got?.reason ?? null };
};

const SESSION = { ok: true, reason: null, activateTab: false, backgroundSession: true };
const TAP = { ok: true, reason: null, activateTab: true, backgroundSession: false };
const UNKNOWN = { ok: false, reason: "unknown_action" };
const SCREENCAST = { action: "in_place_screencast", reason: null };
const IN_PLACE = { action: "in_place", reason: null };
const BUSY = { action: "refuse", reason: "render_in_progress" };

const rows = [
  // L1 click and dblclick: background session, no tab switch
  { row: "L1_click_uses_background_session_without_tab_switch", run: plan("click"), expected: SESSION },
  { row: "L1_dblclick_uses_background_session_without_tab_switch", run: plan("dblclick"), expected: SESSION },
  // L2 hover
  { row: "L2_hover_uses_background_session_without_tab_switch", run: plan("hover"), expected: SESSION },
  // L3 drag and wheel take the same background session
  { row: "L3_drag_uses_background_session_without_tab_switch", run: plan("drag"), expected: SESSION },
  { row: "L3_wheel_uses_background_session_without_tab_switch", run: plan("wheel"), expected: SESSION },
  // L4 neighbour: tap keeps today's path
  { row: "L4_neighbour_tap_keeps_tab_activation_and_no_session", run: plan("tap"), expected: TAP },
  // L5 empty, missing, structurally invalid, hostile
  { row: "L5_empty_action_is_unknown", run: plan(""), expected: UNKNOWN },
  { row: "L5_missing_action_is_unknown", run: plan(undefined), expected: UNKNOWN },
  { row: "L5_number_action_is_unknown", run: plan(42), expected: UNKNOWN },
  { row: "L5_upper_case_action_is_unknown", run: plan("CLICK"), expected: UNKNOWN },
  { row: "L5_scroll_is_not_a_bridge_input_verb", run: plan("scroll"), expected: UNKNOWN },
  // L6 switch off, hidden tab in a covered window or in the background
  { row: "L6_off_hidden_tab_is_captured_in_place_under_screencast", run: capture({}), expected: SCREENCAST },
  // L6b switch off by default; only a strict boolean true switches it on;
  // any visibility other than "visible" counts as hidden (rows carried over
  // from the superseded d7off table)
  { row: "L6b_missing_switch_means_off", run: capture({ renderEnabled: undefined }), expected: SCREENCAST },
  { row: "L6b_hostile_switch_string_1_means_off", run: capture({ renderEnabled: "1" }), expected: SCREENCAST },
  { row: "L6b_hostile_switch_string_true_means_off", run: capture({ renderEnabled: "true" }), expected: SCREENCAST },
  { row: "L6b_hostile_switch_number_1_means_off", run: capture({ renderEnabled: 1 }), expected: SCREENCAST },
  { row: "L6b_unknown_visibility_counts_as_hidden", run: capture({ visibility: "unknown" }), expected: SCREENCAST },
  { row: "L6b_hostile_visibility_value_counts_as_hidden", run: capture({ visibility: 42 }), expected: SCREENCAST },
  // L7 switch off, the cases a move had to refuse: nothing moves now
  {
    row: "L7_off_minimized_window_is_captured_in_place_under_screencast",
    run: capture({ originWindowState: "minimized" }),
    expected: SCREENCAST
  },
  { row: "L7_off_only_tab_is_captured_in_place_under_screencast", run: capture({ originTabCount: 1 }), expected: SCREENCAST },
  { row: "L7_off_pinned_tab_is_captured_in_place_under_screencast", run: capture({ pinned: true }), expected: SCREENCAST },
  { row: "L7_off_grouped_tab_is_captured_in_place_under_screencast", run: capture({ grouped: true }), expected: SCREENCAST },
  {
    row: "L7_off_unknown_tab_count_is_captured_in_place_under_screencast",
    run: capture({ originTabCount: undefined }),
    expected: SCREENCAST
  },
  // L8 neighbour: a visible tab is captured in place as before
  { row: "L8_neighbour_off_visible_tab_is_captured_in_place", run: capture({ visibility: "visible" }), expected: IN_PLACE },
  {
    row: "L8_neighbour_on_visible_tab_is_captured_in_place",
    run: capture({ renderEnabled: true, visibility: "visible" }),
    expected: IN_PLACE
  },
  // L9 second actor: a render already running on this tab
  {
    row: "L9_on_R2f_tab_in_render_window_reports_visible_is_refused",
    run: capture({ renderEnabled: true, visibility: "visible", alreadyRendering: true }),
    expected: BUSY
  },
  {
    row: "L9_off_tab_still_being_returned_is_refused",
    run: capture({ visibility: "visible", alreadyRendering: true }),
    expected: BUSY
  },
  {
    row: "L9_off_second_actor_tab_already_rendering_is_refused",
    run: capture({ alreadyRendering: true }),
    expected: BUSY
  },
  {
    row: "L9_on_second_actor_tab_already_rendering_is_refused",
    run: capture({ renderEnabled: true, alreadyRendering: true }),
    expected: BUSY
  },
  // L10 neighbour: switch on keeps the D7 decisions
  {
    row: "L10_neighbour_on_hidden_tab_uses_render_window",
    run: capture({ renderEnabled: true }),
    expected: { action: "render_window", reason: null }
  },
  {
    row: "L10_neighbour_on_minimized_window_is_refused",
    run: capture({ renderEnabled: true, originWindowState: "minimized" }),
    expected: { action: "refuse", reason: "origin_window_minimized" }
  },
  {
    row: "L10_neighbour_on_pinned_tab_is_refused",
    run: capture({ renderEnabled: true, pinned: true }),
    expected: { action: "refuse", reason: "pinned_or_grouped_tab" }
  },
  {
    row: "L10_neighbour_on_only_tab_is_refused",
    run: capture({ renderEnabled: true, originTabCount: 1 }),
    expected: { action: "refuse", reason: "only_tab_in_window" }
  }
];

test("background_page_table_583", () => {
  const ctx = load();
  const failures = [];
  for (const { row, run, expected } of rows) {
    let got;
    try {
      got = run(ctx);
    } catch (error) {
      failures.push(`row ${row}: threw ${String(error?.message ?? error)}`);
      continue;
    }
    for (const [key, value] of Object.entries(expected)) {
      if (got[key] !== value) {
        failures.push(`row ${row}: ${key}=${JSON.stringify(got[key])}, expected ${JSON.stringify(value)}`);
      }
    }
  }
  assert.equal(failures.length, 0, `background page table rows failed (${failures.length} of ${rows.length}):\n${failures.join("\n")}`);
});
