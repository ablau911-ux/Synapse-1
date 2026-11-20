// Multi-browser bridge: per-browser identity the installer writes beside the
// worker as bridge-config.json (generated from browsers/<id>.json). The shared
// service worker reads it like bridge-auth.json; without a valid config the
// worker stays dormant and never registers with the daemon.

export const BRIDGE_CONFIG_FILE = "bridge-config.json";
export const BRIDGE_CONFIG_SCHEMA_VERSION = 1;

const KIND_PATTERN = /^[a-z0-9_]+$/;
const TAB_PREFIX_PATTERN = /^[a-z0-9_]+-tab:$/;
const FAMILIES = Object.freeze(["chromium", "gecko"]);

function requireString(config, key) {
  const value = config[key];
  if (typeof value !== "string" || value.trim() === "") {
    throw new Error(`${BRIDGE_CONFIG_FILE} field ${key} must be a non-empty string`);
  }
  return value;
}

function requireStringArray(config, key, { allowEmpty }) {
  const value = config[key];
  if (!Array.isArray(value) || (!allowEmpty && value.length === 0)) {
    throw new Error(`${BRIDGE_CONFIG_FILE} field ${key} must be ${allowEmpty ? "an" : "a non-empty"} array of strings`);
  }
  if (!value.every((item) => typeof item === "string" && item !== "")) {
    throw new Error(`${BRIDGE_CONFIG_FILE} field ${key} must contain only non-empty strings`);
  }
  return Object.freeze([...value]);
}

// Validates the parsed JSON and returns a frozen config; throws on any
// missing or malformed field (fail closed: a wrong prefix would mint target
// ids of another browser).
export function parseBridgeConfig(raw) {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) {
    throw new Error(`${BRIDGE_CONFIG_FILE} must be a JSON object`);
  }
  if (raw.schemaVersion !== BRIDGE_CONFIG_SCHEMA_VERSION) {
    throw new Error(`${BRIDGE_CONFIG_FILE} schemaVersion must be ${BRIDGE_CONFIG_SCHEMA_VERSION}`);
  }
  const browserKind = requireString(raw, "browserKind");
  if (!KIND_PATTERN.test(browserKind)) {
    throw new Error(`${BRIDGE_CONFIG_FILE} browserKind ${JSON.stringify(browserKind)} must match [a-z0-9_]+`);
  }
  const family = requireString(raw, "family");
  if (!FAMILIES.includes(family)) {
    throw new Error(`${BRIDGE_CONFIG_FILE} family ${JSON.stringify(family)} must be one of ${FAMILIES.join(",")}`);
  }
  const tabTargetPrefix = requireString(raw, "tabTargetPrefix");
  if (!TAB_PREFIX_PATTERN.test(tabTargetPrefix)) {
    throw new Error(`${BRIDGE_CONFIG_FILE} tabTargetPrefix ${JSON.stringify(tabTargetPrefix)} must match [a-z0-9_]+-tab:`);
  }
  const policyRoot = raw.policyRoot === null || raw.policyRoot === undefined
    ? null
    : requireString(raw, "policyRoot");
  return Object.freeze({
    schemaVersion: BRIDGE_CONFIG_SCHEMA_VERSION,
    browserKind,
    family,
    expectedExtensionId: requireString(raw, "expectedExtensionId"),
    tabTargetPrefix,
    windowTitleSuffixes: requireStringArray(raw, "windowTitleSuffixes", { allowEmpty: false }),
    protectedExtensionIds: requireStringArray(raw, "protectedExtensionIds", { allowEmpty: true }),
    policyRoot
  });
}

// Reads bridge-config.json from the extension package (never the network).
export async function loadBridgeConfig(fetchImpl, getUrl) {
  const response = await fetchImpl(getUrl(BRIDGE_CONFIG_FILE), {
    cache: "no-store",
    redirect: "error"
  });
  if (!response.ok) {
    throw new Error(`${BRIDGE_CONFIG_FILE} is unavailable; rerun scripts\\install-synapse-browser-bridge.ps1 for this browser`);
  }
  return parseBridgeConfig(await response.json());
}

// Strips one configured window-title suffix from a whitespace-collapsed,
// lowercased title (replaces the hardcoded " - google chrome" regex).
export function stripWindowTitleSuffix(normalizedLowerTitle, suffixes) {
  for (const suffix of suffixes || []) {
    const normalized = String(suffix).toLowerCase().replace(/\s+/g, " ");
    if (normalized.trim() && normalizedLowerTitle.endsWith(normalized)) {
      return normalizedLowerTitle.slice(0, -normalized.length).trim();
    }
  }
  return normalizedLowerTitle;
}
