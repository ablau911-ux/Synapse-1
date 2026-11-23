// Decision test at 1920x1080 (owner, 2026-11-23, after the three consultations):
// zstd whole frame (baseline), zstd stream at window 2^24..2^26 x level -3/1, zstd prefix + long-distance
// matching L1/L3, zstd prefix L3 (the 1 MiB match-window cliff), and H.264 Quick Sync q70.
// 5 scenes (typing, ui, scroll, flash, video) x 3 repeats on the main corpus; 2 probes at a time,
// BelowNormal. Output: results/test-1080.csv (one row per run) and a median summary on stdout.
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const BASE = path.join(path.dirname(fileURLToPath(import.meta.url)), '..');
const PROBE = path.join(BASE, 'probe', 'target', 'release', 'clip-probe.exe');
const CORPUS = JSON.parse(fs.readFileSync(path.join(BASE, 'corpus', 'corpus.json'), 'utf8'));
const OUT = path.join(BASE, 'results', 'test-1080.csv');
const SCENES = ['typing', 'ui', 'scroll', 'flash', 'video'];
const REPS = 3;
const T_S = CORPUS.t_s;
const base = { layout: 'bgr', tile: 32, gap: 0, dict: 0, workers: 0, theta: 25, drain: 0 };
const Z = [
  { name: 'whole L-3', way: 'ZW1', level: -3, mode: 'whole', wlog: 0, ldm: 0, kz: 20 },
  ...[24, 25, 26].flatMap((wlog) => [-3, 1].map((level) => ({ name: `stream L${level} W${wlog}`, way: 'ZW13', level, mode: 'stream', wlog, ldm: 0, kz: 20 }))),
  ...[1, 3].map((level) => ({ name: `prefix+ldm L${level}`, way: 'ZW15', level, mode: 'prefix', wlog: 24, ldm: 1, kz: 20 })),
  { name: 'prefix L3 (cliff)', way: 'ZW12', level: 3, mode: 'prefix', wlog: 20, ldm: 0, kz: 'all' },
];
const rate = /rate=\d+/.exec(spawnSync(PROBE, ['exp-rate'], { encoding: 'utf8', windowsHide: true }).stdout)[0];
const qsv = JSON.parse(fs.readFileSync(path.join(BASE, 'results', 'encoders.json'), 'utf8')).find((e) => e.codec === 'H.264' && e.vendor_id === 'VEN_8086');
const f = CORPUS.flash;
const flash = `flash=${f.bgr.join(':')}:${f.box_left_fraction}:${f.box_top}:${f.box_width}:${f.box_height}:${f.min_pixels}`;
const regions = (sc) => (CORPUS.text_regions?.[sc] ?? []).map((g) => [g.left, g.top, g.right, g.bottom, g.max_width ?? 0, g.max_height ?? 0, g.max_width_fraction ?? 0].join(':')).join(';');

function probe(args) {
  return new Promise((resolve) => {
    const child = spawn(PROBE, args, { windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] });
    let out = '';
    let err = '';
    child.stdout.on('data', (d) => { out += d; });
    child.stderr.on('data', (d) => { err += d; });
    child.on('close', (code) => {
      const line = out.split(/\r?\n/).find((l) => l.startsWith('{'));
      try { resolve(line ? JSON.parse(line) : { error: `exit ${code}: ${err.trim().slice(0, 160)}` }); } catch { resolve({ error: `bad JSON, exit ${code}` }); }
    });
  });
}

async function runZ(job) {
  const scratch = path.join(os.tmpdir(), 'clip-1080', String(job.id));
  const c = { ...base, ...job.cfg };
  const args = ['exp-z', `folder=${path.join(CORPUS.root, job.folder)}`, `out=${scratch}`, rate, `rep=${job.rep}`];
  for (const k of ['way', 'level', 'layout', 'mode', 'tile', 'gap', 'wlog', 'ldm', 'dict', 'workers', 'kz', 'theta', 'drain']) args.push(`${k}=${c[k]}`);
  const j = await probe(args);
  fs.rmSync(scratch, { recursive: true, force: true });
  return { status: j.error ? `error ${j.error}` : 'ok', bytes: j.bytes_disk, cpu_s: j.way_cpu_ns / 1e9, ram_mb: j.ram_os / 1048576,
    exact: j.error ? '' : Number(j.diff_sum) === 0 ? 'yes' : 'NO', seek_frames: j.n_dec_worst, ssim: 1, psnr: 'inf', r: 1 };
}

async function runD(job) {
  const scratch = path.join(os.tmpdir(), 'clip-1080', String(job.id));
  const common = [`folder=${path.join(CORPUS.root, job.folder)}`, 'encoder=DW1', `name=${qsv.name.replace(/[^\x20-\x7E]/g, '')}`, `vendor=${qsv.vendor_id}`, 'hw=1',
    'q=70', 'gop=20', 'hint=1', 'ratemode=quality', 'beta=0.1', `scratch=${scratch}`, `repeat=${job.rep}`, rate, 'window_width=1920', flash, `regions=${regions(job.scene)}`];
  const live = await probe(['exp-d', 'phase=live', ...common]);
  let res = live;
  if (live.status === 'ok') {
    const worker = await probe(['exp-d', 'phase=worker', ...common]);
    res = { ...live, ...worker, cpu: Number(live.a_rt_ns) + Number(worker.a_w_ns), ram: Number(worker.live_ram_os ?? live.live_ram_os) + Number(worker.worker_ram_os) };
  }
  fs.rmSync(scratch, { recursive: true, force: true });
  return { status: res.status ?? `error ${res.error}`, bytes: res.bytes_disk ?? res.bytes, cpu_s: res.cpu / 1e9, ram_mb: res.ram / 1048576,
    exact: 'no (lossy)', seek_frames: '', ssim: res.ssim_mean, psnr: res.psnr_mean, r: res.r ?? 'n/a' };
}

const jobs = [];
let id = 0;
for (const scene of SCENES) {
  const folder = `${scene}-1920x1080`;
  for (let rep = 1; rep <= REPS; rep += 1) {
    for (const cfg of Z) jobs.push({ id: id++, kind: 'Z', scene, folder, rep, cfg, name: cfg.name });
    jobs.push({ id: id++, kind: 'D', scene, folder, rep, name: 'H.264 Quick Sync q70' });
  }
}
const cols = ['name', 'scene', 'rep', 'status', 'mib_per_min', 'cpu_s', 'ram_mb', 'exact', 'seek_frames', 'ssim', 'psnr', 'r'];
fs.writeFileSync(OUT, `${cols.join(',')}\n`);
const rows = [];
const todo = [...jobs];
let done = 0;
const started = Date.now();
async function worker() {
  while (todo.length) {
    const job = todo.shift();
    const r = await (job.kind === 'Z' ? runZ(job) : runD(job));
    const row = { name: job.name, scene: job.scene, rep: job.rep, ...r, mib_per_min: (Number(r.bytes) * 60) / T_S / 1048576 };
    rows.push(row);
    fs.appendFileSync(OUT, `${cols.map((c) => String(row[c] ?? '').replace(/,/g, ' ')).join(',')}\n`);
    done += 1;
    if (done % 15 === 0) console.log(`[${done}/${jobs.length}] ${((Date.now() - started) / 60000).toFixed(1)} min`);
  }
}
await Promise.all([worker(), worker()]);

const med = (a) => { const v = a.filter(Number.isFinite).sort((x, y) => x - y); return v.length ? v[Math.floor(v.length / 2)] : NaN; };
console.log('\nmedian per way and scene (MiB/min | CPU s per 20 s | RAM MiB | exact):');
for (const name of [...new Set(rows.map((r) => r.name))]) {
  const cells = SCENES.map((scene) => {
    const rs = rows.filter((r) => r.name === name && r.scene === scene && r.status === 'ok');
    if (!rs.length) return `${scene}: fail`;
    return `${scene} ${med(rs.map((r) => r.mib_per_min)).toFixed(2)} | ${med(rs.map((r) => r.cpu_s)).toFixed(2)} | ${med(rs.map((r) => r.ram_mb)).toFixed(1)} | ${rs[0].exact}`;
  });
  console.log(`${name}: ${cells.join('  ;  ')}`);
}
console.log(`done in ${((Date.now() - started) / 60000).toFixed(1)} min`);
