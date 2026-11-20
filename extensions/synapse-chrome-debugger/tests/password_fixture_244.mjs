// Platform fixture only. All serialization/traversal comes from production.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';
import vm from 'node:vm';

const worker = readFileSync(new URL('../service_worker.js', import.meta.url), 'utf8');
const declarations = [...worker.matchAll(/^(?:async )?function \w+\([^]*?^\}$/gm)]
  .map(match => match[0]).join('\n');
const rect = { x: 8, y: 8, left: 8, top: 8, right: 168, bottom: 32, width: 160, height: 24 };
const style = { display: 'block', visibility: 'visible', opacity: '1', overflow: 'visible' };

export class Element {
  constructor(tag, attrs = {}, value) {
    this.tagName = tag.toUpperCase(); this.attrs = { ...attrs }; this.children = [];
    this.parentElement = null; this.parentNode = null; this.nodeType = 1;
    this.isConnected = true; this.offsetWidth = 160; this.offsetHeight = 24;
    this.textContent = ''; this.innerText = ''; this.disabled = false; this.readOnly = false;
    if (value !== undefined) this.value = value;
  }
  get id() { return this.attrs.id ?? ''; }
  get type() { return this.attrs.type ?? 'text'; }
  get attributes() { return Object.entries(this.attrs).map(([name, value]) => ({ name, value })); }
  get innerHTML() { return this.children.map(child => child.outerHTML).join(''); }
  get outerHTML() {
    const attrs = this.attributes.map(a => ` ${a.name}="${a.value}"`).join('');
    if (this.tagName === 'INPUT') return `<input${attrs}>`;
    return `<${this.tagName.toLowerCase()}${attrs}>${this.innerHTML}</${this.tagName.toLowerCase()}>`;
  }
  setAttribute(key, value) { this.attrs[key] = String(value); }
  getAttribute(key) { return this.attrs[key] ?? null; }
  hasAttribute(key) { return key in this.attrs; }
  removeAttribute(key) { delete this.attrs[key]; }
  getBoundingClientRect() { return rect; }
  getClientRects() { return [rect]; }
  getAnimations() { return []; }
  dispatchEvent() { return true; }
  setSelectionRange(start, end) { this.selectionStart = start; this.selectionEnd = end; }
  getRootNode() { return this.parentNode?.getRootNode?.() ?? this; }
  contains(node) { return node === this || this.children.some(child => child.contains(node)); }
  matches(selector) {
    if (selector === '*') return true;
    if (selector.startsWith('#')) return selector.slice(1) === this.id;
    const parsed = selector.match(/^(\w+)?(?:\[([\w-]+)(?:=["']?([^"'\] ]+)["']?(?:\s+i)?)?\])?$/);
    if (!parsed) return false;
    if (parsed[1] && parsed[1].toUpperCase() !== this.tagName) return false;
    return !parsed[2] || (this.hasAttribute(parsed[2]) && (parsed[3] === undefined ||
      this.getAttribute(parsed[2]).toLowerCase() === parsed[3].toLowerCase()));
  }
  querySelectorAll(selector) {
    return this.children.flatMap(child => [...(child.matches(selector) ? [child] : []), ...child.querySelectorAll(selector)]);
  }
  querySelector(selector) { return this.querySelectorAll(selector)[0] ?? null; }
  closest() { return null; }
  append(child) { this.children.push(child); child.parentNode = this; child.parentElement = this; }
  cloneNode(deep) {
    const copy = new Element(this.tagName, this.attrs, this.value);
    copy.ownerDocument = this.ownerDocument;
    if (deep) for (const child of this.children) copy.append(child.cloneNode(true));
    return copy;
  }
}

class ShadowRoot extends Element {
  constructor(host) { super('shadow-root'); this.host = host; }
  getRootNode() { return this; }
  append(child) { super.append(child); child.parentElement = null; }
}

// A document without the live page's custom-element registry. Import copies
// platform state, never calls source cloneNode/constructors, and never redacts.
function inertDocument() {
  const document = { defaultView: null };
  document.importNode = function importNode(node, deep = false) {
    const copy = new Element(node.tagName, node.attrs, node.value);
    copy.ownerDocument = document;
    copy.isConnected = false;
    copy.textContent = node.textContent;
    copy.innerText = node.innerText;
    copy.disabled = node.disabled;
    copy.readOnly = node.readOnly;
    if (deep) for (const child of node.children) copy.append(importNode(child, true));
    return copy;
  };
  return document;
}

export function platform(options = {}) {
  const type = options.type ?? 'password';
  const secret = options.secret ?? 'fixture-password-244';
  const field = new Element('input', { id: 'plainfield', name: 'login', type, value: secret }, secret);
  field.selectionStart = 0; field.selectionEnd = secret.length;
  const html = new Element('html'), body = new Element('body'); html.append(body);
  const ordinary = new Element('input', { id: 'ordinary', type: 'text', value: 'keep-ordinary-244' }, 'keep-ordinary-244');
  let path = '0.0.0';
  if (options.shadow) {
    const host = new Element('section'); body.append(host); host.shadowRoot = new ShadowRoot(host);
    host.shadowRoot.append(field); host.shadowRoot.activeElement = field; path += '.s.0';
  } else body.append(field);
  body.append(ordinary);
  const document = {
    implementation: { createHTMLDocument: inertDocument },
    documentElement: html, body, activeElement: options.shadow ? body.children[0] : field,
    title: 'Privacy fixture', readyState: 'complete', defaultView: { getComputedStyle: () => style },
    elementFromPoint: () => field, getSelection: () => null,
    querySelector: selector => html.querySelector(selector),
    querySelectorAll: selector => html.querySelectorAll(selector),
    getElementById: id => html.querySelector(`#${id}`)
  };
  for (const element of [html, body, field, ordinary, ...body.children]) element.ownerDocument = document;
  if (options.runtime) field.setAttribute('type', 'password');
  const window = { scrollX: 0, scrollY: 0, innerWidth: 800, innerHeight: 600, devicePixelRatio: 1,
    getComputedStyle: () => style, HTMLInputElement: Element };
  const context = vm.createContext({ Element, ShadowRoot, HTMLInputElement: Element,
    document, window, location: { href: options.frame ? 'https://fixture.test/frame' : 'https://fixture.test/' },
    history: { length: 1 }, getComputedStyle: () => style,
    Event: class Event { constructor(type) { this.type = type; } },
    requestAnimationFrame: callback => callback(0), setTimeout, clearTimeout, console });
  vm.runInContext(declarations, context);
  return { field, ordinary, html, body, context, path, secret };
}

function rustLiteral(file, anchor) {
  const source = readFileSync(new URL(`../../../crates/${file}`, import.meta.url), 'utf8');
  const start = source.indexOf(anchor);
  assert.ok(start >= 0, `production anchor ${anchor}`);
  const raw = source.indexOf('r#"', start) + 3;
  const end = source.indexOf('"#', raw);
  assert.ok(raw > start && end > raw, `production raw literal ${anchor}`);
  return source.slice(raw, end);
}

export async function readRoute(route, options = {}, fixture = platform(options)) {
  const { context, field, path } = fixture;
  let result;
  if (route === 'inspect') result = await context.runBridgeElementCommandInPage({ operation: 'inspect', elementPath: options.ancestor ? '0.0' : path, maxHtmlBytes: options.max ?? 4096 });
  else if (route === 'duplicate') result = { ok: true, element: context.bridgeElementInspection(field, 4096) };
  else if (route === 'content') result = context.readPageContentInPage(options.max ?? 4096);
  else if (route === 'assert') result = context.runAssertPollInPage({ locator: { engine: 'css', query: '#plainfield' } });
  else if (route === 'aria') result = context.runAriaSnapshotInPage({ rootPath: path });
  else if (route === 'active') result = context.readActiveElementInPage();
  else if (route === 'type') result = context.typeActiveElementInPage(options.text ?? 'replacement-244');
  else if (route === 'raw-inspect' || route === 'raw-assert') {
    const code = route === 'raw-inspect'
      ? rustLiteral('synapse-mcp/src/server/m1_tools.rs', 'const BROWSER_INSPECT_FUNCTION:')
      : rustLiteral('synapse-mcp/src/server/browser_assert.rs', 'const ASSERT_ELEMENT_FUNCTION:');
    const fn = vm.runInContext(`(${code})`, context);
    result = fn(options.ancestor ? fixture.body : field, options.max ?? 4096);
  } else if (route === 'raw-content') {
    const code = rustLiteral('synapse-mcp/src/server/m1_tools.rs', 'async fn browser_content_impl(')
      .replaceAll('{max_bytes}', String(options.max ?? 4096)).replaceAll('{{', '{').replaceAll('}}', '}');
    result = vm.runInContext(code, context);
  } else throw new Error(`unknown route ${route}`);
  return { result: JSON.parse(JSON.stringify(result)), fixture };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const { result } = await readRoute(process.argv[2], JSON.parse(process.argv[3] ?? '{}'));
  process.stdout.write(JSON.stringify(result));
}
