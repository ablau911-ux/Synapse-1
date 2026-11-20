import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

const url = new URL('../../../docs/SYNAPSE_40_TOOL_SURFACE_MIGRATION.md', import.meta.url);
const read = () => readFileSync(url, 'utf8');
const section = () => read().split('## `reality operation=delta`:')[1];

test('must_pass_delta_docs_name_missing_baseline_and_recovery_contract', () => {
  const text = section();
  assert.equal(typeof text, 'string');
  const registry = readFileSync(new URL('../../../crates/synapse-mcp/src/server/tool_profiles.rs', import.meta.url), 'utf8');
  assert.equal(registry.includes('"reality"'), true, 'documented facade is public');
  const runtime = readFileSync(new URL('../../../crates/synapse-mcp/src/server/reality.rs', import.meta.url), 'utf8');
  for (const token of ['profile_changed:', 'stale_epoch:', 'delta_overflow:', 'delta_snapshot_budget_exceeded:']) {
    assert.equal(runtime.includes(token), true, `runtime reason exists: ${token}`);
    assert.equal(text.includes(token.slice(0, -1)), true);
  }
  for (const field of ['missing_baseline', 'baseline_required=true', 'empty `deltas`', 'no cursor', 'reality operation=baseline']) {
    assert.equal(text.includes(field), true, `missing documented field/value: ${field}`);
  }
});

test('must_fail_undecodable_head_cannot_be_documented_as_exact_token_only', () => {
  const before = read();
  const text = section();
  assert.equal(text.includes('missing_baseline: stored head row is undecodable; call reality baseline to rebuild it'), true);
  assert.equal(text.includes('reason.starts_with(token + ": ")'), true);
  for (const reason of ['profile_changed', 'stale_epoch', 'delta_overflow', 'delta_snapshot_budget_exceeded']) {
    assert.equal(text.includes(reason), true, `preserve neighbouring reason ${reason}`);
  }
  assert.equal(read(), before, 'contract guard is read-only');
});
