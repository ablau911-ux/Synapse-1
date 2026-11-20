// #236: browser_storage readback `changed` must report whether the store
// actually changed: clearing an absent key or re-setting the same value is not
// a change.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const source = readFileSync(new URL("../service_worker.js", import.meta.url), "utf8");
const start = source.indexOf("function runStorageCommandInPage(");
assert.ok(start >= 0, "runStorageCommandInPage missing");
const end = source.indexOf("\nfunction readStorageStateInPage(", start);
assert.ok(end > start, "readStorageStateInPage boundary missing");
const code = source.slice(start, end);

function fakeStorage(entries = {}) {
  const map = new Map(Object.entries(entries));
  return {
    get length() {
      return map.size;
    },
    key: (index) => [...map.keys()][index] ?? null,
    getItem: (key) => (map.has(key) ? map.get(key) : null),
    setItem: (key, value) => map.set(key, String(value)),
    removeItem: (key) => map.delete(key),
    clear: () => map.clear()
  };
}

function run(request, entries) {
  const context = vm.createContext({
    window: { localStorage: fakeStorage(entries), sessionStorage: fakeStorage() },
    location: { origin: "https://example.com", href: "https://example.com/" },
    document: { title: "t", readyState: "complete" }
  });
  vm.runInContext(code, context);
  return context.runStorageCommandInPage(request);
}

test("must_pass_set_of_a_new_key_and_clear_of_an_existing_key_report_changed", () => {
  assert.equal(run({ operation: "set", key: "k", value: "v" }, {}).changed, true);
  assert.equal(run({ operation: "clear", key: "k" }, { k: "v" }).changed, true);
  assert.equal(run({ operation: "clear" }, { k: "v" }).changed, true);
});

test("must_fail_clear_of_an_absent_key_or_same_value_set_is_not_a_change", () => {
  assert.equal(run({ operation: "clear", key: "missing" }, { other: "x" }).changed, false);
  assert.equal(run({ operation: "set", key: "k", value: "v" }, { k: "v" }).changed, false);
  assert.equal(run({ operation: "clear" }, {}).changed, false);
});
