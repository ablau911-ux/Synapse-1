// Real handleCommand -> handleDomAction; Chrome API is the controlled boundary.
// Deterministic clock starts at real wall time; deferred promises replace sleeps.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const source = readFileSync(new URL('../service_worker.js', import.meta.url), 'utf8');
function production(name) {
  const match = source.match(new RegExp(`^(?:async )?function ${name}\\([\\s\\S]*?(?=^(?:async )?function |$(?![\\s\\S]))`, 'm'));
  assert.notEqual(match, null, `production function ${name}`);
  return match[0];
}
function deferred() {
  let resolve;
  const promise = new Promise(done => { resolve = done; });
  return { promise, resolve };
}

function chromeBoundary(obstacle, started, release, actions) {
  return { runtime: { id: 'fixture' }, scripting: { executeScript: async request => {
    if (request.args[0].resolveOnly) {
      if (obstacle === 'resolve') { started.resolve(); await release.promise; }
      return [{ frameId: 0, result: { ok: true, matched_count: 1, action_point: { x: 3, y: 4 } } }];
    }
    actions.push(request.args[0].action);
    return [{ frameId: 0, result: { ok: true, matched_count: 1 } }];
  } } };
}

const functions = ['handleCommand', 'withInputDispatchStage', 'markInputDispatchStarted', 'handleDomAction', 'handleCdpInput', 'normalizeCdpInputAction',
  'normalizeCdpInputCoordinate', 'applyDaemonDeadline', 'normalizeDomAction',
  'normalizeWaitTimeout', 'normalizeDomActionAutoWaitTimeout', 'normalizeClickCount',
  'normalizeMouseButton', 'normalizeClickModifiers', 'normalizeClickPosition',
  'frameExecutionResults', 'frameResolvedMatchCount', 'summarizeFrameExecutionResult',
  'stringOrNull', 'bridgeError', 'errorPayload', 'errorMessage', 'trimForReadback'];

function fixture(obstacle, kind = 'domAction') {
  const started = deferred();
  const release = deferred();
  const responses = [];
  const actions = [];
  let now = Date.now();
  let pageReads = 0;
  const page = { target_id: 'chrome-tab:7', chrome_window_id: 9 };
  const context = vm.createContext({
    console, Date: { now: () => now }, MAX_PAGE_TEXT_CHARS: 100,
    DAEMON_DEADLINE_RESERVE_MS: 1500,
    DEADLINE_WAIT_FIELDS: ['waitTimeoutMs', 'timeoutMs', 'wait_timeout_ms', 'timeout_ms'],
    DEADLINE_WAIT_FIELD_MIN_MS: 1, DEADLINE_AUTO_WAIT_FIELD: 'autoWaitTimeoutMs',
    DEADLINE_AUTO_WAIT_MIN_MS: 50,
    requireExternalPopupRisksSuppressed: async () => {},
    requireDebuggerApiAvailable() {},
    activateTabForCdpTouch: async () => ({ attempted: true, activated: true }),
    dispatchCdpInput: async (_tab, action) => {
      actions.push(action);
      return { method: 'Input.dispatchMouseEvent', dispatched_events: 2, protocol_version: '1.3' };
    },
    selectTabTarget: async () => ({ tabId: 7, target: { id: 'chrome-tab:7' }, targetCandidateCount: 1 }),
    tabPageState: async () => {
      if (++pageReads === 1 && obstacle === 'page-state') { started.resolve(); await release.promise; }
      return page;
    },
    tabPageTextState: async () => ({ text: '' }),
    waitForTabPageState: async () => page,
    createDomActionPopupTabs: async () => [],
    postResponse: async (...args) => responses.push(args),
    performDomActionInPage() {},
    chrome: chromeBoundary(obstacle, started, release, actions)
  });
  for (const [name, value] of [...source.matchAll(/^const (ERROR_[A-Z_]+) = ("[^"\n]*");/gm)].map(m => [m[1], JSON.parse(m[2])])) {
    context[name] = value;
  }
  vm.runInContext(functions.map(production).join('\n'), context);
  const deadline = now + 60_000;
  const command = { id: 'deadline-183', kind, deadlineUnixMs: deadline,
    params: { targetId: 'chrome-tab:7', action: 'click', selector: '#owned', autoWait: false, waitTimeoutMs: 1 } };
  return { actions, responses, started, release, run: () => context.handleCommand(command),
    expire: () => { now = deadline + 1; } };
}

test('must_pass_in_budget_dom_action_dispatches_once_and_returns_readback', async () => {
  const f = fixture('none');
  assert.deepEqual(f.actions, []);
  await f.run();
  assert.deepEqual(f.actions, ['click']);
  assert.equal(f.responses.length, 1);
  assert.equal(f.responses[0][1], true);
  assert.equal(f.responses[0][2].target_id, 'chrome-tab:7');
  assert.equal(f.responses[0][2].frame_id, 0);
});

for (const kind of ['domAction', 'cdpInput']) for (const obstacle of ['page-state', 'resolve']) {
  test(`must_fail_expired_${kind}_${obstacle}_preflight_cannot_dispatch_late_click`, { timeout: 5000 }, async () => {
    const f = fixture(obstacle, kind);
    assert.deepEqual(f.actions, []);
    const running = f.run();
    await f.started.promise;
    assert.deepEqual(f.actions, [], 'precondition: preflight paused before side effect');
    f.expire();
    f.release.resolve();
    await running;
    assert.deepEqual(f.actions, [], '#183: expired preflight must not execute page action');
    assert.equal(f.responses.length, 1);
    assert.equal(f.responses[0][1], false);
    assert.equal(f.responses[0][3].code, 'A11Y_CDP_EXTENSION_TIMEOUT');
  });
}
