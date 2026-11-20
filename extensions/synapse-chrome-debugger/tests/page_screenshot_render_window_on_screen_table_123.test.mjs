// Frozen behaviour table (manager-owned, work method v3, step 7 of #123/D7).
// The temporary render window for a hidden tab must be placed where Chrome
// accepts it ("Bounds must be at least 50% within visible screen space"):
// every spot inside the work area of the tab's screen, least overlap with the
// tab's own window first; work area unknown = one spot inside the origin
// window's own bounds. When Chrome rejects a spot the next one is tried; only
// when every spot is rejected is the error returned, naming how many were
// tried. Codex and Opus never edit this file. One case per L row.
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
     globalThis.renderWindowSpots = renderWindowSpots;
     globalThis.createRenderWindowAtFirstAcceptedSpot = createRenderWindowAtFirstAcceptedSpot;`,
    context
  );
  return context;
}

const spots = (list) =>
  JSON.stringify(list.map(([side, left, top, width, height]) => ({ side, left, top, width, height })));

// Spot rows: exact list, plus invariants read from the list itself.
const insideArea = (spot, area) =>
  spot.left >= area.left &&
  spot.top >= area.top &&
  spot.left + spot.width <= area.left + area.width &&
  spot.top + spot.height <= area.top + area.height;
const place = (origin, workArea, insideOf) => ({
  run: async (ctx) => {
    const got = ctx.renderWindowSpots(origin, workArea);
    const list = Array.isArray(got) ? got : [];
    return {
      placements: JSON.stringify(got),
      nonEmpty: list.length > 0,
      positive: list.every((spot) => spot.width > 0 && spot.height > 0),
      inside: list.every((spot) => insideArea(spot, insideOf))
    };
  }
});

const rejected = () => new Error("Invalid value for bounds. Bounds must be at least 50% within visible screen space.");
const threeSpots = [
  { side: "right", left: 223, top: 0, width: 1313, height: 864 },
  { side: "below", left: 53, top: 0, width: 1313, height: 864 },
  { side: "left", left: 0, top: 0, width: 1313, height: 864 }
];
// `answers[i]` is what the fake chrome.windows.create does on call i:
// "reject" throws Chrome's bounds error, a number resolves {id: number}.
const create = (list, answers, mentions = []) => ({
  run: async (ctx) => {
    const calls = [];
    const fake = async (options) => {
      calls.push(options);
      const answer = answers[calls.length - 1];
      if (answer === "reject") {
        throw rejected();
      }
      return { id: answer };
    };
    try {
      const got = await ctx.createRenderWindowAtFirstAcceptedSpot(7, list, fake);
      const used = calls[calls.length - 1] ?? {};
      return {
        outcome: "created",
        index: got?.index,
        windowId: got?.window?.id,
        calls: calls.length,
        options: JSON.stringify(used)
      };
    } catch (error) {
      const message = String(error?.message ?? error);
      return {
        outcome: "error",
        calls: calls.length,
        mentions: mentions.every((part) => message.includes(part))
      };
    }
  }
});

const area = { left: 0, top: 0, width: 1536, height: 864 };
const rows = [
  // RW1: the live failure (origin 1313x996 at 13,13 on a 1536x864 work area)
  {
    row: "RW1_normal_origin_spots_inside_work_area_least_overlap_first",
    ...place({ left: 13, top: 13, width: 1313, height: 996 }, area, area),
    expected: {
      placements: spots([
        ["right", 223, 0, 1313, 864],
        ["below", 53, 0, 1313, 864],
        ["left", 0, 0, 1313, 864]
      ]),
      nonEmpty: true,
      positive: true,
      inside: true
    }
  },
  {
    row: "RW1_neighbour_room_beside_origin_keeps_no_overlap_spots_first",
    ...place({ left: 0, top: 0, width: 1280, height: 800 }, { left: 0, top: 0, width: 10000, height: 8000 }, { left: 0, top: 0, width: 10000, height: 8000 }),
    expected: {
      placements: spots([
        ["right", 1320, 40, 1280, 800],
        ["below", 40, 840, 1280, 800],
        ["left", 0, 40, 1280, 800],
        ["above", 40, 0, 1280, 800]
      ]),
      nonEmpty: true,
      positive: true,
      inside: true
    }
  },
  // RW2: work area unknown
  {
    row: "RW2_empty_work_area_one_spot_inside_origin_bounds",
    ...place({ left: 13, top: 13, width: 1313, height: 996 }, null, { left: 13, top: 13, width: 1313, height: 996 }),
    expected: {
      placements: spots([["origin", 13, 13, 1313, 996]]),
      nonEmpty: true,
      positive: true,
      inside: true
    }
  },
  {
    row: "RW2_small_origin_spot_stays_inside_origin_bounds",
    ...place({ left: 10, top: 10, width: 300, height: 200 }, undefined, { left: 10, top: 10, width: 300, height: 200 }),
    expected: {
      placements: spots([["origin", 10, 10, 300, 200]]),
      nonEmpty: true,
      positive: true,
      inside: true
    }
  },
  {
    row: "RW2_empty_origin_and_work_area_default_spot",
    ...place(null, null, { left: 0, top: 0, width: 1280, height: 900 }),
    expected: {
      placements: spots([["origin", 0, 0, 1280, 900]]),
      nonEmpty: true,
      positive: true,
      inside: true
    }
  },
  // RW3: boundary and hostile work areas
  {
    row: "RW3_boundary_tiny_work_area_shrinks_to_fit",
    ...place({ left: 0, top: 0, width: 1280, height: 800 }, { left: 0, top: 0, width: 400, height: 300 }, { left: 0, top: 0, width: 400, height: 300 }),
    expected: {
      placements: spots([["right", 0, 0, 400, 300]]),
      nonEmpty: true,
      positive: true,
      inside: true
    }
  },
  {
    row: "RW3_multi_monitor_negative_origin_work_area",
    ...place(
      { left: -1800, top: -100, width: 1280, height: 800 },
      { left: -1920, top: -200, width: 1920, height: 1080 },
      { left: -1920, top: -200, width: 1920, height: 1080 }
    ),
    expected: {
      placements: spots([
        ["right", -1280, -60, 1280, 800],
        ["below", -1760, 80, 1280, 800],
        ["above", -1760, -200, 1280, 800],
        ["left", -1920, -60, 1280, 800]
      ]),
      nonEmpty: true,
      positive: true,
      inside: true
    }
  },
  {
    row: "RW3_hostile_negative_size_work_area_is_unknown",
    ...place({ left: 13, top: 13, width: 1313, height: 996 }, { left: 0, top: 0, width: -100, height: 500 }, { left: 13, top: 13, width: 1313, height: 996 }),
    expected: {
      placements: spots([["origin", 13, 13, 1313, 996]]),
      nonEmpty: true,
      positive: true,
      inside: true
    }
  },
  {
    row: "RW3_hostile_zero_size_work_area_is_unknown",
    ...place({ left: 13, top: 13, width: 1313, height: 996 }, { left: 0, top: 0, width: 0, height: 0 }, { left: 13, top: 13, width: 1313, height: 996 }),
    expected: {
      placements: spots([["origin", 13, 13, 1313, 996]]),
      nonEmpty: true,
      positive: true,
      inside: true
    }
  },
  {
    row: "RW3_hostile_nan_work_area_is_unknown",
    ...place({ left: 13, top: 13, width: 1313, height: 996 }, { left: "x", top: Number.NaN, width: null, height: {} }, { left: 13, top: 13, width: 1313, height: 996 }),
    expected: {
      placements: spots([["origin", 13, 13, 1313, 996]]),
      nonEmpty: true,
      positive: true,
      inside: true
    }
  },
  // RW4: create loop
  {
    row: "RW4_first_spot_rejected_next_spot_is_tried",
    ...create(threeSpots, ["reject", 99]),
    expected: {
      outcome: "created",
      index: 1,
      windowId: 99,
      calls: 2,
      options: JSON.stringify({ tabId: 7, focused: false, state: "normal", type: "normal", left: 53, top: 0, width: 1313, height: 864 })
    }
  },
  {
    row: "RW4_neighbour_first_spot_accepted_one_call",
    ...create(threeSpots, [42]),
    expected: {
      outcome: "created",
      index: 0,
      windowId: 42,
      calls: 1,
      options: JSON.stringify({ tabId: 7, focused: false, state: "normal", type: "normal", left: 223, top: 0, width: 1313, height: 864 })
    }
  },
  {
    row: "RW4_all_spots_rejected_error_names_count_and_chrome_reason",
    ...create(threeSpots, ["reject", "reject", "reject"], ["tried 3", "Bounds must be at least 50%"]),
    expected: { outcome: "error", calls: 3, mentions: true }
  },
  {
    row: "RW4_empty_spot_list_error_names_zero",
    ...create([], [], ["tried 0"]),
    expected: { outcome: "error", calls: 0, mentions: true }
  }
];

test("page_screenshot_render_window_on_screen_table_123", async () => {
  const ctx = load();
  const failures = [];
  for (const { row, run, expected } of rows) {
    let got;
    try {
      got = await run(ctx);
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
  assert.equal(failures.length, 0, `render window on-screen table rows failed:\n${failures.join("\n")}`);
});
