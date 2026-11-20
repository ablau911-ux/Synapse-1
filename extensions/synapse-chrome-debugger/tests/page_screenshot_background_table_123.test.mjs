// Frozen behaviour table (manager-owned, work method v3; #123/#177, upstream
// 847cb1bc). A pageScreenshot of an owned tab whose Chrome is not in front
// (or whose tab is not the visible one) is rendered through chrome.debugger
// Page.captureScreenshot in chunks, never by switching the tab or focusing a
// window. Codex and Opus never edit this file. One case per L row.
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
     globalThis.pageScreenshotRoute = pageScreenshotRoute;
     globalThis.backgroundChunkPlan = backgroundChunkPlan;
     globalThis.CHUNK = PAGE_SCREENSHOT_BACKGROUND_CHUNK_CSS;
     globalThis.MAX = PAGE_SCREENSHOT_BACKGROUND_MAX_CHUNKS;`,
    context
  );
  return context;
}

const route = (background, tabActive, windowFocused) => ({
  run: (ctx) => ({ lane: ctx.pageScreenshotRoute({ background, tabActive, windowFocused }) })
});
const plan = (clip) => ({
  run: (ctx) => {
    const got = ctx.backgroundChunkPlan(clip, ctx.CHUNK, ctx.MAX);
    return {
      ok: got?.ok,
      reason: got?.ok ? null : got?.reason,
      count: got?.count,
      chunks: got?.ok && got.count <= 3 ? JSON.stringify(got.chunks) : null
    };
  }
});

const rows = [
  // 1k: lane choice inside the worker
  { row: "1k_background_true_inactive_unfocused_is_debugger", ...route(true, false, false), expected: { lane: "debugger" } },
  { row: "1k_background_true_even_active_focused_is_debugger", ...route(true, true, true), expected: { lane: "debugger" } },
  { row: "1k_neighbour_auto_active_focused_is_visible_tab", ...route("auto", true, true), expected: { lane: "visible_tab" } },
  { row: "1k_auto_inactive_tab_is_debugger", ...route("auto", false, true), expected: { lane: "debugger" } },
  { row: "1k_auto_unfocused_window_is_debugger", ...route("auto", true, false), expected: { lane: "debugger" } },
  { row: "1k_someone_else_owner_chrome_inactive_tab_is_refused", ...route(false, false, true), expected: { lane: "refuse" } },
  { row: "1k_neighbour_absent_flag_active_focused_is_visible_tab", ...route(undefined, true, true), expected: { lane: "visible_tab" } },
  { row: "1k_hostile_string_true_inactive_is_refused", ...route("true", false, false), expected: { lane: "refuse" } },
  { row: "1k_hostile_auto_non_boolean_active_is_debugger", ...route("auto", "yes", true), expected: { lane: "debugger" } },
  // 1c: chunk plan
  {
    row: "1c_boundary_4096_is_one_chunk",
    ...plan({ x: 0, y: 0, w: 800, h: 4096 }),
    expected: { ok: true, reason: null, count: 1, chunks: JSON.stringify([{ y: 0, h: 4096 }]) }
  },
  {
    row: "1c_boundary_4097_is_two_chunks",
    ...plan({ x: 0, y: 0, w: 800, h: 4097 }),
    expected: { ok: true, reason: null, count: 2, chunks: JSON.stringify([{ y: 0, h: 4096 }, { y: 4096, h: 1 }]) }
  },
  {
    row: "1c_offset_clip_starts_at_clip_y",
    ...plan({ x: 10, y: 300, w: 800, h: 5000 }),
    expected: { ok: true, reason: null, count: 2, chunks: JSON.stringify([{ y: 300, h: 4096 }, { y: 4396, h: 904 }]) }
  },
  {
    row: "1c_normal_fractional_viewport_is_one_chunk",
    ...plan({ x: 0, y: 0, w: 1280, h: 600.5 }),
    expected: { ok: true, reason: null, count: 1, chunks: JSON.stringify([{ y: 0, h: 600.5 }]) }
  },
  {
    row: "1c_boundary_exactly_max_chunks_is_allowed",
    ...plan({ x: 0, y: 0, w: 800, h: 4096 * 400 }),
    expected: { ok: true, reason: null, count: 400, chunks: null }
  },
  {
    row: "1c_boundary_one_over_max_is_refused_naming_count",
    ...plan({ x: 0, y: 0, w: 800, h: 4096 * 400 + 1 }),
    expected: { ok: false, reason: "too_many_chunks", count: 401, chunks: null }
  },
  {
    row: "1c_empty_zero_height_is_refused",
    ...plan({ x: 0, y: 0, w: 800, h: 0 }),
    expected: { ok: false, reason: "empty_clip", count: 0, chunks: null }
  },
  {
    row: "1c_hostile_nan_height_is_refused",
    ...plan({ x: 0, y: 0, w: 800, h: Number.NaN }),
    expected: { ok: false, reason: "empty_clip", count: 0, chunks: null }
  },
  {
    row: "1c_hostile_negative_height_is_refused",
    ...plan({ x: 0, y: 0, w: 800, h: -5 }),
    expected: { ok: false, reason: "empty_clip", count: 0, chunks: null }
  },
  {
    row: "1c_hostile_missing_clip_is_refused",
    ...plan(null),
    expected: { ok: false, reason: "empty_clip", count: 0, chunks: null }
  }
];

test("page_screenshot_background_table_123", () => {
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
  assert.equal(failures.length, 0, `page screenshot background table rows failed:\n${failures.join("\n")}`);
});
