// #123: pageScreenshot cleanup must restore only the focus/active-tab state the
// capture itself changed; never refocus a window the user moved to meanwhile.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const source = readFileSync(new URL("../service_worker.js", import.meta.url), "utf8");
const start = source.indexOf("async function restorePageScreenshotForeground(");
assert.ok(start >= 0, "restorePageScreenshotForeground missing");
const end = source.indexOf("\nasync function ", start + 10);
const code = source.slice(start, end > start ? end : undefined);

function harness() {
  const calls = [];
  const context = vm.createContext({
    console: { warn() {} },
    chrome: {
      tabs: { update: async (id, props) => { calls.push(["tabs.update", id, props]); } },
      windows: { update: async (id, props) => { calls.push(["windows.update", id, props]); } }
    },
    waitForTabActiveState: async () => true,
    waitForChromeWindowFocused: async () => true,
    errorMessage: (e) => String(e)
  });
  vm.runInContext(code, context);
  return { context, calls };
}

const plain = (v) => JSON.parse(JSON.stringify(v));

const base = {
  selectedTabId: 10,
  windowId: 1,
  previousActiveTabId: 10,
  previousFocusedWindowId: 1,
  activatedTabForCapture: false,
  focusChangedByCapture: false,
  beforeActive: true,
  beforeWindowFocused: true
};

test("backgroundSafe capture: user moved focus to another window mid-capture is not refocused", async () => {
  const { context, calls } = harness();
  // Focus was already on the capture window; preflight raced and saw window 2 (user moved A->B).
  const r = await context.restorePageScreenshotForeground({ ...base, previousFocusedWindowId: 2 });
  assert.deepEqual(plain(calls), []);
  assert.equal(r.restoredPreviousWindowFocus, true);
  assert.equal(r.restoredPreviousActive, true);
});

test("capture that itself focused the window restores the previous focused window", async () => {
  const { context, calls } = harness();
  const r = await context.restorePageScreenshotForeground({
    ...base, previousFocusedWindowId: 2, focusChangedByCapture: true, beforeWindowFocused: false
  });
  assert.deepEqual(plain(calls), [["windows.update", 2, { focused: true }]]);
  assert.equal(r.restoredPreviousWindowFocus, true);
});

test("capture that itself activated the tab restores the previous active tab", async () => {
  const { context, calls } = harness();
  const r = await context.restorePageScreenshotForeground({
    ...base, previousActiveTabId: 7, activatedTabForCapture: true, beforeActive: false
  });
  assert.deepEqual(plain(calls), [["tabs.update", 7, { active: true }]]);
  assert.equal(r.restoredPreviousActive, true);
});

test("unchanged tab activation is never re-applied", async () => {
  const { context, calls } = harness();
  await context.restorePageScreenshotForeground({ ...base, previousActiveTabId: 7 });
  assert.deepEqual(plain(calls), []);
});

test("no previous focused window: nothing to restore, reported as not restored", async () => {
  const { context, calls } = harness();
  const r = await context.restorePageScreenshotForeground({
    ...base, previousFocusedWindowId: null, focusChangedByCapture: true, beforeWindowFocused: false
  });
  assert.deepEqual(plain(calls), []);
  assert.equal(r.restoredPreviousWindowFocus, false);
});
