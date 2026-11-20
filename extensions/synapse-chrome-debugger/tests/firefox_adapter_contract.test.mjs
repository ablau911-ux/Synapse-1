// MY TDD contract for the gecko (Firefox) family adapter.
// must_pass_*: the adapter advertises exactly the WebExtension-capable
// commands (global list minus the firefox row's unsupported_capabilities),
// reads the registration token from browser.storage.managed and aliases the
// promise-based browser.* namespace only inside a moz-extension page.
// must_fail_*: a missing/malformed managed token keeps the bridge dormant and
// the adapter never reads a bundled bridge-auth.json.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import { CHROMIUM_ADAPTER, GECKO_ADAPTER, adapterForFamily } from "../adapters/index.js";
import {
  GECKO_MANAGED_TOKEN_KEY,
  GECKO_UNSUPPORTED_CAPABILITIES,
  installGeckoNamespace,
  loadGeckoRegistrationToken
} from "../adapters/gecko.js";

const TOKEN = "0123456789abcdef".repeat(4);
const firefoxRow = JSON.parse(
  readFileSync(new URL("../../../crates/synapse-browsers/browsers/firefox.json", import.meta.url), "utf8")
);

function managedArea(data) {
  const calls = [];
  return {
    calls,
    async get(keys) {
      calls.push(keys);
      return { ...data };
    }
  };
}

test("must_pass_gecko_adapter_omits_debugger_capabilities", () => {
  assert.equal(adapterForFamily("gecko"), GECKO_ADAPTER);
  assert.equal(GECKO_ADAPTER.family, "gecko");
  assert.equal(GECKO_ADAPTER.background, "persistent_background_page");
  assert.ok(Object.isFrozen(GECKO_ADAPTER.capabilities));
  for (const capability of GECKO_UNSUPPORTED_CAPABILITIES) {
    assert.ok(!GECKO_ADAPTER.capabilities.includes(capability), capability);
  }
  for (const capability of ["listTabs", "openTab", "closeTab", "navigateTab", "activateTab",
    "pageContent", "pageScreenshot", "storageState", "downloads", "alarmReconnect", "reloadSelf"]) {
    assert.ok(GECKO_ADAPTER.capabilities.includes(capability), capability);
  }
  assert.deepEqual(
    [...GECKO_ADAPTER.capabilities],
    CHROMIUM_ADAPTER.capabilities.filter((capability) => !GECKO_UNSUPPORTED_CAPABILITIES.includes(capability)),
    "gecko list is the global list minus the unsupported lanes, same order"
  );
});

test("must_pass_gecko_unsupported_list_matches_firefox_row", () => {
  assert.deepEqual(
    [...GECKO_UNSUPPORTED_CAPABILITIES].sort(),
    [...firefoxRow.unsupported_capabilities].sort(),
    "adapter and browsers/firefox.json must declare the same refused lanes"
  );
  for (const capability of GECKO_UNSUPPORTED_CAPABILITIES) {
    assert.ok(CHROMIUM_ADAPTER.capabilities.includes(capability), `${capability} is a real command kind`);
  }
});

test("must_pass_gecko_adapter_reads_token_from_storage_managed", async () => {
  const area = managedArea({ [GECKO_MANAGED_TOKEN_KEY]: TOKEN });
  assert.equal(await loadGeckoRegistrationToken(area), TOKEN);
  assert.deepEqual(area.calls, [GECKO_MANAGED_TOKEN_KEY]);
  assert.equal(GECKO_ADAPTER.registrationTokenSource, "storage_managed");
  assert.equal(GECKO_ADAPTER.loadRegistrationToken, loadGeckoRegistrationToken);
});

test("must_fail_gecko_adapter_token_missing_or_malformed_stays_dormant", async () => {
  await assert.rejects(loadGeckoRegistrationToken(undefined), /storage\.managed is unavailable/);
  await assert.rejects(loadGeckoRegistrationToken({}), /storage\.managed is unavailable/);
  for (const value of [undefined, "", TOKEN.toUpperCase(), `${TOKEN}0`, TOKEN.slice(1), 42]) {
    await assert.rejects(
      loadGeckoRegistrationToken(managedArea({ [GECKO_MANAGED_TOKEN_KEY]: value })),
      /registration credential is (missing|invalid)/,
      String(value)
    );
  }
  const failing = { async get() { throw new Error("Managed storage manifest not found"); } };
  await assert.rejects(loadGeckoRegistrationToken(failing), /Managed storage manifest not found/);
});

test("must_fail_gecko_adapter_never_reads_bundled_bridge_auth", async () => {
  const source = readFileSync(new URL("../adapters/gecko.js", import.meta.url), "utf8");
  assert.ok(!source.includes("bridge-auth.json"));
  assert.ok(!/\bfetch\s*\(/.test(source), "the gecko token path performs no fetch");
  const originalFetch = globalThis.fetch;
  let fetched = 0;
  globalThis.fetch = async () => {
    fetched += 1;
    return { ok: true, json: async () => ({ registration_token: TOKEN }) };
  };
  try {
    await assert.rejects(loadGeckoRegistrationToken(managedArea({})));
    assert.equal(fetched, 0);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

function geckoScope(origin, chromeDescriptor) {
  const browserApi = { runtime: { getURL: (path) => `${origin}${path}` } };
  const scope = { browser: browserApi };
  if (chromeDescriptor) {
    Object.defineProperty(scope, "chrome", chromeDescriptor);
  }
  return { scope, browserApi };
}

test("must_pass_gecko_namespace_alias_only_in_moz_extension_context", () => {
  const callbackChrome = { runtime: {} };
  const { scope, browserApi } = geckoScope("moz-extension://0b6a3f0e-1c2d-4e5f-8a9b-0c1d2e3f4a5b/", {
    configurable: true,
    enumerable: true,
    get: () => callbackChrome
  });
  const outcome = installGeckoNamespace(scope);
  assert.equal(outcome.installed, true);
  assert.equal(scope.chrome, browserApi);
  assert.equal(installGeckoNamespace(scope).reason, "already_aliased");

  const chromium = geckoScope("chrome-extension://leoocgnkjnplbfdbklajepahofecgfbk/", {
    configurable: true,
    value: callbackChrome
  });
  assert.equal(installGeckoNamespace(chromium.scope).installed, false);
  assert.equal(chromium.scope.chrome, callbackChrome, "Chromium (browser namespace 148+) is untouched");

  const nodeScope = {};
  assert.deepEqual(installGeckoNamespace(nodeScope), { installed: false, reason: "browser_namespace_absent" });
  assert.equal("chrome" in nodeScope, false);
});

test("must_fail_gecko_namespace_alias_reports_non_configurable_chrome", () => {
  const { scope } = geckoScope("moz-extension://0b6a3f0e-1c2d-4e5f-8a9b-0c1d2e3f4a5b/", {
    configurable: false,
    value: { runtime: {} }
  });
  const outcome = installGeckoNamespace(scope);
  assert.equal(outcome.installed, false);
  assert.match(outcome.reason, /^define_failed:/);
});
