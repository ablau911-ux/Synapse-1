// #182 result-size cap + no teardown on 4xx, #183 daemon deadline handling.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const source = readFileSync(new URL("../service_worker.js", import.meta.url), "utf8");

function slice(startMarker, endMarker) {
  const start = source.indexOf(startMarker);
  assert.ok(start >= 0, `missing ${startMarker}`);
  const end = endMarker ? source.indexOf(endMarker, start) : source.length;
  assert.ok(end > start, `missing ${endMarker}`);
  return source.slice(start, end);
}

const constants = slice("const ERROR_RESULT_TOO_LARGE", "const DAEMON_DEADLINE_RESERVE_MS") +
  "const DAEMON_DEADLINE_RESERVE_MS = 1500;\n";
const deadlineCode = slice("const DEADLINE_WAIT_FIELDS", "async function refreshServiceWorkerIntegrity(");
const postCode = slice("function resultByteLength(");

function harness(fetchImpl) {
  const requests = [];
  const reconnects = [];
  const context = vm.createContext({
    TextEncoder,
    console,
    hostId: "host-1",
    bridgeToken: "tok",
    scheduleReconnect: (reason, code) => reconnects.push({ reason, code }),
    chrome: { runtime: { getURL: (path) => `chrome-extension://bridge/${path}` } },
    fetch: async (url, init) => {
      requests.push({ url, init });
      return fetchImpl(url, init);
    }
  });
  vm.runInContext(`
    const BRIDGE_REGISTRATION_TOKEN_HEADER = "X-Synapse-Bridge-Registration-Token";
    const BRIDGE_TOKEN_HEADER = "X-Synapse-Bridge-Token";
    const DAEMON_BASE_URL = "http://127.0.0.1:7700";
    const ERROR_DAEMON_UNAVAILABLE = "SYNAPSE_CHROME_DAEMON_UNAVAILABLE";
    const ERROR_ATTACH_FAILED = "A11Y_CDP_ATTACH_FAILED";
    ${constants}
    ${deadlineCode}
    ${postCode}
  `, context);
  return { context, requests, reconnects };
}

const reply = (status, body) => async () => ({
  ok: status >= 200 && status < 300,
  status,
  text: async () => (typeof body === "string" ? body : JSON.stringify(body))
});

test("must_fail 413 does not tear down the bridge and surfaces status", async () => {
  const { context, reconnects } = harness(
    reply(413, { ok: false, code: "CHROME_BRIDGE_MESSAGE_REJECTED", detail: "too big" })
  );
  await assert.rejects(context.postDaemonMessage({ type: "response" }), (error) => error.status === 413);
  assert.equal(reconnects.length, 0);
});

test("must_fail other 4xx (400 bad request, 404, 422) do not tear down", async () => {
  for (const status of [400, 404, 422]) {
    const { context, reconnects } = harness(reply(status, { ok: false, detail: "rejected" }));
    await assert.rejects(context.postDaemonMessage({ type: "event" }));
    assert.equal(reconnects.length, 0, `status ${status}`);
  }
});

test("must_fail 401, 403, 5xx, network error and unknown host still tear down", async () => {
  for (const [label, impl] of [
    ["401", reply(401, { ok: false, code: "HTTP_TOKEN_INVALID" })],
    ["403", reply(403, { ok: false })],
    ["500", reply(500, { ok: false })],
    ["unknown host", reply(400, { ok: false, detail: 'unknown chrome debugger native host_id "h"' })],
    ["non-json 502", reply(502, "bad gateway")],
    ["network", async () => { throw new TypeError("Failed to fetch"); }]
  ]) {
    const { context, reconnects } = harness(impl);
    await assert.rejects(context.postDaemonMessage({ type: "event" }), undefined, label);
    assert.equal(reconnects.length, 1, label);
  }
});

test("must_fail non-JSON 413 from an older daemon keeps its status and does not tear down", async () => {
  const { context, reconnects } = harness(reply(413, "Failed to buffer the request body"));
  await assert.rejects(context.postDaemonMessage({ type: "response" }), (error) => error.status === 413);
  assert.equal(reconnects.length, 0);
});

test("must_fail oversize result is refused locally with RESULT_TOO_LARGE and never posted", async () => {
  const { context, requests } = harness(reply(200, { ok: true }));
  const huge = { data: "x".repeat(49 * 1024 * 1024) };
  await assert.rejects(
    context.postResponse("c1", true, huge, null),
    (error) => error.code === "RESULT_TOO_LARGE"
  );
  assert.equal(requests.length, 0);
});

test("must_pass multi-byte results are measured in UTF-8 bytes", async () => {
  const { context, requests } = harness(reply(200, { ok: true }));
  // 17M euro signs = 51 MB of UTF-8 but only 17M chars.
  await assert.rejects(
    context.postResponse("c2", true, { text: "€".repeat(17_000_000) }, null),
    (error) => error.code === "RESULT_TOO_LARGE"
  );
  assert.equal(requests.length, 0);
  await context.postResponse("c3", true, { text: "€".repeat(15_000_000) }, null);
  assert.equal(requests.length, 1);
});

test("must_fail daemon 413 on a result becomes RESULT_TOO_LARGE, not a teardown", async () => {
  const { context, reconnects } = harness(
    reply(413, { ok: false, code: "CHROME_BRIDGE_MESSAGE_REJECTED", detail: "too big" })
  );
  await assert.rejects(
    context.postResponse("c4", true, { data: "small" }, null),
    (error) => error.code === "RESULT_TOO_LARGE"
  );
  assert.equal(reconnects.length, 0);
});

test("must_pass normal and error replies are posted; empty/null results are fine", async () => {
  const { context, requests } = harness(reply(200, { ok: true }));
  await context.postResponse("c5", true, { a: 1 }, null);
  await context.postResponse("c6", true, null, null);
  await context.postResponse("c7", false, null, { code: "X", detail: "y" });
  assert.equal(requests.length, 3);
  assert.equal(JSON.parse(requests[2].init.body).message.ok, false);
});

test("must_pass no deadline leaves the command untouched", () => {
  const { context } = harness(reply(200, {}));
  const params = { waitTimeoutMs: 30000 };
  const outcome = context.applyDaemonDeadline({ id: "a", params }, 1_000);
  assert.equal(outcome.expired, false);
  assert.equal(outcome.params, params);
});

test("must_fail past deadline (or inside the reserve) expires the command", () => {
  const { context } = harness(reply(200, {}));
  const past = context.applyDaemonDeadline({ id: "a", params: {}, deadlineUnixMs: 5_000 }, 6_000);
  assert.equal(past.expired, true);
  assert.equal(past.overdueMs, 1_000);
  const inReserve = context.applyDaemonDeadline({ id: "a", params: {}, deadlineUnixMs: 6_000 }, 5_000);
  assert.equal(inReserve.expired, true);
  assert.equal(inReserve.overdueMs, 0);
});

test("must_pass plenty of time keeps the caller waits as asked", () => {
  const { context } = harness(reply(200, {}));
  const params = { waitTimeoutMs: 30000, autoWaitTimeoutMs: 2000 };
  const outcome = context.applyDaemonDeadline(
    { id: "a", params, deadlineUnixMs: 1_000 + 37_000 },
    1_000
  );
  assert.equal(outcome.expired, false);
  assert.deepEqual({ ...outcome.params }, params);
});

test("must_pass late pickup clamps the wait chain to end before the deadline minus reserve", () => {
  const { context } = harness(reply(200, {}));
  const params = { waitTimeoutMs: 30000, autoWaitTimeoutMs: 30000, other: "kept" };
  // 21.5 s left, 1.5 s reserve -> 20 s usable for a 60 s chain.
  const outcome = context.applyDaemonDeadline(
    { id: "a", params, deadlineUnixMs: 1_000 + 21_500 },
    1_000
  );
  assert.equal(outcome.expired, false);
  assert.equal(outcome.params.other, "kept");
  assert.ok(outcome.params.waitTimeoutMs + outcome.params.autoWaitTimeoutMs <= 20_000);
  assert.ok(outcome.params.waitTimeoutMs >= 1 && outcome.params.autoWaitTimeoutMs >= 50);
  assert.equal(params.waitTimeoutMs, 30000, "caller params must not be mutated");
});

test("must_pass clamp keeps minimum valid values for tiny windows", () => {
  const { context } = harness(reply(200, {}));
  const outcome = context.applyDaemonDeadline(
    { id: "a", params: { timeoutMs: 30000, autoWaitTimeoutMs: 30000 }, deadlineUnixMs: 1_000 + 1_600 },
    1_000
  );
  assert.equal(outcome.expired, false);
  assert.ok(outcome.params.timeoutMs >= 1);
  assert.ok(outcome.params.autoWaitTimeoutMs >= 50);
});

test("must_fail handleCommand drops expired commands before executing any handler", () => {
  const body = slice("async function handleCommand(command)", "const DEADLINE_WAIT_FIELDS");
  const guard = body.indexOf("applyDaemonDeadline(command");
  const firstHandler = body.indexOf("await requireExternalPopupRisksSuppressed");
  assert.ok(guard > 0 && firstHandler > guard, "deadline guard must precede handler dispatch");
  assert.match(body, /if \(deadline\.expired\) \{[\s\S]*?return;/);
});
