import fs from 'node:fs';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

const [probe, fixture, out] = process.argv.slice(2);
const clip = JSON.parse(fs.readFileSync(path.join(fixture, 'clip.json')));
fs.mkdirSync(out, { recursive: true });
clip.state = 'recording';
clip.recorded_ms = 0;
clip.lost = [];
clip.exact = null;
clip.moving.segments = [];
fs.writeFileSync(path.join(out, 'clip.json'), JSON.stringify(clip));
const run = spawnSync(probe, ['exp-dyn', 'mode=recover', `folder=${out}`, `out=${out}`], { encoding: 'utf8' });
const recovered = JSON.parse(fs.readFileSync(path.join(out, 'clip.json')));
const planned = clip.end_ms * clip.fps / 1000;
const pass = run.status === 0 && recovered.state === 'ready'
  && recovered.stop_reason === 'service_restart' && recovered.lost.length === planned;
console.log(`recover before first frame | ready and every planned instant lost | ${planned} | state=${recovered.state}, lost=${recovered.lost.length}, exit=${run.status} ${run.stderr.trim()} | ${pass ? 'PASS' : 'FAIL'}`);
process.exitCode = pass ? 0 : 1;
