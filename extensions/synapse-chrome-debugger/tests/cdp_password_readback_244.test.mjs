// Executes the actual Runtime.evaluate expression in cdp_active_element_state.
// No copied production algorithm; only document/element platform objects are fixtures.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const rust = readFileSync(new URL('../../../crates/synapse-a11y/src/cdp_action.rs', import.meta.url), 'utf8');
const fnStart = rust.indexOf('pub async fn cdp_active_element_state(');
const rawStart = rust.indexOf('r#"', fnStart) + 3;
const rawEnd = rust.indexOf('"#', rawStart);
assert.ok(fnStart >= 0 && rawStart > fnStart && rawEnd > rawStart);
const expression = rust.slice(rawStart, rawEnd).replaceAll('{target_id_json}', '"owned-244"')
  .replaceAll('{{', '{').replaceAll('}}', '}');

function read(type, value) {
  const attrs = { type, name: 'fixture' };
  const element = { tagName: 'INPUT', value, id: 'field', parentElement: null,
    selectionStart: 0, selectionEnd: value.length, getAttribute: key => attrs[key] ?? null };
  assert.equal(element.value, value);
  const state = vm.runInNewContext(expression, { document: { activeElement: element } });
  assert.equal(element.value, value, 'readback must not change DOM source of truth');
  return state;
}

test('must_pass_nonpassword_readback_preserves_exact_value_and_selection', () => {
  for (const value of ['', '  text\nשלום  ']) {
    const state = read('text', value);
    assert.equal(state.value, value);
    assert.equal(state.selection_end, value.length);
    assert.equal(state.target_id, 'owned-244');
  }
});

test('must_fail_password_value_cannot_escape_active_element_readback', () => {
  for (const type of ['password', 'PASSWORD']) {
    const state = read(type, 'SECRET_244');
    assert.equal(state.value, '[REDACTED]', 'frozen #244 r2 requires a fixed redaction marker');
    assert.equal(state.is_password, true, 'password evidence must accompany suppression');
    assert.equal(JSON.stringify(state).includes('SECRET_244'), false);
  }
});

// Variant: real router selection readback also returns a value substring.
const router = readFileSync(new URL('../../../crates/synapse-mcp/src/server/background_router.rs', import.meta.url), 'utf8');
const selectionStart = router.indexOf('const TARGET_ACT_SET_SELECTION_JS: &str = r"')
  + 'const TARGET_ACT_SET_SELECTION_JS: &str = r"'.length;
const selectionEnd = router.indexOf('";', selectionStart);
assert.ok(selectionStart > 0 && selectionEnd > selectionStart);
const select = vm.runInNewContext(`(${router.slice(selectionStart, selectionEnd)})`, {
  Event: class Event { constructor(type) { this.type = type; } }
});
function selectFixture(type) {
  return { value: 'SECRET_244', selectionStart: 0, selectionEnd: 0, tagName: 'INPUT',
    getAttribute: key => key === 'type' ? type : null,
    focus() {}, dispatchEvent() {}, setSelectionRange(start, end) {
      this.selectionStart = start; this.selectionEnd = end;
    } };
}
test('must_pass_ordinary_selection_readback_preserves_selected_text', () => {
  const field = selectFixture('text');
  assert.equal(field.selectionEnd, 0);
  assert.equal(select(field, 0, 6).selected_text, 'SECRET');
  assert.equal(field.value, 'SECRET_244');
});
test('must_fail_password_selection_readback_never_returns_plaintext_substring', () => {
  const field = selectFixture('password');
  assert.equal(field.value, 'SECRET_244');
  const result = select(field, 0, 6);
  assert.equal(result.selected_text, '[REDACTED]', 'password selection readback is also private');
  assert.equal(result.is_password, true, 'redaction must identify password readback');
  assert.equal(field.value, 'SECRET_244');
});
