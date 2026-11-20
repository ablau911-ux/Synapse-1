// #183: exercise real command routing and sibling handlers across stalled preflight.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const source = readFileSync(new URL('../service_worker.js', import.meta.url), 'utf8');
function production(name) {
  const match = source.match(new RegExp(`^(?:async )?function ${name}\\([\\s\\S]*?(?=^(?:async )?function |$(?![\\s\\S]))`, 'm'));
  assert.ok(match, `production function ${name}`);
  return match[0];
}
function deferred() {
  let resolve;
  const promise = new Promise(done => { resolve = done; });
  return { promise, resolve };
}
const functions = ['handleCommand', 'applyDaemonDeadline', 'handleCoordinateClick',
  'handleTypeActiveElement', 'handleSetFieldValue', 'handleCdpInputText',
  'normalizeWaitTimeout', 'normalizeCoordinateValue', 'normalizeCoordinateSpace',
  'normalizeClickCount', 'normalizeMouseButton', 'normalizeClickModifiers',
  'frameExecutionResults', 'frameResolvedMatchCount', 'summarizeFrameExecutionResult',
  'stringOrNull', 'bridgeError', 'errorPayload', 'errorMessage', 'trimForReadback'];

function fixture(kind, editableKind, obstacle) {
  const started = deferred(), release = deferred();
  const actions = [], responses = [];
  let now = Date.now(), paused = false;
  const deadline = now + 60_000;
  async function pause(at) {
    if (!paused && obstacle === at) { paused = true; started.resolve(); await release.promise; }
  }
  const page = { target_id: 'chrome-tab:7', chrome_window_id: 9 };
  const context = vm.createContext({
    console, Date: { now: () => now }, MAX_PAGE_TEXT_CHARS: 100,
    COMMAND_CAPABILITIES: ['cdpInput'], DAEMON_DEADLINE_RESERVE_MS: 1500,
    DEADLINE_WAIT_FIELDS: ['waitTimeoutMs', 'timeoutMs', 'wait_timeout_ms', 'timeout_ms'],
    DEADLINE_WAIT_FIELD_MIN_MS: 1, DEADLINE_AUTO_WAIT_FIELD: 'autoWaitTimeoutMs', DEADLINE_AUTO_WAIT_MIN_MS: 50,
    requireExternalPopupRisksSuppressed: async () => {},
    selectTabTarget: async () => { await pause('select'); return { tabId: 7, target: { id: page.target_id }, targetCandidateCount: 1 }; },
    tabPageState: async () => { await pause('page-state'); return page; },
    tabPageTextState: async () => ({ text: '' }), waitForTabPageState: async () => page,
    tabActiveElementState: async () => ({}),
    activateTabForCdpTouch: async () => { actions.push('activate'); return {}; },
    cdpTextInputTargetScript: async () => ({ before_value: '', after_value: 'owned', frame_id: 0 }),
    dispatchCdpTextInput: async () => { actions.push('type'); return { method: 'Input.insertText', dispatched_events: 1 }; },
    performCoordinateClickInPage() {}, setFieldValueInPage() {},
    postResponse: async (...args) => responses.push(args),
    chrome: { runtime: { id: 'fixture' }, scripting: { executeScript: async request => {
      if (request.args[0].resolveOnly) {
        await pause('resolve');
        return [{ frameId: 0, result: { ok: true, matched_count: 1, editable_kind: editableKind } }];
      }
      actions.push(kind === 'coordinateClick' ? 'click' : 'set');
      return [{ frameId: 0, result: { ok: true, after_value: 'owned' } }];
    } } }
  });
  for (const m of source.matchAll(/^const (ERROR_[A-Z_]+) = ("[^"\n]*");/gm)) context[m[1]] = JSON.parse(m[2]);
  vm.runInContext(functions.map(production).join('\n'), context);
  return { actions, responses, started, release, expire: () => { now = deadline + 1; },
    run: () => context.handleCommand({ id: 'sibling-183', kind, deadlineUnixMs: deadline,
      params: { targetId: page.target_id, selector: '#owned', text: 'owned', x: 3, y: 4, waitTimeoutMs: 1 } }) };
}

const variants = [
  ['coordinateClick', 'input', 'select'], ['coordinateClick', 'input', 'page-state'],
  ['typeActiveElement', 'input', 'select'], ['typeActiveElement', 'input', 'page-state'],
  ['setFieldValue', 'input', 'select'], ['setFieldValue', 'input', 'resolve'],
  ['setFieldValue', 'contenteditable', 'resolve'], ['setFieldValue', 'contenteditable', 'page-state']
];
for (const [kind, editable, obstacle] of variants) {
  test(`must_fail_expired_${kind}_${editable}_${obstacle}_without_side_effect`, { timeout: 5000 }, async () => {
    const f = fixture(kind, editable, obstacle);
    const running = f.run();
    await f.started.promise;
    assert.deepEqual(f.actions, [], 'preflight has no side effects');
    f.expire(); f.release.resolve(); await running;
    assert.deepEqual(f.actions, [], '#183 no activation, click, typing or field mutation after expiry');
    assert.equal(f.responses.length, 1);
    assert.equal(f.responses[0][1], false, '#183 expired command must be refused');
    assert.equal(f.responses[0][3]?.code, 'A11Y_CDP_EXTENSION_TIMEOUT');
  });
}
for (const [kind, editable] of [['coordinateClick', 'input'], ['typeActiveElement', 'input'], ['setFieldValue', 'input'], ['setFieldValue', 'contenteditable']]) {
  test(`must_pass_in_budget_${kind}_${editable}_retains_action_and_readback`, async () => {
    const f = fixture(kind, editable, 'none');
    await f.run();
    assert.equal(f.responses.length, 1);
    assert.equal(f.responses[0][1], true, JSON.stringify(f.responses));
    assert.equal(f.responses[0][2].target_id, 'chrome-tab:7');
    assert.deepEqual(f.actions, kind === 'coordinateClick' ? ['click'] : kind === 'setFieldValue' && editable === 'input' ? ['set'] : ['activate', 'type']);
  });
}
