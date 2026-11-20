// Static family adapter table (dynamic import() is forbidden in MV3 service
// workers, so every adapter is imported up front and selected by
// bridge-config.json `family`).

import { CHROMIUM_ADAPTER } from "./chromium.js";
import { GECKO_ADAPTER } from "./gecko.js";

const FAMILY_ADAPTERS = Object.freeze({
  chromium: CHROMIUM_ADAPTER,
  gecko: GECKO_ADAPTER
});

export function adapterForFamily(family) {
  const adapter = FAMILY_ADAPTERS[family];
  if (!adapter) {
    throw new Error(`no bridge adapter for family ${JSON.stringify(family)}`);
  }
  return adapter;
}

export { CHROMIUM_ADAPTER, GECKO_ADAPTER };
