// Frozen behaviour table (manager-owned, work method v3; upstream 618cea9d,
// #2264/#2271). The worker hashes its own code once per worker lifetime: after
// a deploy the disk holds the NEW file while this worker still runs the OLD
// code, so a reconnect must keep reporting the loaded hash, never the disk's.
// Codex and Opus never edit this file. One case per L row.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const source = readFileSync(new URL("../service_worker.js", import.meta.url), "utf8");
const start = source.indexOf("function serviceWorkerIntegrityRefreshNeeded(");
const end = source.indexOf("\nfunction bridgeIdentity(", start);
assert.ok(start >= 0 && end > start, "worker integrity functions missing");
const code = source.slice(start, end);

const CODE_A = "// worker build A\n";
const CODE_B = "// worker build B (deployed while A runs)\n";
const sha = (text) => createHash("sha256").update(text).digest("hex");

function harness({ crypto = globalThis.crypto } = {}) {
  const disk = { text: CODE_A, status: 200, throws: false };
  const calls = { fetch: 0 };
  const context = vm.createContext({
    console: { warn() {}, log() {}, error() {} },
    crypto,
    errorMessage: (error) => String(error?.message ?? error),
    chrome: { runtime: { getURL: (path) => `chrome-extension://bridge/${path}` } },
    fetch: async () => {
      calls.fetch += 1;
      const text = disk.text;
      const status = disk.status;
      const throws = disk.throws;
      await new Promise((resolve) => setTimeout(resolve, 1));
      if (throws) {
        throw new Error("network down");
      }
      return {
        ok: status === 200,
        status,
        arrayBuffer: async () => new TextEncoder().encode(text).buffer
      };
    }
  });
  vm.runInContext(
    `let serviceWorkerIntegrityState = {
       status: "not_checked", source: "chrome-extension://bridge/service_worker.js",
       serviceWorkerSha256: null, byteLength: null, error: null, checkedAtUnixMs: 0, reason: "not_checked"
     };
     let serviceWorkerIntegrityInFlight = null;
     ${code}
     globalThis.readState = () => serviceWorkerIntegrityState;
     globalThis.presetState = (state) => { serviceWorkerIntegrityState = state; };`,
    context
  );
  return { context, disk, calls };
}

const rows = [
  {
    row: "w1a_first_registration_hashes_loaded_code",
    run: async () => {
      const { context, calls } = harness();
      await context.refreshServiceWorkerIntegrity("registerDaemon");
      const state = context.readState();
      return { status: state.status, sha: state.serviceWorkerSha256, fetches: calls.fetch };
    },
    expected: { status: "ok", sha: sha(CODE_A), fetches: 1 }
  },
  {
    row: "w1b_reconnect_after_deploy_keeps_loaded_hash",
    run: async () => {
      const { context, disk, calls } = harness();
      await context.refreshServiceWorkerIntegrity("registerDaemon");
      disk.text = CODE_B;
      await context.refreshServiceWorkerIntegrity("registerDaemon");
      const state = context.readState();
      return { status: state.status, sha: state.serviceWorkerSha256, fetches: calls.fetch };
    },
    expected: { status: "ok", sha: sha(CODE_A), fetches: 1 }
  },
  {
    row: "w1c_failed_first_hash_retries_on_next_registration",
    run: async () => {
      const { context, disk, calls } = harness();
      disk.status = 500;
      await context.refreshServiceWorkerIntegrity("registerDaemon");
      const first = context.readState().status;
      disk.status = 200;
      await context.refreshServiceWorkerIntegrity("registerDaemon");
      const state = context.readState();
      return { first, status: state.status, sha: state.serviceWorkerSha256, fetches: calls.fetch };
    },
    expected: { first: "error", status: "ok", sha: sha(CODE_A), fetches: 2 }
  },
  {
    row: "w1d_missing_crypto_never_claims_verified",
    run: async () => {
      const { context } = harness({ crypto: undefined });
      await context.refreshServiceWorkerIntegrity("registerDaemon");
      const state = context.readState();
      return { status: state.status, sha: state.serviceWorkerSha256 };
    },
    expected: { status: "error", sha: null }
  },
  {
    row: "w1e_hostile_fetch_throw_never_claims_verified_and_retries",
    run: async () => {
      const { context, disk } = harness();
      disk.throws = true;
      await context.refreshServiceWorkerIntegrity("registerDaemon");
      const firstSha = context.readState().serviceWorkerSha256;
      disk.throws = false;
      await context.refreshServiceWorkerIntegrity("registerDaemon");
      return { first_sha: firstSha, sha: context.readState().serviceWorkerSha256 };
    },
    expected: { first_sha: null, sha: sha(CODE_A) }
  },
  {
    row: "w1f_second_actor_concurrent_registrations_hash_once",
    run: async () => {
      const { context, calls } = harness();
      const [one, two] = await Promise.all([
        context.refreshServiceWorkerIntegrity("registerDaemon"),
        context.refreshServiceWorkerIntegrity("alarm")
      ]);
      return { fetches: calls.fetch, one: one.serviceWorkerSha256, two: two.serviceWorkerSha256 };
    },
    expected: { fetches: 1, one: sha(CODE_A), two: sha(CODE_A) }
  },
  {
    row: "w1g_hostile_ok_state_without_hash_is_not_trusted",
    run: async () => {
      const { context, calls } = harness();
      context.presetState({ status: "ok", serviceWorkerSha256: null, source: "x", byteLength: 0, error: null, checkedAtUnixMs: 0, reason: "forged" });
      await context.refreshServiceWorkerIntegrity("registerDaemon");
      return { sha: context.readState().serviceWorkerSha256, fetches: calls.fetch };
    },
    expected: { sha: sha(CODE_A), fetches: 1 }
  },
  {
    row: "w1h_restart_new_worker_hashes_new_code",
    run: async () => {
      const { context, disk } = harness();
      disk.text = CODE_B; // the reloaded worker runs the deployed file
      await context.refreshServiceWorkerIntegrity("registerDaemon");
      return { sha: context.readState().serviceWorkerSha256 };
    },
    expected: { sha: sha(CODE_B) }
  }
];

test("worker_identity_table_worker_identity", async () => {
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
  assert.equal(failures.length, 0, `worker identity table rows failed:\n${failures.join("\n")}`);
});
