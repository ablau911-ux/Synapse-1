// #123 (re-review): drive the REAL `handlePageScreenshot` (plus its real
// `restorePageScreenshotForeground` cleanup) through focus races, not only the
// extracted cleanup helper. Only the Chrome APIs and the page-script/IO helpers
// around the handler are stubbed; every focus/activation decision is the
// production code's.
//
// Contract: a backgroundSafe screenshot never activates a tab or focuses a
// window, refuses before any setup when the tab is inactive or Chrome is not
// focused, aborts (without re-activating) when the tab stops being active, and
// never refocuses a window the user moved to meanwhile (A -> B -> app C).
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const source = readFileSync(new URL("../service_worker.js", import.meta.url), "utf8");
const start = source.indexOf("async function handlePageScreenshot(");
const end = source.indexOf("function normalizePageScreenshotRequest(", start);
assert.ok(start >= 0 && end > start, "handlePageScreenshot / restore slice missing");
const handlerCode = source.slice(start, end);
assert.ok(handlerCode.includes("async function restorePageScreenshotForeground("), "restore in slice");

/** env: tabActive, windowFocused, focusedWindowIds (per call), activeTabIds (per call), onCapture */
function harness(env) {
  const calls = [];
  const next = (list) => (list.length > 1 ? list.shift() : list[0]);
  const context = vm.createContext({
    console: { warn() {} },
    PAGE_SCREENSHOT_COMMAND_RESPONSE_BUDGET_MS: 30000,
    ERROR_AXTREE_FAILED: "A11Y_CDP_AXTREE_FAILED",
    ERROR_CHROME_SCRIPTING_EXECUTE_FAILED: "CHROME_SCRIPTING_EXECUTE_FAILED",
    ERROR_EXTENSION_TIMEOUT: "EXTENSION_TIMEOUT",
    ERROR_ATTACH_FAILED: "A11Y_CDP_ATTACH_FAILED",
    bridgeError: (code, message) => Object.assign(new Error(message), { code }),
    errorMessage: (error) => String(error?.message ?? error),
    selectTabTarget: async () => ({
      tabId: 10, target: { id: "T10" }, targetCandidateCount: 1, selectionReason: "explicit"
    }),
    normalizePageScreenshotRequest: () => ({
      waitTimeoutMs: 5000, captureDelayMs: 0, format: "png", quality: 90,
      omitBackground: false, scope: "viewport"
    }),
    tabPageState: async () => ({
      chrome_window_id: 1, active: env.tabActive, target_id: "T10", url: "u", title: "t"
    }),
    tabPageTextState: async () => ({ text_length: 0 }),
    activeTabForWindow: async () => ({ id: next(env.activeTabIds) }),
    chromeWindowState: async (id) => ({ id, focused: env.windowFocused }),
    focusedChromeWindow: async () => {
      const id = next(env.focusedWindowIds);
      return id === null ? null : { id };
    },
    runPageScreenshotSetup: async () => {
      calls.push(["setup"]);
      return {
        ok: true, clip_css: { x: 0, y: 0, w: 100, h: 80 },
        metrics: { device_pixel_ratio: 1, scroll_width: 100, scroll_height: 80, viewport_width: 100, viewport_height: 80 },
        before_scroll_x: 0, before_scroll_y: 0, background_restore: null, mask_count: 0, masks: []
      };
    },
    pageScreenshotTilePositions: () => [{ x: 0, y: 0 }],
    remainingPageScreenshotBudgetMs: () => 1000,
    runPageScreenshotScroll: async () => ({ scroll_x: 0, scroll_y: 0, viewport_width: 100, viewport_height: 80 }),
    sleep: async () => {},
    captureVisibleTabWithQuota: async () => {
      calls.push(["captureVisibleTab"]);
      env.onCapture?.();
      return "data:image/png;base64,AAAA";
    },
    runPageScreenshotCleanup: async () => { calls.push(["cleanup"]); },
    waitForTabActiveState: async () => true,
    waitForChromeWindowFocused: async () => true,
    chrome: {
      runtime: { id: "ext-123" },
      tabs: { update: async (id, props) => { calls.push(["tabs.update", id, props]); } },
      windows: { update: async (id, props) => { calls.push(["windows.update", id, props]); } }
    }
  });
  vm.runInContext(handlerCode, context);
  return { context, calls };
}

const plain = (value) => JSON.parse(JSON.stringify(value));
const focusCalls = (calls) => plain(calls).filter(([name]) => name === "tabs.update" || name === "windows.update");
const safe = { backgroundSafe: true, targetId: "T10" };

test("must_pass active focused backgroundSafe capture changes no tab or window focus", async () => {
  const { context, calls } = harness({
    tabActive: true, windowFocused: true, focusedWindowIds: [1], activeTabIds: [10]
  });
  const result = await context.handlePageScreenshot(safe);
  assert.equal(result.tile_count, 1);
  assert.equal(result.required_foreground, false);
  assert.equal(result.active_for_capture, true);
  assert.deepEqual(focusCalls(calls), []);
  assert.deepEqual(plain(calls).map(([name]) => name), ["setup", "captureVisibleTab", "cleanup"]);
});

test("must_fail user moves Chrome A->B between preflight reads then to app C: nothing is refocused", async () => {
  const env = {
    tabActive: true, windowFocused: true, focusedWindowIds: [2], activeTabIds: [10],
    onCapture: () => { env.focusedWindowIds = [null]; }
  };
  const { context, calls } = harness(env);
  const result = await context.handlePageScreenshot(safe);
  assert.equal(result.previous_focused_window_id, 2, "precondition: preflight raced to window B");
  assert.deepEqual(focusCalls(calls), [], "#123: cleanup must not focus stale window B");
});

test("must_fail inactive tab is refused before setup with FOREGROUND_ACTIVATION_REFUSED", async () => {
  const { context, calls } = harness({
    tabActive: false, windowFocused: true, focusedWindowIds: [1], activeTabIds: [7]
  });
  await assert.rejects(context.handlePageScreenshot(safe), (error) => error.code === "FOREGROUND_ACTIVATION_REFUSED");
  assert.deepEqual(plain(calls), [], "no setup, no capture, no focus change");
});

test("must_fail unfocused Chrome window is refused before setup with FOREGROUND_ACTIVATION_REFUSED", async () => {
  const { context, calls } = harness({
    tabActive: true, windowFocused: false, focusedWindowIds: [5], activeTabIds: [10]
  });
  await assert.rejects(context.handlePageScreenshot(safe), (error) => error.code === "FOREGROUND_ACTIVATION_REFUSED");
  assert.deepEqual(plain(calls), []);
});

test("must_fail tab switched away mid-capture aborts with ACTION_POSTCONDITION_FAILED and is not re-activated", async () => {
  const { context, calls } = harness({
    tabActive: true, windowFocused: true, focusedWindowIds: [1], activeTabIds: [10, 11]
  });
  await assert.rejects(context.handlePageScreenshot(safe), (error) => error.code === "ACTION_POSTCONDITION_FAILED");
  assert.deepEqual(focusCalls(calls), [], "#123: the user's new tab must stay active");
  assert.deepEqual(plain(calls).map(([name]) => name), ["setup", "cleanup"], "page cleanup still runs");
});
