// Experiments steps 2 and 3, way Z: the run list of docs/clip-recorder-experiments-plan.md
// ("Run design": Grid A-Z and Grid B) over every zstd way of docs/clip-recorder-ways.md,
// one `clip-probe exp-z` subprocess per run at BelowNormal priority, appended to a CSV.
//
//   node run-z.mjs --plan                      count the full run list
//   node run-z.mjs --full [--limit N]          run it into results/z-runs.csv (resumable)
//   node run-z.mjs --pilot [folder]            the pilot into results/z-pilot.csv (resumable)
//   node run-z.mjs --estimate                  T (equation 6) of the full run from the pilot
//
// Repeats: bytes are deterministic, so they come from run 1; CPU and RAM from runs 1 to 3.
// A row already in the CSV (same options and repeat) is skipped.
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { CorpusFolder, rowChecks } from '../spec/step2z.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const BASE = path.resolve(HERE, '..');
const PROBE = path.join(BASE, 'probe', 'target', 'release', 'clip-probe.exe');
const CORPUS = 'C:\\Users\\Adam\\clip-corpus';
const SCRATCH = path.join(os.tmpdir(), 'clip-exp-z');
const RESULTS = path.join(BASE, 'results');
const REPEATS = 3;

// Plan factors (section "Factors").
const LEVELS = [-1, -2, -3, -4, -5, -6, -7, -10, -20, -50, -100, ...Array.from({ length: 22 }, (_, i) => i + 1)];
const SCENARIOS = ['typing', 'flash', 'scroll', 'video', 'still', 'ui'];
const SIZES = ['1920x1080', '1280x720', '640x360', '506x333'];
const GRID_A_MODES = [
  { way: 'ZW1' },
  { way: 'ZW7', kz: 20 },
  { way: 'ZW8', tile: 32, kz: 20 },
  { way: 'ZW8', tile: 64, kz: 20 },
  { way: 'ZW8', tile: 128, kz: 20 },
  { way: 'ZW9', kz: 20 },
];
// Grid B: one factor at a time around the baseline (L = -3, whole frame), and every other runnable way.
const GRID_B = [
  { way: 'ZW0' },
  { way: 'ZW1', drain: 16384 },
  { way: 'ZW2' },
  { way: 'ZW3' },
  { way: 'ZW4', layout: 'sg' },
  { way: 'ZW4', layout: 'sgp' },
  { way: 'ZW5' },
  { way: 'ZW6', layout: 'predl' },
  { way: 'ZW6', layout: 'predu' },
  ...[1, 100, 'all'].map((kz) => ({ way: 'ZW7', kz })),
  { way: 'ZW7', mode: 'sub', kz: 20 },
  ...[1, 20, 100, 'all'].map((kz) => ({ way: 'ZW12', kz })),
  ...[23, 24, 25, 27].map((wlog) => ({ way: 'ZW13', wlog, kz: 20 })),
  ...[1, 100, 'all'].map((kz) => ({ way: 'ZW13', kz })),
  { way: 'ZW14', wlog: 24, kz: 20 },
  { way: 'ZW15', kz: 20 },
  { way: 'ZW16', dict: 110 },
  { way: 'ZW16', dict: 1024 },
  ...[1, 2].map((workers) => ({ way: 'ZW17', workers })),
  ...[10, 25, 50].map((theta) => ({ way: 'ZW18', theta, tile: 32, kz: 20 })),
  // ZW19: a layout with a temporal mode; the best pair is chosen after the single ways.
  { way: 'ZW19', layout: 'ycocg', mode: 'xor', kz: 20 },
  { way: 'ZW19', layout: 'ycocg', mode: 'tiles', tile: 32, kz: 20 },
];
export const NOT_RUN = {
  ZW10: 'Windows dirty rectangles: the headless corpus has no dirty regions; needs a live capture',
  ZW11: 'GPU change mask: needs a new compute shader in the probe',
  ZW20: 'nvCOMP on the NVIDIA GPU: CUDA only, the capture adapter is the Intel GPU, no crate',
};

const OPTION_COLUMNS = ['folder', 'way', 'level', 'layout', 'mode', 'tile', 'wlog', 'ldm', 'dict', 'workers', 'kz', 'theta', 'drain', 'frames_cap', 'rep'];
const MEASURED_COLUMNS = [
  'w', 'h', 'px', 'n_new', 'd_wait', 'frames', 'bytes_out', 'bytes_disk', 'bytes_per_frame', 'files', 'dict_bytes',
  'comp_cpu_ns', 'comp_gpt_ns', 'comp_cycles', 'comp_wall_ns', 'comp_cpu_per_frame_ns', 'comp_wall_total_ns',
  'write_cpu_ns', 'write_wall_ns', 'corpus_cpu_ns', 'corpus_wall_ns', 'train_cpu_ns', 'train_wall_ns',
  'm0_private', 'm0_ws', 'peak_private', 'peak_ws', 'os_peak_private', 'os_peak_ws', 'm_cctx_est', 'cctx_sizeof', 'bc', 'm_dctx_est', 'bd',
  'dec_cpu_ns', 'dec_gpt_ns', 'dec_wall_ns', 'dec_cpu_per_frame_ns', 'dec_peak_private', 'dec_peak_ws', 'read_cpu_ns', 'corpus_dec_cpu_ns',
  'frames_decoded', 'diff_sum', 'diff_frames', 'n_dec_worst', 't_ready_ns', 'run_wall_ms',
];
const KNOWN_COLUMNS = ['known_n_new', 'known_d_wait', 'known_m_comp', 'known_m_dec'];
const PASS_COLUMNS = ['pass_frames', 'pass_bytes', 'pass_baseline', 'pass_ram', 'pass_roundTrip', 'pass_seek', 'pass_ramDec', 'pass_ready', 'error'];
const COLUMNS = [...OPTION_COLUMNS, ...MEASURED_COLUMNS, ...KNOWN_COLUMNS, ...PASS_COLUMNS];

const folders = () => SCENARIOS.flatMap((s) => SIZES.map((z) => `${s}-${z}`));
const optionKey = (r) => OPTION_COLUMNS.map((c) => String(r[c] ?? '')).join('|');

/** The full run list: Grid A-Z, then Grid B, for every folder; each configuration × 3 repeats. */
export function fullPlan() {
  const configs = [];
  for (const folder of folders()) {
    for (const level of LEVELS) for (const m of GRID_A_MODES) configs.push({ folder, level, ...m });
    for (const b of GRID_B) configs.push({ folder, level: -3, ...b });
  }
  return configs.flatMap((c) => Array.from({ length: REPEATS }, (_, i) => ({ ...c, rep: i + 1 })));
}

/** Frames per pilot run: all at fast levels; fewer at slow levels, whose v is per byte. */
function pilotCap(level) {
  if (level <= 6) return '';
  if (level <= 12) return 100;
  if (level <= 18) return 20;
  return 5;
}

/** ZW1 at every level, the baseline 3 times, and one run of each other way at L = -3. */
export function pilotPlan(folder) {
  const runs = LEVELS.map((level) => ({ folder, way: 'ZW1', level, frames_cap: pilotCap(level), rep: 1 }));
  runs.push({ folder, way: 'ZW1', level: -3, frames_cap: '', rep: 2 }, { folder, way: 'ZW1', level: -3, frames_cap: '', rep: 3 });
  for (const m of [...GRID_A_MODES.slice(1), ...GRID_B]) {
    runs.push({ folder, level: -3, frames_cap: m.way === 'ZW16' ? 40 : '', ...m, rep: 1 });
  }
  return runs;
}

class Csv {
  constructor(file) {
    this.file = file;
    this.done = new Set();
    if (fs.existsSync(file)) {
      const [head, ...lines] = fs.readFileSync(file, 'utf8').split(/\r?\n/).filter((l) => l.trim());
      const cols = head.split(',');
      for (const line of lines) {
        const values = line.split(',');
        this.done.add(optionKey(Object.fromEntries(cols.map((c, i) => [c, values[i]]))));
      }
    } else {
      fs.mkdirSync(path.dirname(file), { recursive: true });
      fs.writeFileSync(file, `${COLUMNS.join(',')}\n`);
    }
  }

  has(run) {
    return this.done.has(optionKey(run));
  }

  append(row) {
    fs.appendFileSync(this.file, `${COLUMNS.map((c) => String(row[c] ?? '').replace(/[,\n]/g, ';')).join(',')}\n`);
    this.done.add(optionKey(row));
  }
}

function probeArgs(run, rate) {
  const args = ['exp-z', `folder=${path.join(CORPUS, run.folder)}`, `out=${SCRATCH}`, `rate=${rate}`];
  for (const key of ['way', 'level', 'layout', 'mode', 'tile', 'wlog', 'ldm', 'dict', 'workers', 'kz', 'theta', 'drain', 'rep']) {
    if (run[key] !== undefined && run[key] !== '') args.push(`${key}=${run[key]}`);
  }
  if (run.frames_cap) args.push(`frames=${run.frames_cap}`);
  return args;
}

/** One subprocess at BelowNormal priority; resolves with its JSON line. */
function runProbe(args) {
  return new Promise((resolve) => {
    const started = Date.now();
    const child = spawn(PROBE, args, { windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] });
    try {
      os.setPriority(child.pid, os.constants.priority.PRIORITY_BELOW_NORMAL);
    } catch { /* the run still counts; priority is best effort */ }
    let out = '';
    let err = '';
    child.stdout.on('data', (d) => { out += d; });
    child.stderr.on('data', (d) => { err += d; });
    child.on('close', (code) => {
      const line = out.trim().split('\n').filter((l) => l.startsWith('{')).pop();
      let json = null;
      try { json = line ? JSON.parse(line) : null; } catch { json = null; }
      resolve({ json, error: json ? '' : `exit ${code}: ${err.trim().slice(0, 200)}`, wallMs: Date.now() - started });
    });
  });
}

function cycleRate() {
  const text = execFileSync(PROBE, ['exp-rate'], { windowsHide: true }).toString();
  return Number(/rate=(\d+)/.exec(text)?.[1] ?? 0);
}

async function runAll(runs, csvFile, limit = Infinity) {
  const csv = new Csv(csvFile);
  const todo = runs.filter((r) => !csv.has(r)).slice(0, limit);
  console.log(`run-z: ${runs.length} runs, ${runs.length - todo.length} already in ${path.basename(csvFile)}, ${todo.length} to run`);
  if (todo.length === 0) return;
  fs.mkdirSync(SCRATCH, { recursive: true });
  const rate = cycleRate();
  console.log(`run-z: cycle rate ${rate} per second`);
  for (const [i, run] of todo.entries()) {
    const { json, error, wallMs } = await runProbe(probeArgs(run, rate));
    const row = { ...run, ...(json ?? {}), folder: run.folder, way: run.way, level: run.level, rep: run.rep, run_wall_ms: wallMs, error };
    for (const key of ['layout', 'mode', 'tile', 'wlog', 'ldm', 'dict', 'workers', 'kz', 'theta', 'drain']) row[key] = json?.[key] ?? run[key] ?? '';
    if (json) {
      const folder = CorpusFolder.of(CORPUS, run.folder);
      row.known_n_new = folder.nNew;
      row.known_d_wait = folder.dWait;
      row.known_m_comp = json.m0_private + json.m_cctx_est + json.bc * json.px * 3;
      row.known_m_dec = json.m0_private + json.m_dctx_est + json.bd * json.px * 3;
      for (const c of rowChecks(row, folder)) if (!c.note) row[`pass_${c.scenario}`] = c.ok ? 1 : 0;
    }
    csv.append(row);
    console.log(`[${i + 1}/${todo.length}] ${run.folder} ${run.way} L=${run.level} ${row.mode}/${row.layout} rep ${run.rep}: ${json ? `${json.bytes_per_frame} B/frame, ${(json.comp_cpu_per_frame_ns / 1e6).toFixed(2)} ms/frame, ${(wallMs / 1000).toFixed(1)} s` : error}`);
  }
}

/** Equation 6 from the pilot: T = Σ_runs N_new · px · 3 / v(L, m) · repeats. */
export function estimate(pilotCsv) {
  const [head, ...lines] = fs.readFileSync(pilotCsv, 'utf8').split(/\r?\n/).filter((l) => l.trim());
  const cols = head.split(',');
  const rows = lines.map((l) => Object.fromEntries(l.split(',').map((v, i) => [cols[i], v]))).filter((r) => !r.error);
  // v = raw bytes the run compressed per second of the whole subprocess (compress, decode, verify, start).
  const speed = (r) => (Number(r.frames) * Number(r.px) * 3) / (Number(r.run_wall_ms) / 1000);
  const vLevel = new Map();
  for (const r of rows.filter((x) => x.way === 'ZW1' && Number(x.workers || 0) === 0 && Number(x.drain || 0) === 0 && x.layout === 'bgr')) {
    const level = Number(r.level);
    vLevel.set(level, Math.max(vLevel.get(level) ?? 0, speed(r)));
  }
  const vBase = vLevel.get(-3);
  // A way's cost relative to ZW1 at L = -3, assumed the same at every level: v(L, m) = v_ZW1(L) · v_m(-3) / v_ZW1(-3).
  const ratio = new Map();
  for (const r of rows.filter((x) => Number(x.level) === -3)) {
    const key = wayKey(r);
    ratio.set(key, speed(r) / vBase);
  }
  const plan = fullPlan();
  let seconds = 0;
  const cells = new Map();
  const missing = new Set();
  for (const run of plan) {
    const folder = CorpusFolder.of(CORPUS, run.folder);
    const [w, h] = run.folder.split('-').pop().split('x').map(Number);
    const raw = folder.nNew * w * h * 3;
    const r = ratio.get(wayKey(run)) ?? (missing.add(wayKey(run)), 1);
    const v = (vLevel.get(run.level) ?? vBase) * r;
    const s = raw / v;
    seconds += s;
    const cell = `L=${run.level} ${w}x${h}`;
    cells.set(cell, (cells.get(cell) ?? 0) + s);
  }
  return { runs: plan.length, seconds, vLevel, ratio, cells, missing };
}

function wayKey(r) {
  const mode = r.mode || '';
  return [r.way, r.layout || '', mode, r.tile || '', r.wlog || '', r.dict || '', r.workers || '', r.kz || '', r.theta || '', r.drain || ''].join('/');
}

async function main() {
  const [flag, arg] = process.argv.slice(2);
  if (flag === '--plan') {
    const plan = fullPlan();
    console.log(`full run: ${plan.length} runs (${plan.length / REPEATS} configurations × ${REPEATS} repeats); not run: ${Object.keys(NOT_RUN).join(', ')}`);
  } else if (flag === '--full') {
    await runAll(fullPlan(), path.join(RESULTS, 'z-runs.csv'), Number(process.argv[4] ?? Infinity));
  } else if (flag === '--pilot') {
    await runAll(pilotPlan(arg ?? 'scroll-1920x1080'), path.join(RESULTS, 'z-pilot.csv'));
  } else if (flag === '--estimate') {
    const e = estimate(path.join(RESULTS, 'z-pilot.csv'));
    console.log(`T = ${(e.seconds / 3600).toFixed(1)} h over ${e.runs} runs`);
  } else {
    console.log('usage: node run-z.mjs --plan | --full [--limit N] | --pilot [folder] | --estimate');
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) await main();
