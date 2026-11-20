// Gecko family adapter (Firefox). Firefox implements the WebExtension tabs,
// scripting, webNavigation, cookies, downloads, storage, alarms and
// tabs.captureVisibleTab APIs but no chrome.debugger and no
// management.setEnabled for regular extensions, so every command whose
// shared-worker handler needs CDP is not advertised; the daemon refuses those
// per command (BROWSER_BRIDGE_CAPABILITY_UNSUPPORTED) from
// browsers/firefox.json unsupported_capabilities, which must list the same
// commands (tests/firefox_adapter_contract.test.mjs pins the two together).
//
// The registration token never ships inside the extension package (the XPI is
// uploaded to AMO for signing): the installer delivers it through Firefox
// managed storage (HKCU\Software\Mozilla\ManagedStorage\<gecko id>), read here
// from browser.storage.managed.

import { CHROMIUM_ADAPTER } from "./chromium.js";

export const GECKO_UNSUPPORTED_CAPABILITIES = Object.freeze([
  "evaluateScript",
  "initScript",
  "exposeBinding",
  "handleDialog",
  "fileUpload",
  "cdpInput",
  "typeActiveElement",
  "viewportEmulation",
  "deviceEmulation",
  "geolocationEmulation",
  "localeEmulation",
  "mediaEmulation",
  "networkConditions",
  "pagePdf",
  "externalPopupRiskSuppression"
]);

// Key of the registration token in the managed storage manifest `data`
// (scripts\install-synapse-gecko-bridge.ps1 writes the same key).
export const GECKO_MANAGED_TOKEN_KEY = "registrationToken";

const REGISTRATION_TOKEN_PATTERN = /^[0-9a-f]{64}$/;
const GECKO_EXTENSION_ORIGIN_PREFIX = "moz-extension://";

// Reads the registration token from Firefox managed storage. Throws (the
// worker stays dormant) when managed storage is unavailable or the token is
// missing or malformed; never falls back to a packaged file.
export async function loadGeckoRegistrationToken(managedStorageArea) {
  if (!managedStorageArea || typeof managedStorageArea.get !== "function") {
    throw new Error(
      "Synapse bridge browser.storage.managed is unavailable; rerun scripts\\install-synapse-gecko-bridge.ps1 and restart Firefox"
    );
  }
  const stored = await managedStorageArea.get(GECKO_MANAGED_TOKEN_KEY);
  const token = stored && typeof stored === "object" ? stored[GECKO_MANAGED_TOKEN_KEY] : undefined;
  if (token === undefined || token === null || token === "") {
    throw new Error(
      "Synapse bridge registration credential is missing from Firefox managed storage; rerun scripts\\install-synapse-gecko-bridge.ps1 and restart Firefox"
    );
  }
  if (typeof token !== "string" || !REGISTRATION_TOKEN_PATTERN.test(token)) {
    throw new Error(
      "Synapse bridge registration credential is invalid in Firefox managed storage; rerun scripts\\install-synapse-gecko-bridge.ps1"
    );
  }
  return token;
}

// Firefox exposes promise-returning APIs on `browser`; under Manifest V2 the
// `chrome` alias is callback-based, while the shared worker awaits chrome.*.
// Inside a moz-extension page only, make `chrome` the `browser` namespace
// (chrome.debugger and chrome.management stay undefined there, which the
// worker already guards). Returns {installed, reason}; never throws.
export function installGeckoNamespace(scope) {
  const browserApi = scope?.browser;
  if (!browserApi || typeof browserApi.runtime?.getURL !== "function") {
    return { installed: false, reason: "browser_namespace_absent" };
  }
  let base = "";
  try {
    base = String(browserApi.runtime.getURL(""));
  } catch (error) {
    return { installed: false, reason: `runtime_get_url_failed:${String(error?.message ?? error)}` };
  }
  if (!base.startsWith(GECKO_EXTENSION_ORIGIN_PREFIX)) {
    return { installed: false, reason: "not_a_gecko_extension_context" };
  }
  if (scope.chrome === browserApi) {
    return { installed: true, reason: "already_aliased" };
  }
  try {
    Object.defineProperty(scope, "chrome", {
      value: browserApi,
      configurable: true,
      enumerable: true,
      writable: true
    });
  } catch (error) {
    return { installed: false, reason: `define_failed:${String(error?.message ?? error)}` };
  }
  return scope.chrome === browserApi
    ? { installed: true, reason: "aliased_browser_namespace" }
    : { installed: false, reason: "alias_not_observed" };
}

export const GECKO_ADAPTER = Object.freeze({
  family: "gecko",
  background: "persistent_background_page",
  registrationTokenSource: "storage_managed",
  loadRegistrationToken: loadGeckoRegistrationToken,
  capabilities: Object.freeze(
    CHROMIUM_ADAPTER.capabilities.filter(
      (capability) => !GECKO_UNSUPPORTED_CAPABILITIES.includes(capability)
    )
  )
});
