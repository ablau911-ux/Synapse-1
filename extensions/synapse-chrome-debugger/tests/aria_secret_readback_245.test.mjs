import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const source = readFileSync(new URL('../service_worker.js', import.meta.url), 'utf8');
const start = source.indexOf('function runAriaSnapshotInPage(');
const end = source.indexOf('\nfunction runClockInPage(', start);
assert.ok(start >= 0 && end > start);
const code = source.slice(start, end);

// Minimal DOM platform, no privacy or traversal decisions implemented here.
class Element {
  constructor(tag, attrs = {}, value) {
    this.tagName = tag.toUpperCase(); this.attrs = attrs; this.children = [];
    this.id = attrs.id ?? ''; this.textContent = ''; this.parentElement = null;
    if (value !== undefined) this.value = value;
  }
  getAttribute(key) { return this.attrs[key] ?? null; }
  hasAttribute(key) { return key in this.attrs; }
  append(child) { this.children.push(child); child.parentElement = this; child.parentNode = this; }
  getRootNode() { return this.parentNode?.getRootNode?.() ?? this; }
  closest() { return null; }
  querySelector() { return null; }
  querySelectorAll() { return []; }
}
class ShadowRoot {
  constructor(host) { this.host = host; this.children = []; }
  getRootNode() { return this; }
  getElementById(id) { return this.children.find(e => e.id === id) ?? null; }
  querySelector() { return null; }
  querySelectorAll() { return []; }
}
function snapshot(field, setup = () => {}, rootPath) {
  const html = new Element('html'), body = new Element('body');
  html.append(body); body.append(field); setup(body, field);
  const doc = { documentElement: html, body, title: 'Fixture', readyState: 'complete',
    activeElement: field, querySelector: () => null, querySelectorAll: () => [],
    getElementById: id => [field, ...body.children].find(e => e.id === id) ?? null };
  const context = vm.createContext({ Element, ShadowRoot, document: doc, location: { href: 'https://fixture.test/' } });
  vm.runInContext(code, context);
  return context.runAriaSnapshotInPage({ rootPath });
}

test('must_pass_ordinary_text_field_retains_exact_value', () => {
  const field = new Element('input', { 'aria-label': 'Search' }, 'ordinary');
  assert.equal(field.value, 'ordinary');
  const result = snapshot(field);
  assert.equal(result.ok, true);
  const row = result.nodes.find(n => n.role === 'textbox');
  assert.equal(row.name, 'Search'); assert.equal(row.value, 'ordinary');
  assert.equal(field.value, 'ordinary');
});

test('must_fail_password_aria_secret_and_sensitive_ancestor_never_leak_values', () => {
  for (const attrs of [{ type: 'password' }, { 'aria-label': 'API secret' }, { autocomplete: 'one-time-code' }]) {
    const field = new Element('input', attrs, 'SECRET_245');
    const result = snapshot(field);
    assert.equal(result.ok, true);
    assert.equal(JSON.stringify(result).includes('SECRET_245'), false, JSON.stringify(attrs));
    assert.equal(field.value, 'SECRET_245');
  }
  const field = new Element('input', {}, 'ANCESTOR_SECRET_245');
  const result = snapshot(field, body => { body.attrs['aria-label'] = 'Password'; });
  assert.equal(JSON.stringify(result).includes('ANCESTOR_SECRET_245'), false);
  assert.equal(field.value, 'ANCESTOR_SECRET_245');
});

test('must_fail_shadow_labelled_secret_is_redacted_when_shadow_root_is_requested', () => {
  const host = new Element('div');
  const shadow = new ShadowRoot(host); host.shadowRoot = shadow;
  const label = new Element('span', { id: 'secret-label' }); label.textContent = 'API secret';
  const field = new Element('input', { 'aria-labelledby': 'secret-label' }, 'SHADOW_SECRET_245');
  shadow.children.push(label, field); label.parentNode = shadow; field.parentNode = shadow;
  assert.equal(field.getRootNode(), shadow);
  const result = snapshot(host, () => {}, '0.0.0.s.1');
  assert.equal(result.ok, true);
  assert.equal(result.nodes.length, 1, 'requested shadow field must actually be observed');
  assert.equal(JSON.stringify(result).includes('SHADOW_SECRET_245'), false);
  assert.equal(field.value, 'SHADOW_SECRET_245');
});
