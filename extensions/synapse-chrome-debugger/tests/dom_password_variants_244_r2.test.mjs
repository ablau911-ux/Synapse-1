import assert from 'node:assert/strict';
import test from 'node:test';
import { platform, readRoute } from './password_fixture_244.mjs';

const marker = '[REDACTED]';
const routes = ['inspect', 'content', 'assert', 'raw-inspect', 'raw-content', 'raw-assert', 'duplicate'];
const row = (route, result) => result.element ?? result.element_state ?? result;

function privateOutput(route, result, secret) {
  assert.equal(JSON.stringify(result).includes(secret), false, `${route} leaked password plaintext`);
  const state = row(route, result);
  if (route.endsWith('content')) assert.ok(result.html.includes(marker), `${route} needs fixed HTML marker`);
  else {
    assert.equal(state.is_password, true, `${route} must identify the password field`);
    assert.equal(state.input_value ?? state.value, marker, `${route} needs fixed value marker`);
    assert.equal(state.attributes.value, marker, `${route} needs fixed attribute marker`);
    assert.equal(state.attributes.id, 'plainfield');
    assert.equal(state.attributes.name, 'login');
  }
}

for (const route of routes) {
  test(`must_pass_${route}_ordinary_input_retains_value_and_identity`, async () => {
    const { result, fixture } = await readRoute(route, { type: 'text', secret: 'ordinary-244' });
    assert.ok(JSON.stringify(result).includes('ordinary-244'));
    if (!route.endsWith('content')) {
      assert.equal(row(route, result).attributes.id, 'plainfield');
      assert.equal(row(route, result).input_value ?? row(route, result).value, 'ordinary-244');
    }
    assert.equal(fixture.field.value, 'ordinary-244');
  });
  test(`must_fail_${route}_password_never_serializes_attribute_or_live_value`, async () => {
    for (const options of [{}, { type: 'PASSWORD' }, { secret: 'short' }, { type: 'text', runtime: true }]) {
      const { result, fixture } = await readRoute(route, options);
      privateOutput(route, result, fixture.secret);
      assert.equal(fixture.field.value, fixture.secret, 'privacy readback must not erase the real field');
      assert.equal(fixture.field.getAttribute('value'), fixture.secret);
    }
  });
}

test('must_fail_bridge_inspect_shadow_and_frame_document_passwords_are_private', async () => {
  for (const options of [{ shadow: true }, { frame: true }, { shadow: true, frame: true }]) {
    const { result, fixture } = await readRoute('inspect', options);
    assert.equal(result.ok, true, 'must resolve actual shadow/frame-local field');
    assert.equal(result.element.tag_name, 'INPUT');
    privateOutput('inspect', result, fixture.secret);
    assert.equal(fixture.field.value, fixture.secret);
  }
});

test('must_fail_assert_all_candidates_and_shadow_value_maps_are_private', async () => {
  const { result, fixture } = await readRoute('assert', { shadow: true });
  assert.equal(result.ok, true); assert.equal(result.match_count, 1);
  privateOutput('assert', result, fixture.secret);
  privateOutput('assert', result.element_states[0], fixture.secret);
});

for (const route of ['inspect', 'raw-inspect']) {
  test(`must_fail_${route}_ancestor_html_redacts_nested_password_and_keeps_text_input`, async () => {
    const { result, fixture } = await readRoute(route, { ancestor: true });
    assert.equal(JSON.stringify(result).includes(fixture.secret), false, 'ancestor HTML leaks descendants');
    const state = row(route, result);
    assert.ok(state.inner_html.includes(marker));
    assert.ok(state.inner_html.includes('keep-ordinary-244'));
    assert.equal(fixture.field.value, fixture.secret);
  });
}

test('must_fail_inspect_empty_password_and_zero_html_limit_keep_password_flag', async () => {
  for (const options of [{ secret: '' }, { max: 0 }]) {
    const { result, fixture } = await readRoute('inspect', options);
    if (fixture.secret) assert.equal(JSON.stringify(result).includes(fixture.secret), false);
    assert.equal(result.element.is_password, true);
    assert.equal(result.element.input_value, marker);
    assert.equal(result.element.attributes.value, marker);
    assert.equal(fixture.field.value, fixture.secret);
  }
});

for (const route of ['aria', 'active']) {
  test(`must_fail_${route}_suppressed_password_has_fixed_marker_and_explicit_flag`, async () => {
    const { result, fixture } = await readRoute(route, { shadow: true });
    assert.equal(JSON.stringify(result).includes(fixture.secret), false);
    const state = result.nodes?.[0] ?? result;
    assert.equal(state.is_password, true);
    assert.equal(state.value, marker);
    assert.equal(fixture.field.value, fixture.secret);
  });
}

test('must_fail_runtime_type_change_is_evaluated_on_every_readback', async () => {
  const fixture = platform({ type: 'text' });
  const before = await readRoute('inspect', {}, fixture);
  assert.equal(before.result.element.input_value, fixture.secret);
  fixture.field.setAttribute('type', 'password');
  const after = await readRoute('inspect', {}, fixture);
  privateOutput('inspect', after.result, fixture.secret);
  assert.equal(fixture.field.value, fixture.secret);
});

test('must_pass_password_form_typing_still_changes_real_value_without_public_plaintext', async () => {
  for (const shadow of [false, true]) {
    const { result, fixture } = await readRoute('type', { shadow, text: 'new-credential-244' });
    assert.equal(result.ok, true, 'redaction must not break internal exact-value verification');
    assert.equal(fixture.field.value, 'new-credential-244');
    assert.equal(JSON.stringify(result).includes(fixture.secret), false);
    assert.equal(JSON.stringify(result).includes('new-credential-244'), false);
  }
});
