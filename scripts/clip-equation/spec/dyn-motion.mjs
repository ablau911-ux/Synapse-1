import fs from 'node:fs';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { SCENES, renderScenes } from './dyn-scenes.mjs';

const BASE = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const options = Object.fromEntries(process.argv.slice(3).map(a => { const i = a.indexOf('='); return [a.slice(0, i), a.slice(i + 1)]; }));
const root = options.root || JSON.parse(fs.readFileSync(path.join(BASE, 'corpus/corpus.json'))).root;
const out = path.resolve(options.out || path.join(BASE, '../../.runs/dyn-motion'));
const probe = options.probe || path.join(BASE, 'probe/target/release/clip-probe.exe');
const scenes = path.join(out, 'sources');
fs.mkdirSync(out, { recursive: true });
// Check the product mode before creating the larger synthetic fixtures.
const supported = spawnSync(probe, ['exp-dyn', 'mode=layers', `folder=${path.join(root, 'still-1920x1080')}`, `out=${path.join(out, 'still')}`], { encoding: 'utf8' });
if (supported.status !== 0) {
  console.error(`FAIL layers mode: ${supported.stderr || supported.error}`);
  process.exit(1);
}
renderScenes(scenes, path.join(root, 'still-1920x1080/frame-0000.zst'));
let failed = 0;
const rows = [];
function check(name, ok, measured) { console.log(`${ok ? 'PASS' : 'FAIL'} ${name}: ${JSON.stringify(measured)}`); if (!ok) failed++; }
for (const scene of [...SCENES, ...['video', 'typing', 'ui', 'scroll', 'flash'].map(name => ({ name, corpus: true }))]) {
  const folder = scene.corpus ? path.join(root, `${scene.name}-1920x1080`) : path.join(scenes, scene.name);
  const run = spawnSync(probe, ['exp-dyn', 'mode=layers', 'clipjson=1', `folder=${folder}`, `out=${path.join(out, scene.name)}`], { encoding: 'utf8' });
  if (run.status !== 0) { check(scene.name, false, run.stderr || run.error?.message); continue; }
  const r = JSON.parse(run.stdout.trim().split(/\r?\n/).at(-1)); rows.push({ name: scene.name, ...r });
  const moving = r.lines.filter(l => l.mv);
  check(`${scene.name} exact outside`, r.outside_mismatches === 0, r.outside_mismatches);
  if (['small', 'bar', 'flat', 'slow', 'typing', 'ui', 'scroll', 'flash'].includes(scene.name)) check(`${scene.name} stays exact`, moving.length === 0, moving.length);
  if (scene.name === 'noise') {
    check('noise interval', moving[0]?.instant === 44 && moving.at(-1)?.instant === 134 && r.segments.length === 1, [moving[0]?.instant, moving.at(-1)?.instant, r.segments.length]);
    check('noise rectangle', JSON.stringify(r.segments[0]?.rect) === '[400,304,320,240]', r.segments[0]?.rect);
    check('frozen exact rows', r.lines.filter(l => l.instant >= 44 && l.instant <= 134).every(l => !l.x), r.lines.filter(l => l.instant >= 44 && l.instant <= 134 && l.x));
    check('stop restored exact', Boolean(r.lines[135]?.x), r.lines[135]);
    check('first four exact', r.lines.slice(40, 44).every(l => l.x) && r.lines.slice(40, 44).reduce((n,l) => n + (l.x?.[3] || 0), 0) <= 4 * (240 * 1920 * 3 + 8), r.lines.slice(40,44));
  }
  if (scene.name === 'two') check('largest group', r.segments.length > 0 && r.segments.every(s => JSON.stringify(s.rect) === '[400,304,320,240]'), r.segments.map(s => s.rect));
  if (scene.name === 'odd') check('outward alignment', r.segments.length > 0 && r.segments.every(s => JSON.stringify(s.rect) === '[400,288,336,256]'), r.segments.map(s => s.rect));
  if (scene.name === 'corner') check('clamped corner', r.segments.length > 0 && r.segments.every(s => s.rect[0]+s.rect[2] === 1001 && s.rect[1]+s.rect[3] === 701 && s.coded_width % 2 === 0 && s.coded_height % 2 === 0), r.segments);
  if (['noise','video'].includes(scene.name)) check(`${scene.name} moving quality`, r.mean_ssim >= 0.95, r.mean_ssim);
  if (scene.name === 'video') check('video admits whole picture', moving[0]?.instant <= 5 && r.segments.some(s => s.rect[2]*s.rect[3] >= 0.99*1920*1080), r.segments);
}
fs.writeFileSync(path.join(out, 'checks.json'), JSON.stringify(rows, null, 2));
console.log(`dyn-motion: ${failed} failed`);
process.exitCode = failed ? 1 : 0;
