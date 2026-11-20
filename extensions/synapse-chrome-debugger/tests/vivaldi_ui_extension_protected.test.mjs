// MY TDD contract for the Vivaldi row (browsers/vivaldi.json): Vivaldi's own
// browser UI is the built-in component extension
// mpognobbkildjkofajifpdfhcoklimli. The shared worker's external popup-risk
// suppression (chrome.management.setEnabled(id, false)) must NEVER disable it.
// The protected id flows row -> bridge-config.json (protectedExtensionIds) ->
// PROTECTED_EXTENSION_IDS; this test drives the real suppression code with the
// config built from the shipped row.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

import { parseBridgeConfig } from "../bridge_config.js";

const VIVALDI_UI_EXTENSION_ID = "mpognobbkildjkofajifpdfhcoklimli";
const BRIDGE_EXTENSION_ID = "leoocgnkjnplbfdbklajepahofecgfbk";
const EXTERNAL_HAZARD_ID = "aaaabbbbccccddddeeeeffffgggghhhh";

const row = JSON.parse(
  readFileSync(
    new URL("../../../crates/synapse-browsers/browsers/vivaldi.json", import.meta.url),
    "utf8"
  )
);

// Mirrors New-SynapseBridgeConfig (scripts/synapse-browser-rows.ps1).
function bridgeConfigFromRow(browserRow) {
  return parseBridgeConfig({
    schemaVersion: 1,
    browserKind: browserRow.id,
    family: browserRow.family,
    expectedExtensionId: browserRow.extension.id,
    tabTargetPrefix: browserRow.tab_target_prefix,
    windowTitleSuffixes: browserRow.window_title_suffixes,
    protectedExtensionIds: browserRow.protected_extension_ids ?? [],
    policyRoot: browserRow.policy ? browserRow.policy.root : null
  });
}

const source = readFileSync(new URL("../service_worker.js", import.meta.url), "utf8");
const start = source.indexOf("async function refreshExternalPopupRiskSuppression(");
assert.ok(start >= 0, "refreshExternalPopupRiskSuppression missing");
const end = source.indexOf("\nfunction popupRiskSuppressionSnapshot(", start);
assert.ok(end > start, "popupRiskSuppressionSnapshot boundary missing");
const suppressionCode = source.slice(start, end);

function managementItems() {
  return [
    {
      // Vivaldi UI: component extension holding the hazard permissions.
      id: VIVALDI_UI_EXTENSION_ID,
      name: "Vivaldi",
      type: "hosted_app",
      installType: "other",
      enabled: true,
      mayDisable: true,
      permissions: ["debugger", "nativeMessaging", "tabs"]
    },
    {
      id: EXTERNAL_HAZARD_ID,
      name: "External debugger extension",
      type: "extension",
      installType: "normal",
      enabled: true,
      mayDisable: true,
      permissions: ["debugger"]
    },
    {
      id: BRIDGE_EXTENSION_ID,
      name: "Synapse bridge",
      type: "extension",
      installType: "development",
      enabled: true,
      mayDisable: true,
      permissions: ["debugger", "tabs"]
    }
  ];
}

async function runSuppression(protectedExtensionIds) {
  const items = managementItems();
  const setEnabledCalls = [];
  const context = vm.createContext({
    console: { warn() {}, log() {} },
    Date,
    JSON,
    Set,
    String,
    Boolean,
    Number,
    Array,
    EXTERNAL_POPUP_RISK_PERMISSIONS: Object.freeze(["debugger", "nativeMessaging"]),
    PROTECTED_EXTENSION_IDS: protectedExtensionIds,
    popupRiskSuppressionState: null,
    errorMessage: (error) => String(error?.message ?? error),
    formatPopupRiskSuppressionForError: () => "",
    chrome: {
      runtime: { id: BRIDGE_EXTENSION_ID },
      management: {
        getAll: async () => items.map((item) => ({ ...item })),
        setEnabled: async (id, enabled) => {
          setEnabledCalls.push({ id, enabled });
          const item = items.find((entry) => entry.id === id);
          if (item) {
            item.enabled = enabled;
          }
        }
      }
    }
  });
  vm.runInContext(suppressionCode, context);
  const state = await context.refreshExternalPopupRiskSuppression("vivaldi_row_test");
  return { state, setEnabledCalls };
}

test("must_pass_vivaldi_row_bridge_config_carries_the_protected_ui_id", () => {
  const config = bridgeConfigFromRow(row);
  assert.equal(config.browserKind, "vivaldi");
  assert.equal(config.tabTargetPrefix, "vivaldi-tab:");
  assert.equal(config.expectedExtensionId, BRIDGE_EXTENSION_ID);
  assert.equal(config.policyRoot, null);
  assert.ok(config.protectedExtensionIds.includes(VIVALDI_UI_EXTENSION_ID));
});

test("must_fail_vivaldi_ui_extension_never_suppressed", async () => {
  const config = bridgeConfigFromRow(row);
  const { state, setEnabledCalls } = await runSuppression(config.protectedExtensionIds);

  assert.ok(
    setEnabledCalls.every((call) => call.id !== VIVALDI_UI_EXTENSION_ID),
    `Vivaldi UI must never be disabled: ${JSON.stringify(setEnabledCalls)}`
  );
  assert.ok(state.hazards.every((hazard) => hazard.id !== VIVALDI_UI_EXTENSION_ID));
  assert.ok(state.remaining_hazards.every((hazard) => hazard.id !== VIVALDI_UI_EXTENSION_ID));
  // Real external hazards are still suppressed; the bridge never disables itself.
  assert.deepEqual(setEnabledCalls, [{ id: EXTERNAL_HAZARD_ID, enabled: false }]);
  assert.equal(state.ok, true);
  assert.equal(state.status, "suppressed");
});

test("must_fail_without_the_row_protection_the_vivaldi_ui_would_be_disabled", async () => {
  // Obstacle: proves the protected list is load-bearing. With an empty list
  // (a row that forgot the UI id) the suppression disables Vivaldi's UI.
  const { setEnabledCalls } = await runSuppression(Object.freeze([]));
  assert.ok(setEnabledCalls.some((call) => call.id === VIVALDI_UI_EXTENSION_ID));
});
