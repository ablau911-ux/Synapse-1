// MY TDD contract for bridge-config.json (multi-browser bridge foundation):
// the config drives the expected extension id, tab prefix, title suffixes
// and family adapter; without a valid config the worker stays dormant and
// never sends a registration request.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import { CHROMIUM_ADAPTER, GECKO_ADAPTER, adapterForFamily } from "../adapters/index.js";
import {
  BRIDGE_CONFIG_FILE,
  loadBridgeConfig,
  parseBridgeConfig,
  stripWindowTitleSuffix
} from "../bridge_config.js";

const chromeConfig = Object.freeze({
  schemaVersion: 1,
  browserKind: "chrome",
  family: "chromium",
  expectedExtensionId: "leoocgnkjnplbfdbklajepahofecgfbk",
  tabTargetPrefix: "chrome-tab:",
  windowTitleSuffixes: [" - Google Chrome"],
  protectedExtensionIds: [],
  policyRoot: "Software\\Policies\\Google\\Chrome"
});

function packageFetch(responses) {
  const requests = [];
  const fetchImpl = async (url, init) => {
    requests.push({ url, init });
    const body = responses[url];
    if (body === undefined) {
      return { ok: false, json: async () => ({}) };
    }
    return { ok: true, json: async () => body };
  };
  return { requests, fetchImpl };
}

const getUrl = (path) => `chrome-extension://bridge/${path}`;

test("must_pass_sw_reads_bridge_config", async () => {
  const { requests, fetchImpl } = packageFetch({ [getUrl(BRIDGE_CONFIG_FILE)]: chromeConfig });
  const config = await loadBridgeConfig(fetchImpl, getUrl);
  assert.equal(requests.length, 1);
  assert.equal(requests[0].url, "chrome-extension://bridge/bridge-config.json");
  assert.equal(requests[0].init.cache, "no-store");
  assert.equal(config.browserKind, "chrome");
  assert.equal(config.expectedExtensionId, "leoocgnkjnplbfdbklajepahofecgfbk");
  assert.equal(config.tabTargetPrefix, "chrome-tab:");
  assert.deepEqual([...config.windowTitleSuffixes], [" - Google Chrome"]);
  assert.equal(adapterForFamily(config.family), CHROMIUM_ADAPTER);
  assert.ok(Object.isFrozen(config));

  const edge = parseBridgeConfig({
    ...chromeConfig,
    browserKind: "edge",
    tabTargetPrefix: "edge-tab:",
    windowTitleSuffixes: [" - Microsoft​ Edge", " - Microsoft Edge"],
    policyRoot: null
  });
  assert.equal(edge.tabTargetPrefix, "edge-tab:");
  assert.equal(edge.policyRoot, null);
});

test("must_fail_sw_missing_bridge_config_stays_dormant", async () => {
  const { requests, fetchImpl } = packageFetch({});
  await assert.rejects(loadBridgeConfig(fetchImpl, getUrl), /bridge-config\.json is unavailable/);
  assert.equal(requests.length, 1, "only the package file is read; nothing reaches the daemon");
  assert.ok(requests.every((request) => !request.url.startsWith("http://127.0.0.1")));
});

test("must_fail_invalid_bridge_config_fields", () => {
  for (const broken of [
    null,
    [],
    { ...chromeConfig, schemaVersion: 2 },
    { ...chromeConfig, browserKind: "Chrome Browser" },
    { ...chromeConfig, family: "webkit" },
    { ...chromeConfig, tabTargetPrefix: "chrome:" },
    { ...chromeConfig, expectedExtensionId: "" },
    { ...chromeConfig, windowTitleSuffixes: [] },
    { ...chromeConfig, protectedExtensionIds: "x" }
  ]) {
    assert.throws(() => parseBridgeConfig(broken), undefined, JSON.stringify(broken));
  }
});

test("must_pass_gecko_stub_adapter_advertises_no_capabilities", () => {
  // The gecko capability list itself is pinned by firefox_adapter_contract.test.mjs.
  assert.equal(adapterForFamily("gecko"), GECKO_ADAPTER);
  assert.throws(() => adapterForFamily("webkit"));
});

test("must_pass_title_suffix_strip_matches_legacy_chrome_regex", () => {
  const legacy = (title) => title.replace(/\s+-\s+google chrome$/, "").trim();
  for (const title of ["inbox - google chrome", "inbox", "a - google chrome - google chrome"]) {
    assert.equal(stripWindowTitleSuffix(title, [" - Google Chrome"]), legacy(title));
  }
  assert.equal(
    stripWindowTitleSuffix("inbox - microsoft​ edge", [" - Microsoft​ Edge"]),
    "inbox"
  );
});

test("must_pass_chromium_adapter_is_a_pure_move_of_the_capability_list", () => {
  const source = readFileSync(new URL("../service_worker.js", import.meta.url), "utf8");
  assert.ok(!source.includes('const COMMAND_CAPABILITIES = Object.freeze(['));
  assert.ok(!source.includes('const EXPECTED_EXTENSION_ID = "'));
  assert.ok(!source.includes('const TAB_TARGET_PREFIX = "chrome-tab:"'));
  assert.ok(source.includes("browserKind: BROWSER_KIND"));
  for (const capability of ["listTabs", "evaluateScript", "reloadSelf", "setFieldValue"]) {
    assert.ok(CHROMIUM_ADAPTER.capabilities.includes(capability), capability);
  }
  assert.equal(CHROMIUM_ADAPTER.capabilities.length, 51);
});
