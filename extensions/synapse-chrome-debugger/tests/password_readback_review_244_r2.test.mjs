// r2 D2/minor: execute production serializers/actions, fixture only platform DOM.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';
import { Element, platform, readRoute } from './password_fixture_244.mjs';

const marker = '[REDACTED]';
const cdp = readFileSync(new URL('../../../crates/synapse-a11y/src/cdp_action.rs', import.meta.url), 'utf8');
const rawPrivacy = readFileSync(new URL('../../../crates/synapse-a11y/src/cdp_password_readback.js', import.meta.url), 'utf8');

function reactionFixture(type = 'password') {
  const fixture = platform({ type });
  const document = fixture.context.document;
  let constructors = 0;
  const probe = new Element('x-password-readback-probe');
  probe.ownerDocument = document;
  probe.cloneNode = function (deep) {
    // A registered custom element cloned in its live document upgrades.
    constructors += 1;
    return Element.prototype.cloneNode.call(this, deep);
  };
  fixture.body.append(probe);
  document.implementation = {
    createHTMLDocument() {
      const inert = {};
      inert.importNode = function importNode(node, deep) {
        // Platform import in a document without the page's element registry.
        // Never invoke the source node's live-document clone operation.
        const copy = new Element(node.tagName, node.attrs, node.value);
        copy.ownerDocument = inert;
        copy.textContent = node.textContent;
        copy.innerText = node.innerText;
        if (deep) for (const child of node.children) copy.append(importNode(child, true));
        return copy;
      };
      return inert;
    }
  };
  return { ...fixture, constructors: () => constructors };
}

test('must_pass_reaction_fixture_distinguishes_live_clone_from_inert_import_244_d2', () => {
  const fixture = reactionFixture();
  assert.equal(fixture.constructors(), 0);
  fixture.body.cloneNode(true);
  assert.equal(fixture.constructors(), 1, 'negative control: live clone activates page constructor');
  const inert = fixture.context.document.implementation.createHTMLDocument('');
  const copy = inert.importNode(fixture.body, true);
  assert.equal(fixture.constructors(), 1, 'inert import cannot activate page constructor');
  assert.ok(copy.outerHTML.includes(fixture.secret), 'fixture itself does no redaction');
});

for (const route of ['content', 'inspect', 'raw-content', 'raw-inspect']) {
  test(`must_fail_${route}_password_readback_never_runs_page_constructors_244_d2`, async () => {
    const fixture = reactionFixture();
    if (route.startsWith('raw-')) vm.runInContext(rawPrivacy, fixture.context);
    const before = fixture.constructors();
    const { result } = await readRoute(route, { ancestor: true }, fixture);
    assert.equal(fixture.constructors(), before,
      `${route} readback invoked a page constructor through live-document cloneNode`);
    const html = result.html ?? result.element?.outer_html ?? result.outer_html;
    assert.equal(typeof html, 'string', 'route must actually return serialized HTML');
    assert.ok(html.includes(marker), 'inert output must still redact nested password');
    assert.ok(html.includes('keep-ordinary-244'), 'ordinary value survives redaction');
    assert.equal(html.includes(fixture.secret), false);
    assert.equal(fixture.field.value, fixture.secret, 'source password remains intact');
    assert.equal(fixture.field.getAttribute('value'), fixture.secret);
  });
}

function cdpPrimitive() {
  const anchor = 'const CDP_DOM_PRIMITIVE_FUNCTION: &str = r#"';
  const start = cdp.indexOf(anchor);
  assert.ok(start >= 0, 'production CDP primitive anchor');
  const end = cdp.indexOf('"#;', start + anchor.length);
  assert.ok(end > start, 'production CDP primitive terminator');
  return vm.runInNewContext(`(${cdp.slice(start + anchor.length, end)})`, {
    Event: class Event { constructor(type) { this.type = type; } },
    passwordReadback: vm.runInNewContext(`(${rawPrivacy})`)
  });
}

function selectedField(type, secret = 'selection-secret-244') {
  const fixture = platform({ type, secret });
  const { field, context } = fixture;
  context.document.dispatchEvent = () => true;
  field.focus = () => { context.document.activeElement = field; };
  field.select = () => { field.selectionStart = 0; field.selectionEnd = field.value.length; };
  field.selectionStart = 0;
  field.selectionEnd = Math.min(3, secret.length);
  return fixture;
}

for (const route of ['cdp', 'bridge']) {
  function perform(fixture) {
    if (route === 'cdp') return cdpPrimitive()(fixture.field, 'selectText');
    // This reader is nested in the production page action. Extract that actual
    // declaration and its platform helpers; do not copy its selection logic.
    const worker = readFileSync(new URL('../service_worker.js', import.meta.url), 'utf8');
    const start = worker.indexOf('  function performSelectText(element, events) {');
    const end = worker.indexOf('\n  function performCheckState(', start);
    assert.ok(start >= 0 && end > start, 'production bridge selection reader');
    const selectionStart = worker.indexOf('  function selectionSummary(element) {', end);
    const selectionEnd = worker.indexOf('\n  function ', selectionStart + 3);
    assert.ok(selectionStart > end && selectionEnd > selectionStart, 'production bridge selection summary');
    return vm.runInNewContext(`(() => {
      ${worker.slice(start, end)}
      ${worker.slice(selectionStart, selectionEnd)}
      return performSelectText;
    })()`, {
      Event: class Event { constructor(type) { this.type = type; } },
      isSelectableValueControl: () => true,
      elementTextValue: element => String(element.value),
      actionError: (code, detail) => new Error(`${code}: ${detail}`),
      ERROR_POSTCONDITION_FAILED: 'POSTCONDITION_FAILED',
      ERROR_ACTION_UNSUPPORTED: 'ACTION_UNSUPPORTED',
      passwordReadback: fixture.context.passwordReadback
    })(fixture.field, []);
  }

  test(`must_pass_${route}_selectText_ordinary_value_and_selection_are_readable_244`, () => {
    const fixture = selectedField('text');
    const result = perform(fixture);
    assert.equal(result.action_readback?.selected_text ?? result.selected_text, fixture.secret);
    assert.equal(fixture.field.value, fixture.secret);
    assert.equal(fixture.field.selectionEnd, fixture.secret.length);
  });

  test(`must_fail_${route}_selectText_password_has_fixed_marker_and_flag_244_minor`, () => {
    for (const type of ['password', 'PASSWORD']) {
      const fixture = selectedField(type);
      const result = perform(fixture);
      const readback = result.action_readback ?? result;
      for (const selection of [readback, readback.before_selection, readback.after_selection]) {
        assert.equal(selection.selected_text, marker, 'password selection requires a fixed marker');
        assert.equal(selection.is_password, true, 'password selection must carry is_password');
      }
      assert.equal(JSON.stringify(result).includes(fixture.secret), false);
      assert.equal(fixture.field.value, fixture.secret);
      assert.equal(fixture.field.selectionEnd, fixture.secret.length, 'redaction must preserve selection action');
    }
  });
}
