// #89: active claims follow the registry; historical evidence stays untouched.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

const root = new URL('../../../', import.meta.url);
const read = path => readFileSync(new URL(path, root), 'utf8');
// Docs count every built tool, including ones behind an opt-in cargo feature
// (livefeed): drop only the cfg attribute lines, keep the gated items.
const registry = read('crates/synapse-mcp/src/server/tool_profiles.rs')
  .replace(/^[ \t]*#\[cfg\((?:not\()?feature = "[a-z_]+"\)?\)\]\r?\n/gm, '');
const limit = Math.max(...[...registry.matchAll(/PUBLIC_TOOL_LIMIT: usize = (\d+);/g)].map(m => Number(m[1])));
const names = registry.match(/PUBLIC_TOOL_NAMES: &\[&str\] = &\[([\s\S]*?)\];/)[1]
  .match(/"[a-z_]+"/g);
const normalDeclaration = registry.match(/NORMAL_ALLOWED_EXACT: &\[&str\] = (PUBLIC_TOOL_NAMES|&\[([\s\S]*?)\]);/);
assert.notEqual(normalDeclaration, null, 'normal-agent registry authority exists');
const normalNames = normalDeclaration[1] === 'PUBLIC_TOOL_NAMES'
  ? names : normalDeclaration[2].match(/"[a-z_]+"/g);

function claims(text) {
  return [...text.matchAll(/\b(\d+)(?:\*\*)?\s+(?:[A-Za-z-]+\s+){0,3}tools?\b|\b(\d+)[- ]tool contract/gi)]
    .map(match => Number(match[1] ?? match[2]));
}

test('must_pass_active_readme_count_agrees_with_registry', () => {
  assert.equal(names.length, limit, 'precondition: registry names match limit');
  const readme = read('README.md');
  const values = claims(readme);
  assert.ok(values.length >= 4, 'README scan must include badge alt, focused headline, unlock sentence and architecture claim');
  assert.deepEqual(values, values.map(() => limit), 'active README contradicts PUBLIC_TOOL_LIMIT');
  const badges = [...readme.matchAll(/MCP%20tools-(\d+)-/g)].map(match => Number(match[1]));
  assert.deepEqual(badges, [limit], 'README badge must follow same authority');
});

test('must_fail_changing_any_readme_count_claim_is_detected', () => {
  const readme = read('README.md');
  const sites = [...readme.matchAll(/\b\d+(?:\*\*)?\s+(?:[A-Za-z-]+\s+){0,3}tools?\b|\b\d+[- ]tool contract|MCP%20tools-\d+-/gi)];
  assert.ok(sites.length >= 5, 'all four prose claims plus badge URL');
  assert.ok(sites.some(site => /focused MCP tools/.test(site[0])), 'headline must be covered');
  for (const site of sites) {
    const changed = site[0].startsWith('MCP%20tools-')
      ? site[0].replace(/tools-\d+-/, `tools-${limit - 1}-`)
      : site[0].replace(/\d+/, String(limit - 1));
    const mutated = readme.slice(0, site.index) + changed + readme.slice(site.index + site[0].length);
    const counts = [...claims(mutated), ...[...mutated.matchAll(/MCP%20tools-(\d+)-/g)].map(m => Number(m[1]))];
    assert.ok(counts.some(value => value !== limit), `missed README claim: ${site[0]}`);
  }
});

test('must_fail_migration_current_surface_cannot_override_registry_authority', () => {
  const before = read('docs/SYNAPSE_40_TOOL_SURFACE_MIGRATION.md');
  const current = before.split('## Current Surface')[1]?.split('Default normal-agent tools:')[0];
  assert.equal(typeof current, 'string', 'active surface section exists');
  assert.equal(current.includes('PUBLIC_TOOL_NAMES'), true);
  const amendment = before.split('## Current Surface')[0].split('\n')
    .find(line => /supersed|current.*PUBLIC_TOOL_LIMIT|PUBLIC_TOOL_LIMIT.*current/i.test(line));
  if (amendment) {
    assert.equal(amendment.includes('PUBLIC_TOOL_LIMIT'), true);
    assert.equal(amendment.includes(String(limit)), true);
    return; // Explicit amendment makes the old body historical; retain its original counts.
  }
  const cap = current.match(/Public registry limit: (\d+) tools maximum/);
  assert.notEqual(cap, null, 'current section must bind its cap to registry authority');
  assert.equal(Number(cap[1]), limit);
  const visible = current.match(/normal-agent profile exposes (\d+) tools/);
  const publicCount = current.match(/public registry contains (\d+) tools/);
  assert.notEqual(visible, null);
  assert.notEqual(publicCount, null);
  assert.equal(Number(visible[1]), normalNames.length);
  assert.equal(Number(publicCount[1]), names.length);
  assert.equal(read('docs/SYNAPSE_40_TOOL_SURFACE_MIGRATION.md'), before);
});

test('must_fail_stale_active_api_contract_cannot_be_advertised', () => {
  const path = 'docs/systemdocs/16_api_tools_reference.md';
  const before = read(path);
  const currentNote = before.split('\n').find(line => line.includes('Current production note'));
  assert.equal(typeof currentNote, 'string', 'precondition: active note exists');
  assert.deepEqual(claims(currentNote), claims(currentNote).map(() => limit), path);
  assert.equal(read(path), before, 'guard must not rewrite historical/reference text');
});

test('must_fail_guard_rejects_coexisting_wrong_claim_even_with_correct_claim', () => {
  assert.deepEqual(claims(`surface: ${limit} tools; limit: ${limit - 1} tools`), [limit, limit - 1]);
  assert.notDeepEqual(claims(`surface: ${limit} tools; limit: ${limit - 1} tools`), [limit, limit]);
});
