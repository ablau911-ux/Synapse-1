// Frozen behaviour table of Gitea #2339 (roadmap #571), table 2: rows M24-M29.
// Rows come from the pre-release review (R-SECRET / F2: a credential marked
// only by its <label>; a script under an inspected ancestor).
// Production code runs as is (service_worker.js page functions, the Rust raw
// literals, both copies of passwordReadback). Loader, routes and helpers are
// copied from page_secret_mask_table_2339.test.mjs; this file adds to the fake
// DOM only what these rows need:
//  - text as a leading text node plus children, so a container's textContent
//    holds its descendants' text and innerText skips <script>/<style>, as a
//    browser does;
//  - `labels` on <input>, the one label property passwordReadback reads
//    (`node.labels`, then each label's textContent). A connected input gets
//    the document's label[for=id] elements and its wrapping <label> (no `for`,
//    or `for` equal to its id). A disconnected input (the inert clone the
//    helper serializes) gets an empty list: Chromium roots the labels list at
//    the tree scope only for connected nodes, else at the input itself, which
//    never contains a label.
// Coders never edit this file.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';
import { Element, readRoute } from './password_fixture_244.mjs';

const marker = '[REDACTED]';
const worker = readFileSync(new URL('../service_worker.js', import.meta.url), 'utf8');
const rawHelper = readFileSync(new URL('../../../crates/synapse-a11y/src/cdp_password_readback.js', import.meta.url), 'utf8');

const NOT_RENDERED = new Set(['SCRIPT', 'STYLE', 'TEMPLATE']);

function* descendants(node) {
  for (const child of node.children) {
    yield child;
    yield* descendants(child);
  }
}

// Fake-DOM extension: leading text + children, template content, input labels.
class PageElement extends Element {
  constructor(tag, attrs = {}, text = '') {
    const lower = tag.toLowerCase();
    super(tag, attrs, lower === 'input' ? attrs.value ?? '' : undefined);
    this.ownText = String(text ?? '');
    if (lower === 'template') this.content = new PageElement('#document-fragment');
  }
  // Setting text replaces the children, as in a browser.
  set textContent(text) { this.ownText = String(text ?? ''); this.children = []; }
  get textContent() { return this.ownText + this.children.map(child => child.textContent).join(''); }
  set innerText(text) { this.textContent = text; }
  get innerText() {
    return this.ownText + this.children.filter(child => !NOT_RENDERED.has(child.tagName))
      .map(child => child.innerText).join('');
  }
  get labels() {
    if (this.tagName !== 'INPUT') return undefined;
    if (!this.isConnected) return [];
    let root = this;
    while (root.parentNode) root = root.parentNode;
    const all = [...descendants(root)];
    const id = this.id;
    return all.filter(label => {
      if (label.tagName !== 'LABEL') return false;
      const target = label.getAttribute('for');
      if (target !== null) return id !== '' && all.find(node => node.id === target) === this;
      return [...descendants(label)].find(node => node.tagName === 'INPUT') === this;
    });
  }
  get innerHTML() {
    const children = this.content ? this.content.children : this.children;
    return this.ownText + children.map(child => child.outerHTML).join('');
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
    const copy = new PageElement(node.tagName, node.attrs, node.ownText);
    copy.ownerDocument = inert;
    copy.isConnected = false;
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

// Every top-level page function of the worker, run as is (same extraction as
// the first table).
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

const absent = (text, secret, where) =>
  assert.equal(String(text).includes(secret), false, `${where}: secret ${secret} is in the output`);
const present = (text, part, where) =>
  assert.ok(String(text).includes(part), `${where}: expected ${part} in ${text}`);

// Row givens.
const labelKey = 'plain-FAKE-label-key';
const scriptSecret = 'tok-SYNTH-inner-55bb';
const keyLabel = () => el('label', { for: 'k' }, 'API key');
const keyInput = () => el('input', { id: 'k', type: 'text', value: labelKey });

const contentRows = [
  ['M24_label_for_names_the_credential', () => [keyLabel(), keyInput()], html => {
    absent(html, labelKey, 'M24'); present(html, `value="${marker}"`, 'M24'); present(html, 'API key', 'M24');
  }],
  ['M25_wrapping_label_names_the_credential', () => [
    el('label', {}, 'Recovery code ', [el('input', { type: 'text', value: 'plain-FAKE-wrapped' })])
  ], html => {
    absent(html, 'plain-FAKE-wrapped', 'M25'); present(html, `value="${marker}"`, 'M25');
  }],
  ['M28_guard_ordinary_label_city', () => [
    el('label', { for: 'c' }, 'City'), el('input', { id: 'c', type: 'text', value: 'Haifa' })
  ], (html, pg) => {
    present(html, 'value="Haifa"', 'M28'); absent(html, marker, 'M28');
    assert.equal(html, pg.original, 'M28: HTML changed');
  }]
];

// Each inspect row names the element it inspects (index in <body>).
const inspectRows = [
  ['M26_inspect_ancestor_of_script', () => [el('div', { id: 'box' }, '', [
    el('p', {}, 'Hello'), el('script', {}, `var t='${scriptSecret}'`)
  ])], 0, element => {
    for (const field of ['text_content', 'inner_text', 'inner_html', 'outer_html']) {
      assert.equal(typeof element[field], 'string', `M26: ${field} missing`);
      absent(element[field], scriptSecret, `M26 ${field}`);
    }
    present(element.text_content, 'Hello', 'M26 text_content');
  }],
  ['M27_inspect_form_with_labelled_credential', () => [el('form', {}, '', [keyLabel(), keyInput()])], 0, element => {
    for (const field of ['outer_html', 'inner_html']) {
      assert.equal(typeof element[field], 'string', `M27: ${field} missing`);
      absent(element[field], labelKey, `M27 ${field}`);
    }
  }]
];

async function runContentRow([, given, then], route) {
  const pg = page(given());
  const html = await readContent(pg, route);
  then(html, pg);
  return html;
}

function inspectAnswer(element) {
  const { outer_html, inner_html, inner_text, text_content, attributes, input_value, is_password } = element;
  return { outer_html, inner_html, inner_text, text_content, attributes, input_value, is_password };
}

async function runInspectRow([, given, index, then], route) {
  const pg = page(given());
  const element = await readInspect(pg, pg.body.children[index], route);
  then(element);
  return inspectAnswer(element);
}

// Bridge answers without the row's assertions, for the raw-vs-bridge comparison.
async function bridgeContent([, given]) {
  return readContent(page(given()), 'bridge');
}
async function bridgeInspect([, given, index]) {
  const pg = page(given());
  return inspectAnswer(await readInspect(pg, pg.body.children[index], 'bridge'));
}

for (const row of contentRows) test(row[0], () => runContentRow(row, 'bridge'));
for (const row of inspectRows) test(row[0], () => runInspectRow(row, 'bridge'));

// M29: the same rows on the raw CDP route (second copy of the helper): the same
// row answers, and the same output as the bridge route.
for (const row of contentRows) {
  test(`M29_raw_${row[0]}`, async () => {
    const raw = await runContentRow(row, 'raw');
    assert.equal(raw, await bridgeContent(row), `M29 ${row[0]}: raw and bridge differ`);
  });
}
for (const row of inspectRows) {
  test(`M29_raw_${row[0]}`, async () => {
    const raw = await runInspectRow(row, 'raw');
    assert.deepEqual(raw, await bridgeInspect(row), `M29 ${row[0]}: raw and bridge differ`);
  });
}

// Fixture self-check (not a row): the fake DOM's labels work on the live page,
// so a red M24/M25/M27 is the helper's clone, not a missing label model.
test('fixture_live_input_labels_are_modelled', () => {
  const pg = page([keyLabel(), keyInput(), el('label', {}, 'Recovery code ', [el('input', { type: 'text' })])]);
  const [, forInput, wrap] = pg.body.children;
  assert.deepEqual(forInput.labels.map(label => label.textContent), ['API key']);
  assert.deepEqual(wrap.children[0].labels.map(label => label.textContent), ['Recovery code ']);
  assert.equal(vm.runInContext('passwordReadback', pg.context)(forInput, 'sensitive_node'), true);
  assert.equal(vm.runInContext('passwordReadback', pg.context)(wrap.children[0], 'sensitive_node'), true);
});
