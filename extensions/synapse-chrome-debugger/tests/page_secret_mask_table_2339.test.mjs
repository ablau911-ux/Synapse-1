// Frozen behaviour table of Gitea #2339 (roadmap #571): rows M01-M23.
// Production code runs as is (service_worker.js page functions, the Rust raw
// literals, both copies of passwordReadback). This file only adds the fake-DOM
// parts the shared fixture cannot express: element text (script, style, p),
// <template> content, and an inert document that imports them.
// Coders never edit this file.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';
import { Element, readRoute } from './password_fixture_244.mjs';

const marker = '[REDACTED]';
const worker = readFileSync(new URL('../service_worker.js', import.meta.url), 'utf8');
const rawHelper = readFileSync(new URL('../../../crates/synapse-a11y/src/cdp_password_readback.js', import.meta.url), 'utf8');

// Fake-DOM extension: an element that serializes its text and its template content.
class PageElement extends Element {
  constructor(tag, attrs = {}, text = '') {
    const lower = tag.toLowerCase();
    super(tag, attrs, lower === 'input' ? attrs.value ?? '' : undefined);
    this.textContent = text;
    this.innerText = text;
    if (lower === 'template') this.content = new PageElement('#document-fragment');
  }
  get innerHTML() {
    const children = this.content ? this.content.children : this.children;
    return children.length ? children.map(child => child.outerHTML).join('') : String(this.textContent ?? '');
  }
  get outerHTML() {
    const tag = this.tagName.toLowerCase();
    const attrs = this.attributes.map(a => ` ${a.name}="${a.value}"`).join('');
    return tag === 'input' ? `<input${attrs}>` : `<${tag}${attrs}>${this.innerHTML}</${tag}>`;
  }
}

// A document without the page's registry: copies attributes, value, text and
// template content, never redacts (same contract as the shared fixture's).
function inertDocument() {
  const inert = { defaultView: null };
  inert.importNode = function importNode(node, deep = false) {
    const copy = new PageElement(node.tagName, node.attrs, node.textContent);
    copy.ownerDocument = inert;
    copy.isConnected = false;
    copy.innerText = node.innerText;
    if ('value' in node) copy.value = node.value;
    if (deep) {
      for (const child of node.children) copy.append(importNode(child, true));
      for (const child of node.content?.children ?? []) copy.content.append(importNode(child, true));
    }
    return copy;
  };
  return inert;
}

const el = (tag, attrs = {}, text = '', children = []) => {
  const element = new PageElement(tag, attrs, text);
  for (const child of children) (element.content ?? element).append(child);
  return element;
};

// Every top-level page function of the worker, run as is. Same extraction as
// password_fixture_244.mjs, with the closing line anchored to exactly `}`:
// the fixture's pattern stops at the `}) {` line of
// `function hiddenTabRenderDecision({` (since fd5fdc64), so its platform()
// cannot load today's worker. Switch to platform() once that is fixed.
const declarations = [...worker.matchAll(/^(?:async )?function \w+\([^]*?^\}$/gm)]
  .map(match => match[0]).join('\n');
const style = { display: 'block', visibility: 'visible', opacity: '1', overflow: 'visible' };
class ShadowRoot extends Element {}

// One page: <html><body>...elements</body></html>, with the same globals as the fixture's platform().
function page(elements) {
  const html = el('html'), body = el('body');
  html.append(body);
  for (const element of elements) body.append(element);
  const document = {
    implementation: { createHTMLDocument: inertDocument },
    documentElement: html, body, activeElement: body,
    title: 'Secret mask fixture', readyState: 'complete', defaultView: { getComputedStyle: () => style },
    elementFromPoint: () => body, getSelection: () => null,
    querySelector: selector => html.querySelector(selector),
    querySelectorAll: selector => html.querySelectorAll(selector),
    getElementById: id => html.querySelector(`#${id}`)
  };
  const own = node => { node.ownerDocument = document; node.children.forEach(own); node.content?.children.forEach(own); };
  own(html);
  const window = { scrollX: 0, scrollY: 0, innerWidth: 800, innerHeight: 600, devicePixelRatio: 1,
    getComputedStyle: () => style, HTMLInputElement: Element };
  const context = vm.createContext({ Element, ShadowRoot, HTMLInputElement: Element,
    document, window, location: { href: 'https://fixture.test/' },
    history: { length: 1 }, getComputedStyle: () => style,
    Event: class Event { constructor(type) { this.type = type; } },
    requestAnimationFrame: callback => callback(0), setTimeout, clearTimeout, console });
  vm.runInContext(declarations, context);
  return { context, html, body, field: body, path: '0.0', original: html.outerHTML };
}

// route 'bridge' = service_worker.js copy; 'raw' = cdp_password_readback.js copy.
async function readContent(pg, route) {
  if (route === 'raw') vm.runInContext(rawHelper, pg.context);
  const { result } = await readRoute(route === 'raw' ? 'raw-content' : 'content', {}, pg);
  assert.equal(typeof result.html, 'string', `${route} content returned no html`);
  return result.html;
}

async function readInspect(pg, element, route) {
  if (route === 'raw') vm.runInContext(rawHelper, pg.context);
  const path = `0.0.${pg.body.children.indexOf(element)}`;
  const { result } = await readRoute(route === 'raw' ? 'raw-inspect' : 'inspect', {}, { ...pg, path, field: element });
  if (route === 'raw') return result;
  assert.equal(result.ok, true, `bridge inspect failed: ${JSON.stringify(result)}`);
  return result.element;
}

const count = (text, part) => text.split(part).length - 1;
const absent = (text, secret, where) =>
  assert.equal(String(text).includes(secret), false, `${where}: secret ${secret} is in the output`);
const present = (text, part, where) =>
  assert.ok(String(text).includes(part), `${where}: expected ${part} in ${text}`);

// Row givens.
const hiddenToken = 'ghp_FAKEP1310FAKEP1310FAKE0000111122';
const scriptLiteral = 'tok-SYNTH-9f3a2c71';
const hiddenInput = () => el('input', { type: 'hidden', name: 'session_token', value: hiddenToken });
const cardInput = () => el('input', { id: 'cc', autocomplete: 'cc-number', value: '4111111111111111' });
const apiKeyInput = () => el('input', { type: 'text', name: 'api_key', value: 'plain-FAKE-key' });
const cityInput = () => el('input', { type: 'text', name: 'city', value: 'Haifa' });
const inlineScript = () => el('script', {}, `localStorage.setItem('auth_token','${scriptLiteral}')`);

const contentRows = [
  ['M01_password_input', () => [el('input', { type: 'password', value: 'FAKE-pw-1' })], html => {
    absent(html, 'FAKE-pw-1', 'M01'); present(html, `value="${marker}"`, 'M01');
  }],
  ['M02_hidden_input_token', () => [hiddenInput()], html => {
    absent(html, hiddenToken, 'M02'); present(html, `value="${marker}"`, 'M02'); present(html, 'name="session_token"', 'M02');
  }],
  ['M03_card_number_autocomplete', () => [cardInput()], html => {
    absent(html, '4111111111111111', 'M03'); present(html, `value="${marker}"`, 'M03'); present(html, 'id="cc"', 'M03');
  }],
  ['M04_cvv_autocomplete', () => [el('input', { id: 'cvv', autocomplete: 'cc-csc', value: '737' })], html => {
    absent(html, '737', 'M04'); present(html, `value="${marker}"`, 'M04');
  }],
  ['M05_one_time_code_autocomplete', () => [el('input', { type: 'text', autocomplete: 'section-a one-time-code', value: '482913' })], html => {
    absent(html, '482913', 'M05'); present(html, `value="${marker}"`, 'M05');
  }],
  ['M06_credential_word_in_name', () => [apiKeyInput()], html => {
    absent(html, 'plain-FAKE-key', 'M06'); present(html, `value="${marker}"`, 'M06');
  }],
  ['M07_token_shaped_value', () => [el('input', { type: 'text', name: 'q', value: 'ghp_FAKE0000111122223333' })], html => {
    absent(html, 'ghp_FAKE0000111122223333', 'M07'); present(html, `value="${marker}"`, 'M07');
  }],
  ['M08_ordinary_city_value', () => [cityInput()], html => {
    present(html, 'value="Haifa"', 'M08'); absent(html, marker, 'M08');
  }],
  ['M09_inline_script_literal', () => [inlineScript()], html => {
    absent(html, scriptLiteral, 'M09'); present(html, `<script>${marker}</script>`, 'M09');
  }],
  ['M10_json_script_token', () => [el('script', { type: 'application/json', id: 'data' }, '{"token":"FAKE-json-token"}')], html => {
    absent(html, 'FAKE-json-token', 'M10'); present(html, 'id="data"', 'M10');
  }],
  ['M11_external_script_src', () => [el('script', { src: '/app.js' })], html => {
    present(html, 'src="/app.js"', 'M11');
  }],
  ['M12_plain_paragraph', () => [el('p', {}, 'Hello')], (html, pg) => {
    assert.equal(html, pg.original, 'M12: HTML changed');
  }],
  ['M13_uppercase_hidden_and_cc', () => [
    el('input', { type: 'HIDDEN', value: 'FAKE-upper' }),
    el('input', { autocomplete: 'CC-Number', value: 'FAKE-cc-upper' })
  ], html => {
    absent(html, 'FAKE-upper', 'M13'); absent(html, 'FAKE-cc-upper', 'M13');
    assert.equal(count(html, `value="${marker}"`), 2, `M13: both values must be ${marker}: ${html}`);
  }],
  ['M14_hidden_input_in_template', () => [el('template', {}, '', [el('input', { type: 'hidden', name: 'csrf', value: 'FAKE-tpl-secret' })])], html => {
    absent(html, 'FAKE-tpl-secret', 'M14'); present(html, `value="${marker}"`, 'M14'); present(html, '<template>', 'M14');
  }],
  ['M15_style_text', () => [el('style', {}, '.a{color:red}')], (html, pg) => {
    assert.equal(html, pg.original, 'M15: HTML changed');
  }],
  ['M16_submit_and_checkbox', () => [
    el('input', { type: 'submit', value: 'Sign in' }),
    el('input', { type: 'checkbox', name: 'agree', value: 'yes' })
  ], (html, pg) => {
    present(html, 'value="Sign in"', 'M16'); present(html, 'value="yes"', 'M16');
    assert.equal(html, pg.original, 'M16: HTML changed');
  }]
];

const inspectRows = [
  ['M17_inspect_hidden_input', () => [hiddenInput()], element => {
    absent(JSON.stringify(element), hiddenToken, 'M17');
    assert.equal(element.attributes.value, marker, 'M17: attributes.value');
    assert.equal(element.input_value, marker, 'M17: input_value');
    present(element.outer_html, `value="${marker}"`, 'M17 outer_html');
    assert.equal(element.attributes.name, 'session_token', 'M17: name attribute kept');
  }],
  ['M18_inspect_inline_script', () => [inlineScript()], element => {
    for (const field of ['inner_html', 'outer_html', 'inner_text', 'text_content']) {
      assert.equal(typeof element[field], 'string', `M18: ${field} missing`);
      absent(element[field], scriptLiteral, `M18 ${field}`);
    }
    present(element.outer_html, '<script>', 'M18 outer_html');
  }],
  ['M19_inspect_form_with_secrets', () => [el('form', {}, '', [hiddenInput(), cardInput()])], element => {
    for (const field of ['outer_html', 'inner_html']) {
      absent(element[field], hiddenToken, `M19 ${field}`);
      absent(element[field], '4111111111111111', `M19 ${field}`);
      assert.equal(count(element[field], `value="${marker}"`), 2, `M19 ${field}: both values must be ${marker}: ${element[field]}`);
    }
  }],
  ['M20_inspect_city_value', () => [cityInput()], element => {
    assert.equal(element.input_value, 'Haifa', 'M20: input_value');
  }]
];

async function runContentRow([, given, then], route) {
  const pg = page(given());
  const html = await readContent(pg, route);
  then(html, pg);
  return html;
}

async function runInspectRow([, given, then], route) {
  const pg = page(given());
  const element = await readInspect(pg, pg.body.children[0], route);
  then(element);
  const { outer_html, inner_html, inner_text, text_content, attributes, input_value, is_password } = element;
  return { outer_html, inner_html, inner_text, text_content, attributes, input_value, is_password };
}

for (const row of contentRows) test(row[0], () => runContentRow(row, 'bridge'));
for (const row of inspectRows) test(row[0], () => runInspectRow(row, 'bridge'));

test('M21_raw_cdp_route_same_answers', async () => {
  for (const row of contentRows) {
    const raw = await runContentRow(row, 'raw');
    assert.equal(raw, await runContentRow(row, 'bridge'), `M21 ${row[0]}: raw and bridge differ`);
  }
  for (const row of inspectRows) {
    const raw = await runInspectRow(row, 'raw');
    assert.deepEqual(raw, await runInspectRow(row, 'bridge'), `M21 ${row[0]}: raw and bridge differ`);
  }
});

test('M22_helper_copies_identical', () => {
  const install = '\n  if (element === undefined) {\n    globalThis.passwordReadback = passwordReadback;\n    return;\n  }';
  const bridge = worker.match(/^function passwordReadback\([^]*?^\}/m)?.[0];
  assert.equal(typeof bridge, 'string', 'M22: bridge copy not found in service_worker.js');
  assert.ok(bridge.includes(install), 'M22: bridge copy lost its install lines');
  assert.equal(bridge.replace(install, ''), rawHelper.trimEnd(), 'M22: the two helper copies differ');
});

test('M23_aria_snapshot_asks_helper', async () => {
  const pg = page([hiddenInput(), cardInput(), apiKeyInput(), cityInput()]);
  const { result } = await readRoute('aria', {}, { ...pg, path: '0.0' });
  assert.equal(result.ok, true, `M23: aria failed ${JSON.stringify(result)}`);
  const redacted = index => result.nodes.find(node => node.element_id === `0.0.${index}`)?.redacted;
  assert.deepEqual([0, 1, 2, 3].map(redacted), [true, true, true, false], 'M23: redacted for M02, M03, M06, M08');
});
