// MY TDD contract for the family adapter dispatch inside the shared worker:
// ensureBridgeConfig() selects BRIDGE_ADAPTER; the registration token comes
// from the adapter's loader when it has one (gecko: Firefox managed storage)
// and from the packaged bridge-auth.json otherwise (chromium, unchanged);
// commands/startup work behind a capability the adapter does not advertise
// (cdpInput, externalPopupRiskSuppression) is refused or skipped, never run.
// The real worker functions are sliced from service_worker.js and run in a vm
// context with stubbed browser APIs.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

import { CHROMIUM_ADAPTER, GECKO_ADAPTER, adapterForFamily } from "../adapters/index.js";

const source = readFileSync(new URL("../service_worker.js", import.meta.url), "utf8");
const registrationToken = "cd".repeat(32);

function sliceFunction(name) {
  let start = source.indexOf(`\nasync function ${name}(`);
  if (start < 0) {
    start = source.indexOf(`\nfunction ${name}(`);
  }
  assert.ok(start >= 0, `${name} missing from service_worker.js`);
  const end = source.indexOf("\n}\n", start);
  assert.ok(end > start, `${name} end missing`);
  return source.slice(start, end + 2);
}

function run(names, globals) {
  const context = vm.createContext({ ...globals });
  vm.runInContext(names.map(sliceFunction).join("\n"), context);
  return context;
}

const bridgeErrorGlobals = {
  ERROR_BROWSER_BRIDGE_CAPABILITY_UNSUPPORTED: "BROWSER_BRIDGE_CAPABILITY_UNSUPPORTED",
  ERROR_CHROME_DOM_ACTION_UNSUPPORTED: "CHROME_DOM_ACTION_UNSUPPORTED",
  ERROR_DEBUGGER_WARNING_UNSUPPRESSED: "A11Y_CDP_DEBUGGER_WARNING_UNSUPPRESSED"
};

test("must_pass_chromium_registration_token_path_unchanged", async () => {
  assert.equal(CHROMIUM_ADAPTER.loadRegistrationToken, undefined);
  assert.ok(CHROMIUM_ADAPTER.capabilities.includes("cdpInput"));
  assert.ok(CHROMIUM_ADAPTER.capabilities.includes("externalPopupRiskSuppression"));
  assert.match(source, /\nlet BRIDGE_ADAPTER = CHROMIUM_ADAPTER;\n/);

  const requests = [];
  const context = run(["loadBridgeRegistrationToken"], {
    BRIDGE_ADAPTER: CHROMIUM_ADAPTER,
    chrome: {
      runtime: { getURL: (path) => `chrome-extension://bridge/${path}` },
      storage: { managed: { get: async () => assert.fail("chromium must not read managed storage") } }
    },
    fetch: async (url, init) => {
      requests.push({ url, init });
      return { ok: true, json: async () => ({ registration_token: registrationToken }) };
    }
  });
  assert.equal(await context.loadBridgeRegistrationToken(), registrationToken);
  assert.equal(requests.length, 1);
  assert.equal(requests[0].url, "chrome-extension://bridge/bridge-auth.json");
});

test("must_pass_gecko_adapter_token_loader_used", async () => {
  const requests = [];
  const managedReads = [];
  const fetchImpl = async (url) => {
    requests.push(url);
    return { ok: true, json: async () => ({ registration_token: "ab".repeat(32) }) };
  };
  const context = run(["loadBridgeRegistrationToken"], {
    BRIDGE_ADAPTER: GECKO_ADAPTER,
    chrome: {
      runtime: { getURL: (path) => `moz-extension://bridge/${path}` },
      storage: {
        managed: {
          get: async (key) => {
            managedReads.push(key);
            return { registrationToken };
          }
        }
      }
    },
    fetch: fetchImpl
  });
  assert.equal(await context.loadBridgeRegistrationToken(), registrationToken);
  assert.deepEqual(managedReads, ["registrationToken"]);
  assert.equal(requests.length, 0, "gecko never reads a packaged bridge-auth.json");

  // Managed storage unavailable: the gecko loader fails closed, still no packaged fallback.
  const noManaged = run(["loadBridgeRegistrationToken"], {
    BRIDGE_ADAPTER: GECKO_ADAPTER,
    chrome: { runtime: { getURL: (path) => `moz-extension://bridge/${path}` } },
    fetch: fetchImpl
  });
  await assert.rejects(noManaged.loadBridgeRegistrationToken(), /storage\.managed is unavailable/);
  assert.equal(requests.length, 0);
});

test("must_pass_ensure_bridge_config_selects_the_family_adapter", async () => {
  for (const [family, adapter] of [["gecko", GECKO_ADAPTER], ["chromium", CHROMIUM_ADAPTER]]) {
    const context = run(["ensureBridgeConfig"], {
      bridgeConfig: null,
      BRIDGE_ADAPTER: CHROMIUM_ADAPTER,
      COMMAND_CAPABILITIES: CHROMIUM_ADAPTER.capabilities,
      adapterForFamily,
      fetch: async () => assert.fail("loadBridgeConfig is stubbed"),
      chrome: { runtime: { getURL: (path) => path } },
      loadBridgeConfig: async () => ({
        family,
        expectedExtensionId: "leoocgnkjnplbfdbklajepahofecgfbk",
        browserKind: family === "gecko" ? "firefox" : "chrome",
        tabTargetPrefix: family === "gecko" ? "firefox-tab:" : "chrome-tab:",
        windowTitleSuffixes: [],
        protectedExtensionIds: [],
        policyRoot: null
      })
    });
    await context.ensureBridgeConfig();
    assert.equal(context.BRIDGE_ADAPTER, adapter, family);
    assert.equal(context.COMMAND_CAPABILITIES, adapter.capabilities, family);
  }
});

test("must_fail_cdp_input_refused_when_not_advertised", async () => {
  const activations = [];
  const globals = {
    ...bridgeErrorGlobals,
    activateTabForCdpTouch: async () => {
      activations.push("activate");
      throw new Error("sentinel: reached the CDP input path");
    }
  };
  const gecko = run(["handleCdpInputText", "bridgeError"], {
    ...globals,
    COMMAND_CAPABILITIES: GECKO_ADAPTER.capabilities
  });
  for (const action of ["type_text", "set_text"]) {
    await assert.rejects(
      gecko.handleCdpInputText({ tabId: 7, target: "firefox-tab:7" }, action, { text: "hi" }, {}, null, 1000, false, 0),
      (error) => error.code === "BROWSER_BRIDGE_CAPABILITY_UNSUPPORTED" && /cdpInput/.test(error.message)
    );
  }
  // Refused before any validation or tab activation, even for empty text.
  await assert.rejects(
    gecko.handleCdpInputText({ tabId: 7 }, "type_text", { text: "" }, {}, null, 1000, false, 0),
    (error) => error.code === "BROWSER_BRIDGE_CAPABILITY_UNSUPPORTED"
  );
  assert.equal(activations.length, 0);

  // Chromium (advertises cdpInput): unchanged, reaches the CDP input path.
  const chromium = run(["handleCdpInputText", "bridgeError"], {
    ...globals,
    COMMAND_CAPABILITIES: CHROMIUM_ADAPTER.capabilities
  });
  await assert.rejects(
    chromium.handleCdpInputText({ tabId: 7 }, "type_text", { text: "hi" }, {}, null, 1000, false, 0),
    /sentinel: reached the CDP input path/
  );
  assert.equal(activations.length, 1);
});

test("must_fail_popup_suppression_never_runs_when_not_advertised", async () => {
  const calls = [];
  const globals = {
    ...bridgeErrorGlobals,
    commandRequiresExternalPopupSuppression: () => true,
    ensureExternalPopupRiskSuppression: async (reason) => {
      calls.push(reason);
      return { ok: false, remaining_hazard_count: 1, hazard_count: 1, status: "hazards_remaining" };
    },
    formatPopupRiskSuppressionForError: () => "suppression=stubbed",
    ensureReconnectWakeAlarm: () => calls.push("alarm"),
    loadMaintenanceReconnectPause: async () => calls.push("pause"),
    connectDaemon: () => calls.push("connect"),
    disableBridgePermanently: (detail) => assert.fail(`unexpected disable: ${detail}`),
    ensureBridgeConfig: async () => ({}),
    EXPECTED_EXTENSION_ID: "leoocgnkjnplbfdbklajepahofecgfbk",
    chrome: { runtime: { id: "leoocgnkjnplbfdbklajepahofecgfbk" } }
  };
  const names = ["requireExternalPopupRisksSuppressed", "startBridge", "bridgeError"];

  const gecko = run(names, { ...globals, COMMAND_CAPABILITIES: GECKO_ADAPTER.capabilities });
  await gecko.requireExternalPopupRisksSuppressed("tabs.list", { hwnd: 1 });
  await gecko.startBridge();
  assert.deepEqual(calls, ["alarm", "pause", "connect"]);

  // Chromium (advertises externalPopupRiskSuppression): unchanged.
  calls.length = 0;
  const chromium = run(names, { ...globals, COMMAND_CAPABILITIES: CHROMIUM_ADAPTER.capabilities });
  await assert.rejects(
    chromium.requireExternalPopupRisksSuppressed("tabs.list", { hwnd: 1 }),
    (error) => error.code === "A11Y_CDP_DEBUGGER_WARNING_UNSUPPRESSED"
  );
  await chromium.startBridge();
  assert.deepEqual(calls, ["command:tabs.list", "alarm", "pause", "startup", "connect"]);
});
