// #271 (from #189 B8): the extension's dispatch stage must reach the daemon.
// Real handleCommand -> withInputDispatchStage -> handleDomAction -> errorPayload;
// the Chrome API is the controlled boundary. Each test fails if the producer
// stops sending `dispatch_stage` at one specific hop:
//   page result -> handleDomAction copy (error.dispatchStage) -> errorPayload.
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

const functions = ['handleCommand', 'withInputDispatchStage', 'markInputDispatchStarted', 'handleDomAction',
  'handleCdpInput', 'normalizeCdpInputAction', 'normalizeCdpInputCoordinate', 'applyDaemonDeadline',
  'normalizeDomAction', 'normalizeWaitTimeout', 'normalizeDomActionAutoWaitTimeout', 'normalizeClickCount',
  'normalizeMouseButton', 'normalizeClickModifiers', 'normalizeClickPosition', 'frameExecutionResults',
  'frameResolvedMatchCount', 'summarizeFrameExecutionResult', 'stringOrNull', 'bridgeError', 'errorPayload',
  'errorMessage', 'trimForReadback'];

// `resolveResult` answers the resolve-only preflight call; `actionResult`
// answers the call that performs the page action (the first dispatch).
function fixture({ resolveResult, actionResult }) {
  const responses = [];
  const actionCalls = [];
  const now = Date.now();
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
    dispatchCdpInput: async () => ({ method: 'Input.dispatchMouseEvent', dispatched_events: 2, protocol_version: '1.3' }),
    selectTabTarget: async () => ({ tabId: 7, target: { id: 'chrome-tab:7' }, targetCandidateCount: 1 }),
    tabPageState: async () => page,
    tabPageTextState: async () => ({ text: '' }),
    waitForTabPageState: async () => page,
    createDomActionPopupTabs: async () => [],
    postResponse: async (...args) => responses.push(args),
    performDomActionInPage() {},
    chrome: { runtime: { id: 'fixture' }, scripting: { executeScript: async request => {
      if (request.args[0].resolveOnly) {
        return [{ frameId: 0, result: resolveResult }];
      }
      actionCalls.push(request.args[0].action);
      return [{ frameId: 0, result: actionResult }];
    } } }
  });
  for (const [name, value] of [...source.matchAll(/^const (ERROR_[A-Z_]+) = ("[^"\n]*");/gm)].map(m => [m[1], JSON.parse(m[2])])) {
    context[name] = value;
  }
  vm.runInContext(functions.map(production).join('\n'), context);
  const command = { id: 'stage-271', kind: 'domAction', deadlineUnixMs: now + 60_000,
    params: { targetId: 'chrome-tab:7', action: 'click', selector: '#owned', autoWait: false, waitTimeoutMs: 1 } };
  return { context, responses, actionCalls, run: () => context.handleCommand(command) };
}

const RESOLVED = { ok: true, matched_count: 1, action_point: { x: 3, y: 4 } };

function pageActionFailure(stage) {
  const result = { ok: false, error_code: 'CHROME_DOM_ACTION_UNSUPPORTED', error_detail: 'page threw', events_dispatched: [] };
  if (stage !== undefined) {
    result.dispatch_stage = stage;
  }
  return result;
}

async function failureResponse(actionResult) {
  const f = fixture({ resolveResult: RESOLVED, actionResult });
  await f.run();
  assert.deepEqual(f.actionCalls, ['click'], 'precondition: the page action ran once');
  assert.equal(f.responses.length, 1);
  assert.equal(f.responses[0][1], false, 'precondition: the command failed');
  return f.responses[0][3];
}

test('must_fail_after_dispatch_stage_is_never_dropped_before_the_wire', async () => {
  const payload = await failureResponse(pageActionFailure('after_dispatch'));
  assert.equal(payload.code, 'CHROME_DOM_ACTION_UNSUPPORTED');
  assert.equal(payload.dispatch_stage, 'after_dispatch');
});

test('must_fail_not_started_stage_from_the_page_is_never_dropped_before_the_wire', async () => {
  // handleDomAction already marked the dispatch as started, so only the copy of
  // the page's own stage can prove this failure happened before any event.
  const payload = await failureResponse(pageActionFailure('not_started'));
  assert.equal(payload.dispatch_stage, 'not_started');
});

test('must_pass_page_failure_without_a_stage_sends_none_so_delivery_stays_unknown', async () => {
  const payload = await failureResponse(pageActionFailure(undefined));
  assert.equal(payload.dispatch_stage, undefined);
  const bogus = await failureResponse(pageActionFailure('bogus_stage'));
  assert.equal(bogus.dispatch_stage, undefined, 'only the two known stages are evidence');
});

test('must_fail_preflight_failure_before_the_first_dispatch_reports_not_started', async () => {
  const f = fixture({
    resolveResult: { ok: false, error_code: 'CHROME_DOM_ELEMENT_NOT_FOUND', error_detail: 'no match' },
    actionResult: RESOLVED
  });
  await f.run();
  assert.deepEqual(f.actionCalls, [], 'precondition: no page action ran');
  assert.equal(f.responses.length, 1);
  assert.equal(f.responses[0][1], false);
  assert.equal(f.responses[0][3].code, 'CHROME_DOM_ELEMENT_NOT_FOUND');
  assert.equal(f.responses[0][3].dispatch_stage, 'not_started');
});

test('must_fail_error_payload_drops_no_known_dispatch_stage', () => {
  const f = fixture({ resolveResult: RESOLVED, actionResult: RESOLVED });
  const stageOf = dispatchStage => {
    const error = Object.assign(new Error('boom'), { code: 'CHROME_DOM_ACTION_UNSUPPORTED' });
    if (dispatchStage !== undefined) {
      error.dispatchStage = dispatchStage;
    }
    return f.context.errorPayload(error).dispatch_stage;
  };
  assert.equal(stageOf('after_dispatch'), 'after_dispatch');
  assert.equal(stageOf('not_started'), 'not_started');
  assert.equal(stageOf(undefined), undefined);
  assert.equal(stageOf('bogus_stage'), undefined);
});

// Source guard (no DOM is available to run performDomActionInPage): the in-page
// failure must put the computed stage on the result that handleDomAction copies.
test('must_fail_in_page_failure_result_carries_the_computed_dispatch_stage', () => {
  const marker = source.indexOf('// #189 B8: a throw after any dispatched event');
  assert.notEqual(marker, -1, 'domAction catch stage block');
  const call = source.indexOf('return fail(', marker);
  assert.notEqual(call, -1, 'domAction catch failure return');
  const end = source.indexOf('\n    });', call);
  assert.notEqual(end, -1, 'end of the fail(...) call');
  assert.match(source.slice(call, end), /dispatch_stage:\s*dispatchStage/);
});
