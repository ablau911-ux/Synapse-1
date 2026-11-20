// Frozen behaviour table (manager-owned, work method v3; upstream #2265
// browser interoperability). Other extensions belong to the owner: the bridge
// observes debugger/nativeMessaging extensions and never disables them; a
// command fails closed only when the extension list could not be read.
// Codex and Opus never edit this file. One case per L row.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const BRIDGE_ID = "leoocgnkjnplbfdbklajepahofecgfbk";
const EXTERNAL_ID = "aaaabbbbccccddddeeeeffffgggghhhh";
const PROTECTED_ID = "mpognobbkildjkofajifpdfhcoklimli";

const source = readFileSync(new URL("../service_worker.js", import.meta.url), "utf8");
const start = source.indexOf("function externalPopupRiskAllowsCommand(");
const end = source.indexOf("\nfunction popupRiskSuppressionSnapshot(", start);
assert.ok(start >= 0 && end > start, "external popup risk functions missing");
const code = source.slice(start, end);

function external(overrides = {}) {
  return {
    id: EXTERNAL_ID,
    name: "Owner debugger extension",
    type: "extension",
    installType: "normal",
    enabled: true,
    mayDisable: true,
    permissions: ["debugger", "tabs"],
    ...overrides
  };
}

function harness(items, { management = true } = {}) {
  const list = items;
  const setEnabledCalls = [];
  const chrome = { runtime: { id: BRIDGE_ID } };
  if (management) {
    chrome.management = {
      getAll: async () => list.map((item) => (item && typeof item === "object" ? { ...item } : item)),
      setEnabled: async (id, enabled) => {
        setEnabledCalls.push({ id, enabled });
        const item = list.find((entry) => entry?.id === id);
        if (item) {
          item.enabled = enabled;
        }
      }
    };
  }
  const context = vm.createContext({
    console: { warn() {}, log() {}, error() {} },
    EXTERNAL_POPUP_RISK_PERMISSIONS: Object.freeze(["debugger", "nativeMessaging"]),
    PROTECTED_EXTENSION_IDS: Object.freeze([PROTECTED_ID]),
    popupRiskSuppressionState: null,
    errorMessage: (error) => String(error?.message ?? error),
    formatPopupRiskSuppressionForError: () => "",
    chrome
  });
  vm.runInContext(code, context);
  return { context, list, setEnabledCalls };
}

const rows = [
  {
    row: "i5a_enabled_external_debugger_extension_is_never_disabled",
    run: async () => {
      const { context, setEnabledCalls } = harness([external()]);
      const state = await context.refreshExternalPopupRiskSuppression("table");
      return {
        set_enabled_calls: setEnabledCalls.length,
        status: state.status,
        remaining_hazard_count: state.remaining_hazard_count,
        disabled_count: state.disabled_count,
        allowed: context.externalPopupRiskAllowsCommand(state)
      };
    },
    expected: { set_enabled_calls: 0, status: "observed", remaining_hazard_count: 1, disabled_count: 0, allowed: true }
  },
  {
    row: "i5b_empty_extension_list_is_clear_and_allowed",
    run: async () => {
      const { context, setEnabledCalls } = harness([]);
      const state = await context.refreshExternalPopupRiskSuppression("table");
      return { set_enabled_calls: setEnabledCalls.length, status: state.status, allowed: context.externalPopupRiskAllowsCommand(state) };
    },
    expected: { set_enabled_calls: 0, status: "clear", allowed: true }
  },
  {
    row: "i5c_management_api_unavailable_is_unknown_and_refused",
    run: async () => {
      const { context } = harness([], { management: false });
      const state = await context.refreshExternalPopupRiskSuppression("table");
      return { status: state.status, management_available: state.management_available, allowed: context.externalPopupRiskAllowsCommand(state) };
    },
    expected: { status: "management_unavailable", management_available: false, allowed: false }
  },
  {
    row: "i5d_hostile_metadata_never_mutates_and_never_throws",
    run: async () => {
      const { context, setEnabledCalls } = harness([
        null,
        { id: "" },
        { id: 123, enabled: "yes", permissions: "debugger" },
        external({ id: "x", mayDisable: "nope", permissions: ["debugger", 7, null] })
      ]);
      const state = await context.refreshExternalPopupRiskSuppression("table");
      return { set_enabled_calls: setEnabledCalls.length, failure_count: state.failure_count, allowed: context.externalPopupRiskAllowsCommand(state) };
    },
    expected: { set_enabled_calls: 0, failure_count: 0, allowed: true }
  },
  {
    row: "i5e_may_disable_false_extension_is_observed_not_a_failure",
    run: async () => {
      const { context, setEnabledCalls } = harness([external({ mayDisable: false, installType: "admin" })]);
      const state = await context.refreshExternalPopupRiskSuppression("table");
      return { set_enabled_calls: setEnabledCalls.length, failure_count: state.failure_count, allowed: context.externalPopupRiskAllowsCommand(state) };
    },
    expected: { set_enabled_calls: 0, failure_count: 0, allowed: true }
  },
  {
    row: "i5f_second_actor_keeps_extension_enabled_rescan_never_disables",
    run: async () => {
      const { context, list, setEnabledCalls } = harness([external({ permissions: ["nativeMessaging"] })]);
      await context.refreshExternalPopupRiskSuppression("startup");
      list[0].enabled = true; // the owner (second actor) turns it back on
      const state = await context.refreshExternalPopupRiskSuppression("management_event");
      return { set_enabled_calls: setEnabledCalls.length, enabled_after: list[0].enabled, allowed: context.externalPopupRiskAllowsCommand(state) };
    },
    expected: { set_enabled_calls: 0, enabled_after: true, allowed: true }
  },
  {
    row: "i5g_someone_elses_browser_ui_extension_untouched_and_unlisted",
    run: async () => {
      const { context, setEnabledCalls } = harness([external({ id: PROTECTED_ID, type: "hosted_app" })]);
      const state = await context.refreshExternalPopupRiskSuppression("table");
      return { set_enabled_calls: setEnabledCalls.length, hazard_count: state.hazard_count };
    },
    expected: { set_enabled_calls: 0, hazard_count: 0 }
  },
  {
    row: "i5h_neighbour_scan_error_state_refuses",
    run: async () => {
      const { context } = harness([]);
      const allowed = context.externalPopupRiskAllowsCommand({
        ok: false,
        status: "suppression_error",
        management_available: true,
        remaining_hazard_count: 0,
        failure_count: 1
      });
      return { allowed };
    },
    expected: { allowed: false }
  },
  {
    row: "i5h_hostile_missing_state_refuses",
    run: async () => {
      const { context } = harness([]);
      return { allowed: context.externalPopupRiskAllowsCommand(undefined) };
    },
    expected: { allowed: false }
  },
  {
    row: "i5i_neighbour_own_bridge_extension_never_listed",
    run: async () => {
      const { context, setEnabledCalls } = harness([external({ id: BRIDGE_ID })]);
      const state = await context.refreshExternalPopupRiskSuppression("table");
      return { set_enabled_calls: setEnabledCalls.length, hazard_count: state.hazard_count };
    },
    expected: { set_enabled_calls: 0, hazard_count: 0 }
  }
];

test("external_extension_interop_table_interop", async () => {
  const failures = [];
  for (const { row, run, expected } of rows) {
    let got;
    try {
      got = await run();
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
  assert.equal(failures.length, 0, `interop table rows failed:\n${failures.join("\n")}`);
});
