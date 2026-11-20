// Firefox persistent background page bootstrap. Runs before the shared worker
// module (background.html orders them) and makes the worker's `chrome.*` calls
// return promises by aliasing Firefox's `browser` namespace.

import { installGeckoNamespace } from "./adapters/gecko.js";

const outcome = installGeckoNamespace(globalThis);
if (!outcome.installed) {
  console.error(
    `Synapse Firefox bridge could not alias the browser namespace: reason=${outcome.reason}; ` +
      "the shared worker's chrome.* calls will not return promises"
  );
}
