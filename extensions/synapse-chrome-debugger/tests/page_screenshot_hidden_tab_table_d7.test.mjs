// Frozen behaviour table (manager-owned, work method v3; owner decision D7,
// upstream 6d9f8b8e). A hidden owned tab (inactive, or its window covered) is
// rendered in a temporary unfocused window beside its own window, captured,
// and returned to its window, index and active-tab state; anything that
// cannot be returned safely is refused before the tab moves. Codex and Opus
// never edit this file. One case per L row.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const source = readFileSync(new URL("../service_worker.js", import.meta.url), "utf8");
const startMarker = "// --- background page screenshot decisions";
const endMarker = "// --- end background page screenshot decisions ---";
const start = source.indexOf(startMarker);
const end = source.indexOf(endMarker, start);
assert.ok(start >= 0 && end > start, "background page screenshot decisions block missing");
const code = source.slice(start, end);

function load() {
  const context = vm.createContext({});
  vm.runInContext(
    `${code}
     globalThis.hiddenTabRenderDecision = hiddenTabRenderDecision;
     globalThis.renderWindowPlacements = renderWindowPlacements;
     globalThis.restoreVerdict = restoreVerdict;`,
    context
  );
  return context;
}

const base = {
  visibility: "hidden",
  originTabCount: 3,
  originWindowState: "normal",
  pinned: false,
  grouped: false,
  alreadyRendering: false
};
const decide = (overrides) => ({
  run: (ctx) => {
    const got = ctx.hiddenTabRenderDecision({ ...base, ...overrides });
    return { action: got?.action, reason: got?.reason ?? null };
  }
});
const place = (origin) => ({
  run: (ctx) => ({ placements: JSON.stringify(ctx.renderWindowPlacements(origin)) })
});
const spots = (list) =>
  JSON.stringify(list.map(([side, left, top, width, height]) => ({ side, left, top, width, height })));

const record = {
  tabId: 7,
  originWindowId: 1,
  originIndex: 2,
  originWasActive: false,
  originActiveTabId: 5
};
const verdict = (recordOverrides, after, mentions = []) => ({
  run: (ctx) => {
    const got = ctx.restoreVerdict({ ...record, ...recordOverrides }, after);
    const error = String(got?.error ?? "");
    return {
      ok: got?.ok,
      mentions: mentions.every((part) => error.includes(part))
    };
  }
});

const rows = [
  // 2b: decision
  { row: "2b_visible_tab_is_captured_in_place", ...decide({ visibility: "visible" }), expected: { action: "in_place", reason: null } },
  { row: "2b_hidden_tab_uses_render_window", ...decide({}), expected: { action: "render_window", reason: null } },
  { row: "2b_unknown_visibility_uses_render_window", ...decide({ visibility: "unknown" }), expected: { action: "render_window", reason: null } },
  { row: "2b_hostile_visibility_value_uses_render_window", ...decide({ visibility: 42 }), expected: { action: "render_window", reason: null } },
  // 2c: boundary and refusals before any move
  { row: "2c_boundary_only_tab_in_window_is_refused", ...decide({ originTabCount: 1 }), expected: { action: "refuse", reason: "only_tab_in_window" } },
  { row: "2c_boundary_two_tabs_uses_render_window", ...decide({ originTabCount: 2 }), expected: { action: "render_window", reason: null } },
  { row: "2c_empty_tab_count_is_refused", ...decide({ originTabCount: undefined }), expected: { action: "refuse", reason: "origin_tab_count_unknown" } },
  { row: "2c_hostile_zero_tab_count_is_refused", ...decide({ originTabCount: 0 }), expected: { action: "refuse", reason: "origin_tab_count_unknown" } },
  { row: "2c_minimized_origin_is_refused", ...decide({ originWindowState: "minimized" }), expected: { action: "refuse", reason: "origin_window_minimized" } },
  { row: "2c_pinned_tab_is_refused", ...decide({ pinned: true }), expected: { action: "refuse", reason: "pinned_or_grouped_tab" } },
  { row: "2c_grouped_tab_is_refused", ...decide({ grouped: true }), expected: { action: "refuse", reason: "pinned_or_grouped_tab" } },
  { row: "2f_second_actor_same_tab_already_rendering_is_refused", ...decide({ alreadyRendering: true }), expected: { action: "refuse", reason: "render_in_progress" } },
  // 2d: placements
  {
    row: "2d_normal_origin_four_spots_in_order",
    ...place({ left: 0, top: 0, width: 1280, height: 800 }),
    expected: {
      placements: spots([
        ["right", 1320, 40, 1280, 800],
        ["left", -1320, 40, 1280, 800],
        ["below", 40, 840, 1280, 800],
        ["above", 40, -840, 1280, 800]
      ])
    }
  },
  {
    row: "2d_boundary_huge_origin_clamps_render_size",
    ...place({ left: 100, top: 50, width: 3000, height: 2000 }),
    expected: {
      placements: spots([
        ["right", 3140, 90, 1600, 1000],
        ["left", -1540, 90, 1600, 1000],
        ["below", 140, 2090, 1600, 1000],
        ["above", 140, -990, 1600, 1000]
      ])
    }
  },
  {
    row: "2d_boundary_tiny_origin_clamps_render_size",
    ...place({ left: 10, top: 10, width: 300, height: 200 }),
    expected: {
      placements: spots([
        ["right", 350, 50, 800, 600],
        ["left", -830, 50, 800, 600],
        ["below", 50, 250, 800, 600],
        ["above", 50, -630, 800, 600]
      ])
    }
  },
  {
    row: "2d_empty_origin_uses_defaults",
    ...place(null),
    expected: {
      placements: spots([
        ["right", 1320, 40, 1280, 900],
        ["left", -1320, 40, 1280, 900],
        ["below", 40, 940, 1280, 900],
        ["above", 40, -940, 1280, 900]
      ])
    }
  },
  {
    row: "2d_hostile_nan_origin_uses_defaults",
    ...place({ left: Number.NaN, top: "x", width: Number.NaN, height: null }),
    expected: {
      placements: spots([
        ["right", 1320, 40, 1280, 900],
        ["left", -1320, 40, 1280, 900],
        ["below", 40, 940, 1280, 900],
        ["above", 40, -940, 1280, 900]
      ])
    }
  },
  // 2e: restore verdict
  {
    row: "2e_normal_returned_to_window_index_and_active_tab_is_ok",
    ...verdict({}, { windowId: 1, index: 2, activeTabId: 5 }),
    expected: { ok: true, mentions: true }
  },
  {
    row: "2e_was_active_tab_returned_active_is_ok",
    ...verdict({ originWasActive: true, originActiveTabId: 7 }, { windowId: 1, index: 2, activeTabId: 7 }),
    expected: { ok: true, mentions: true }
  },
  {
    row: "2e_unknown_prior_active_checks_position_only",
    ...verdict({ originActiveTabId: null }, { windowId: 1, index: 2, activeTabId: 9 }),
    expected: { ok: true, mentions: true }
  },
  {
    row: "2e_hostile_wrong_window_is_error_naming_tab_window_index",
    ...verdict({}, { windowId: 44, index: 0, activeTabId: 5 }, ["tab 7", "window 1", "index 2"]),
    expected: { ok: false, mentions: true }
  },
  {
    row: "2e_wrong_index_is_error",
    ...verdict({}, { windowId: 1, index: 0, activeTabId: 5 }, ["tab 7", "index 2"]),
    expected: { ok: false, mentions: true }
  },
  {
    row: "2e_someone_else_active_tab_not_restored_is_error",
    ...verdict({}, { windowId: 1, index: 2, activeTabId: 7 }, ["active tab 5"]),
    expected: { ok: false, mentions: true }
  },
  {
    row: "2e_tab_gone_is_error",
    ...verdict({}, null, ["tab 7", "window 1", "index 2"]),
    expected: { ok: false, mentions: true }
  }
];

test("page_screenshot_hidden_tab_table_d7", () => {
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
  assert.equal(failures.length, 0, `hidden tab render table rows failed:\n${failures.join("\n")}`);
});
