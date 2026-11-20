import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const source = readFileSync(new URL("../service_worker.js", import.meta.url), "utf8");
const start = source.indexOf("async function loadBridgeRegistrationToken()");
const end = source.indexOf("function bridgeError(", start);
assert.ok(start >= 0 && end > start);
const functions = source.slice(start, end);
const registrationToken = "ab".repeat(32);

function harness(config) {
  const requests = [];
  const context = vm.createContext({
    bridgeToken: "host-session-token",
    chrome: { runtime: { getURL: (path) => `chrome-extension://bridge/${path}` } },
    fetch: async (url, init) => {
      requests.push({ url, init });
      if (url.startsWith("chrome-extension:")) {
        return { ok: true, json: async () => config };
      }
      return { ok: true, text: async () => JSON.stringify({ ok: true }) };
    }
  });
  vm.runInContext(`
    const BRIDGE_REGISTRATION_TOKEN_HEADER = "X-Synapse-Bridge-Registration-Token";
    const BRIDGE_TOKEN_HEADER = "X-Synapse-Bridge-Token";
    const DAEMON_BASE_URL = "http://127.0.0.1:7700";
    ${functions}
  `, context);
  return { context, requests };
}

test("registration uses private config only for registration", async () => {
  const { context, requests } = harness({ registration_token: registrationToken });
  await context.daemonFetchJson("/chrome-debugger/native/register", { method: "POST", body: { pid: 0 } });
  assert.equal(requests[0].url, "chrome-extension://bridge/bridge-auth.json");
  assert.equal(requests[0].init.cache, "no-store");
  assert.equal(requests[1].init.headers["X-Synapse-Bridge-Registration-Token"], registrationToken);
  assert.equal(requests[1].init.headers["X-Synapse-Bridge-Token"], undefined);
  assert.equal(requests[1].init.body.includes(registrationToken), false);
  await context.daemonFetchJson("/chrome-debugger/native/next?host_id=x");
  assert.equal(requests[2].init.headers["X-Synapse-Bridge-Token"], "host-session-token");
  assert.equal(requests[2].init.headers["X-Synapse-Bridge-Registration-Token"], undefined);
});

test("invalid private config never sends a registration request", async () => {
  for (const config of [{}, { registration_token: "short" }, { registration_token: "A".repeat(64) }]) {
    const { context, requests } = harness(config);
    await assert.rejects(context.daemonFetchJson("/chrome-debugger/native/register", { method: "POST" }));
    assert.equal(requests.length, 1);
  }
});
