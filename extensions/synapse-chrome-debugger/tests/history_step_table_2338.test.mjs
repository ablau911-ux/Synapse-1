// Frozen behaviour table (manager-owned, work method v3; Gitea #2338, roadmap
// item #571). browser_nav back / forward on a Chrome bridge tab move through
// the tab's own history list (Page.getNavigationHistory, then
// Page.navigateToHistoryEntry), because chrome.tabs.goBack skips entries the
// user never touched. The object `TabHistory` owns the tab's history (read,
// step, close); its pure static method decides the step; this file is its
// table. Codex and Opus never edit this file. One case per H row.
//
// Contract of `TabHistory.stepTarget(history, direction)` (static method of the
// top-level `class TabHistory`, pure: no Chrome call, no global, does not change
// its argument; the class body must load in an empty context, so it names
// extension helpers only inside its instance methods):
//   history   = what Page.getNavigationHistory returns:
//               { currentIndex: <integer>, entries: [{ id, url, title, ... }] }
//   direction = "back" | "forward"
// A move returns exactly    { ok: true, entryId: <entries[target].id>, index: <target> }
//   where target = currentIndex - 1 (back) or currentIndex + 1 (forward).
// A refusal returns exactly { ok: false, reason, index, count }
//   reason = "first_entry" | "last_entry" | "no_entries" | "unusable_history" | "bad_direction"
//   index  = history.currentIndex when it is an integer, else -1
//   count  = history.entries.length when entries is an array, else 0
// Order of the checks: bad_direction, then unusable_history (entries not an
// array, currentIndex not an integer), then no_entries (empty list), then
// unusable_history (currentIndex outside the list), then first_entry / last_entry.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const source = readFileSync(new URL("../service_worker.js", import.meta.url), "utf8");

// Loaded per case, so each row fails on its own when the object is missing.
function loadHistoryStepTarget() {
  const start = source.indexOf("\nclass TabHistory {");
  if (start < 0) {
    throw new Error("class TabHistory is not defined in service_worker.js");
  }
  const end = source.indexOf("\n}\n", start);
  assert.ok(end > start, "class TabHistory has no closing } in column 0");
  // An empty context: the static rule may use no extension global.
  const context = vm.createContext({});
  vm.runInContext(`${source.slice(start, end + 2)}\nglobalThis.TabHistory = TabHistory;`, context);
  assert.equal(typeof context.TabHistory.stepTarget, "function", "TabHistory.stepTarget is not a static method");
  return (history, direction) => context.TabHistory.stepTarget(history, direction);
}

const A = { id: 11, url: "http://127.0.0.1:8796/a.html", userTypedURL: "http://127.0.0.1:8796/a.html", title: "A", transitionType: "typed" };
const B = { id: 12, url: "http://127.0.0.1:8796/b.html", userTypedURL: "http://127.0.0.1:8796/b.html", title: "B", transitionType: "typed" };
const C = { id: 13, url: "http://127.0.0.1:8796/c.html", userTypedURL: "http://127.0.0.1:8796/c.html", title: "C", transitionType: "typed" };
const history = (currentIndex, entries) => ({ currentIndex, entries });
const move = (entryId, index) => ({ ok: true, entryId, index });
const refuse = (reason, index, count) => ({ ok: false, reason, index, count });

// One call: the exact result, and the argument left unchanged.
function check(step, label, given, direction, expected) {
  const before = JSON.stringify(given);
  let got;
  assert.doesNotThrow(() => {
    got = step(given, direction);
  }, `${label}: TabHistory.stepTarget threw`);
  assert.equal(JSON.stringify(given), before, `${label}: the history argument was changed`);
  // JSON copy: the result comes from another vm realm.
  assert.deepEqual(JSON.parse(JSON.stringify(got ?? null)), expected, `${label}: wrong result`);
}

const rows = [
  {
    row: "H1_back_from_index_1_of_2_goes_to_entry_A_index_0",
    calls: [["back", history(1, [A, B]), "back", move(A.id, 0)]]
  },
  {
    row: "H2_forward_from_index_0_of_2_goes_to_entry_B_index_1",
    calls: [["forward", history(0, [A, B]), "forward", move(B.id, 1)]]
  },
  {
    row: "H3_middle_of_3_back_goes_to_A_forward_goes_to_C",
    calls: [
      ["back", history(1, [A, B, C]), "back", move(A.id, 0)],
      ["forward", history(1, [A, B, C]), "forward", move(C.id, 2)]
    ]
  },
  {
    row: "H4_back_at_first_entry_is_refused_index_0_of_2",
    calls: [["back", history(0, [A, B]), "back", refuse("first_entry", 0, 2)]]
  },
  {
    row: "H5_forward_at_last_entry_is_refused_index_1_of_2",
    calls: [["forward", history(1, [A, B]), "forward", refuse("last_entry", 1, 2)]]
  },
  {
    row: "H6_one_entry_refuses_back_and_forward",
    calls: [
      ["back", history(0, [A]), "back", refuse("first_entry", 0, 1)],
      ["forward", history(0, [A]), "forward", refuse("last_entry", 0, 1)]
    ]
  },
  {
    row: "H7_empty_entry_list_is_refused_without_crash",
    calls: [["back", history(0, []), "back", refuse("no_entries", 0, 0)]]
  },
  {
    row: "H8_unusable_history_read_is_refused_without_crash",
    calls: [
      ["index 5 of [A, B]", history(5, [A, B]), "back", refuse("unusable_history", 5, 2)],
      ["index -1 of [A, B]", history(-1, [A, B]), "back", refuse("unusable_history", -1, 2)],
      ["index not a number", history("1", [A, B]), "back", refuse("unusable_history", -1, 2)],
      ["entries missing", { currentIndex: 1 }, "back", refuse("unusable_history", 1, 0)],
      ["history null", null, "back", refuse("unusable_history", -1, 0)]
    ]
  },
  {
    row: "H9_direction_neither_back_nor_forward_is_refused",
    calls: [
      ["reload", history(1, [A, B]), "reload", refuse("bad_direction", 1, 2)],
      ["no direction", history(1, [A, B]), undefined, refuse("bad_direction", 1, 2)]
    ]
  }
];

for (const { row, calls } of rows) {
  test(row, () => {
    const step = loadHistoryStepTarget();
    for (const [label, given, direction, expected] of calls) {
      check(step, `${row} (${label})`, given, direction, expected);
    }
  });
}
