// Three sequential repeats. Every child gets its own fixture/output directory.
import fs from 'node:fs';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

const REPEATS = 3;
const [group = 'core'] = process.argv.slice(2);
const root = process.cwd(), results = path.join(root, 'scripts/clip-equation/results');
const exe = path.join(root, 'target/release/synapse-mcp.exe');
const probe = path.join(root, 'scripts/clip-equation/probe/target/release/clip-probe.exe');
const summary = [];
const run = (label, args, env = {}) => {
  const output = path.join(results, `${label}.txt`);
  const fd = fs.openSync(output, 'w');
  const started = Date.now();
  const child = spawnSync('node', args, { cwd: root, stdio: ['ignore', fd, fd], env: { ...process.env, ...env } });
  fs.closeSync(fd);
  const row = { label, exit: child.status, elapsed_ms: Date.now()-started, error: child.error?.message };
  summary.push(row);
  fs.writeFileSync(path.join(results, `final-${group}-summary.json`), JSON.stringify(summary, null, 2));
  console.log(JSON.stringify(row));
};
if (group === 'core') {
  for (const check of ['dyn-exact', 'dyn-moving', 'dyn-motion']) for (let repeat=1; repeat<=REPEATS; repeat++)
    run(`final-${check}-${repeat}`, ['scripts/clip-equation/spec/run-spec.mjs', check, `out=${path.join(root, '.runs', `final-${check}-${repeat}`)}`]);
} else if (group === 'viewer') {
  for (let repeat=1; repeat<=REPEATS; repeat++) {
    const fixture = path.join(root, '.runs', `final-dyn-motion-${repeat}`);
    run(`final-dyn-seek-${repeat}`, ['scripts/clip-equation/spec/dyn-seek.mjs', probe, path.join(fixture, 'noise')]);
    run(`final-dyn-seek-video-${repeat}`, ['scripts/clip-equation/spec/dyn-seek.mjs', probe, path.join(fixture, 'video')]);
    run(`final-dyn-viewer-${repeat}`, ['scripts/clip-equation/spec/dyn-viewer.mjs', exe, path.join(fixture, 'noise'), path.join(fixture, 'sources/noise'), path.join(root, '.runs', `final-viewer-${repeat}`)]);
  }
} else if (group === 'live') {
  for (const [name,scene,duration,kill] of [['ui','ui',20000,0],['video','video',20000,0],['long','video',70000,0],['recovery','video',20000,8000]])
    for (let repeat=1; repeat<=REPEATS; repeat++) {
      const label=`final-dyn-${name}-${repeat}`, out=path.join(root,'.runs',label);
      run(label,['scripts/clip-equation/spec/dyn-live.mjs',exe,out,scene,String(duration),String(kill)]);
    }
} else if (group === 'd5') {
  for (let repeat=1; repeat<=REPEATS; repeat++) {
    const out=path.join(root,'.runs',`final-d5-${repeat}`);
    run(`final-d5-${repeat}`,['scripts/clip-equation/live-test/d5-proof.mjs',exe,out],{LOCALAPPDATA:path.join(out,'local')});
  }
} else throw Error(`unknown check group ${group}`);
process.exitCode=summary.some(r=>r.exit!==0)?1:0;
