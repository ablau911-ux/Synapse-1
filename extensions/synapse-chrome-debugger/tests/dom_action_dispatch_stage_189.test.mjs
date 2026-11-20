// #189 B8: performDomActionInPage's catch must report dispatch_stage
// "not_started" when no event was recorded (a validation throw before the
// first event) and "after_dispatch" once any event was dispatched. Runs the
// production stage computation from the catch block itself.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const source = readFileSync(new URL('../service_worker.js', import.meta.url), 'utf8');

function productionDispatchStage() {
  const start = source.indexOf('async function performDomActionInPage(');
  assert.notEqual(start, -1, 'performDomActionInPage');
  const marker = source.indexOf('// #189 B8: a throw after any dispatched event', start);
  assert.notEqual(marker, -1, 'domAction catch stage block');
  const end = source.indexOf('return fail(', marker);
  assert.notEqual(end, -1, 'domAction catch failure return');
  const body = source.slice(marker, end);
  return vm.runInNewContext(`(eventsDispatched, error) => { ${body}; return dispatchStage; }`);
}

const stageOf = productionDispatchStage();

test('must_fail_validation_throw_before_any_event_reports_not_started', () => {
  const validation = Object.assign(new Error('element is not a checkbox'), { code: 'CHROME_DOM_ACTION_UNSUPPORTED' });
  assert.equal(stageOf([], validation), 'not_started');
  assert.equal(stageOf([], new TypeError('cannot read properties of null')), 'not_started');
  assert.equal(stageOf([], undefined), 'not_started');
});

test('must_pass_throw_after_dispatched_events_reports_after_dispatch', () => {
  const late = new Error('restoreWindowOpen threw after click');
  assert.equal(stageOf(['pointerdown'], late), 'after_dispatch');
  assert.equal(stageOf(['focus', 'pointerdown', 'mousedown', 'click'], late), 'after_dispatch');
});
