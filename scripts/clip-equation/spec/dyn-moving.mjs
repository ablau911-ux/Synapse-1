import fs from 'node:fs';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { runFeature } from './feature.mjs';

const BASE = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const options = Object.fromEntries(process.argv.slice(3).map(arg => {
  const split = arg.indexOf('=');
  return [arg.slice(0, split), arg.slice(split + 1)];
}));
const ROOT = options.root || JSON.parse(fs.readFileSync(path.join(BASE, 'corpus/corpus.json'), 'utf8')).root;
const OUT = path.resolve(options.out || path.join(BASE, '../../.runs/dyn-moving'));
const PROBE = path.resolve(options.probe || path.join(BASE, 'probe/target/release/clip-probe.exe'));
const QUALITY_MIN = 0.95;
const cases = [
  ['full', '1920x1080', '0,0,1920,1080'],
  ['crop', '1920x1080', '400,304,640,368'],
  ['minimum', '1920x1080', '400,304,64,64'],
  ['pause-long', '1920x1080', '0,0,1920,1080', 'pause=100-140'],
  ['pause-short', '1920x1080', '0,0,1920,1080', 'pause=200-203'],
  ['odd', '506x333', '0,0,506,333'],
  ['unavailable', '1920x1080', '0,0,1920,1080', 'force=unavailable'],
];
const rows = [], errors = [];
fs.mkdirSync(OUT, { recursive: true });
for (const [name, size, rect, extra] of cases) {
  const run = spawnSync(PROBE, ['exp-dyn', 'mode=moving', `folder=${path.join(ROOT, `video-${size}`)}`,
    `out=${path.join(OUT, name)}`, `rect=${rect}`, ...(extra ? [extra] : [])], { encoding: 'utf8' });
  try {
    if (run.error || run.status !== 0) throw new Error(run.error?.message || run.stderr.trim() || `exit ${run.status}`);
    rows.push({ name, ...JSON.parse(run.stdout.trim().split(/\r?\n/).at(-1)) });
  } catch (error) { errors.push(`${name}: ${error.message}`); }
}
const normal = rows.filter(r => r.name !== 'unavailable');
const report = (pass, measured) => ({ pass: !errors.length && rows.length === cases.length && pass,
  value: '-', measured: errors.length ? errors.join('; ') : measured });
const all = fn => normal.length === cases.length - 1 && normal.every(fn);
const table = {
  'hardware quality grant': () => report(all(r => r.grant?.hardware && r.grant.quality_level === 90 && r.grant.b_frames === 0 && r.grant.gop_frames === r.constants.GOP_FRAMES), JSON.stringify(normal.map(r => r.grant))),
  'one sample per instant': () => report(all(r => r.segments.reduce((n, s) => n + s.frames, 0) === r.instants), JSON.stringify(normal.map(r => [r.name, r.instants, r.segments]))),
  'mapping including repeated instants': () => report(all(r => r.scores.length === r.instants && r.scores.every(s => s.current >= s.previous && s.current >= s.next)), JSON.stringify(normal.map(r => [r.name, r.scores?.filter(s => s.current < s.previous || s.current < s.next).length]))),
  'box quality': () => report(all(r => r.scores.reduce((sum, s) => sum + s.current, 0) / r.scores.length >= QUALITY_MIN), JSON.stringify(normal.map(r => [r.name, r.scores?.reduce((sum, s) => sum + s.current, 0) / r.scores?.length]))),
  'odd dimensions': () => report(rows.find(r => r.name === 'odd')?.segments.every(s => s.coded_width === 506 && s.coded_height === 334 && s.rect[2] === 506 && s.rect[3] === 333), JSON.stringify(rows.find(r => r.name === 'odd')?.segments)),
  'keyframe spacing': () => report(all(r => r.segments.every(s => s.key_frames[0]?.[0] === 0 && s.key_frames.every((k, i) => (s.key_frames[i + 1]?.[0] ?? s.frames) - k[0] <= r.constants.GOP_FRAMES))), 'verified sample table'),
  'bounded queue': () => report(all(r => r.high_water <= r.constants.MOVING_QUEUE_FRAMES && r.behind === 0), JSON.stringify(normal.map(r => [r.name, r.high_water, r.behind, r.creation_ms]))),
  'unavailable stays exact': () => { const r = rows.find(r => r.name === 'unavailable'); return report(r?.segments.length === 0 && r.files === 0 && Boolean(r.unavailable), JSON.stringify(r)); },
};
fs.writeFileSync(path.join(OUT, 'checks.json'), `${JSON.stringify({ rows, errors }, null, 2)}\n`);
process.exitCode = runFeature('dyn-moving', path.join(BASE, 'spec/dyn-moving.feature'), table);
