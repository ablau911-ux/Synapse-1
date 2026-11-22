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
  'm0_private', 'm0_ws', 'peak_private', 'peak_private_sampled', 'b_max', 'peak_ws', 'os_peak_private', 'os_peak_ws', 'm_cctx_est', 'cctx_sizeof', 'bc', 'm_dctx_est', 'bd',
  'dec_cpu_ns', 'dec_gpt_ns', 'dec_wall_ns', 'dec_cpu_per_frame_ns', 'dec_peak_private', 'dec_peak_ws', 'read_cpu_ns', 'corpus_dec_cpu_ns',
  'frames_decoded', 'diff_sum', 'diff_frames', 'n_dec_worst', 't_ready_ns', 'run_wall_ms',
];
const KNOWN_COLUMNS = ['rate', 'cell', 'held_out', 'known_n_new', 'known_d_wait', 'known_m_comp', 'known_m_dec'];
const PASS_COLUMNS = ['pass_frames', 'pass_bytes', 'pass_baseline', 'pass_baselineWriter', 'pass_ram', 'pass_ramWorkers', 'pass_ramStream', 'pass_cpuCycles', 'pass_roundTrip', 'pass_seek', 'pass_ramDec', 'pass_ready', 'error'];
const COLUMNS = [...OPTION_COLUMNS, ...MEASURED_COLUMNS, ...KNOWN_COLUMNS, ...PASS_COLUMNS];

const folders = () => SCENARIOS.flatMap((s) => SIZES.map((z) => `${s}-${z}`));
const optionKey = (r) => OPTION_COLUMNS.map((c) => String(r[c] ?? '')).join('|');
export const plannedKey = optionKey;
export const rowKey = optionKey;

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

// Run list v2 (manager, 2026-11-22 about 21:37): the coverage floor, then larger sizes by cost.
const FLOOR_SIZE = '640x360';
const LARGER_SIZES = ['506x333', '1280x720', '1920x1080'];
const BUDGET_S = 12 * 3600;
const HOLD_OUT_SHARE = 0.2;
const HOLD_OUT_SEED = 20261122;
const PILOT_PX = 1920 * 1080;
export const RUNLIST = path.join(RESULTS, 'z-runlist.json');

/** Every runnable way and option level swept over every level; ZW0 and the corpus writer's call once. */
function wayConfigs() {
  const once = GRID_B.filter((b) => b.way === 'ZW0' || b.drain);
  const swept = [...GRID_A_MODES, ...GRID_B.filter((b) => !once.includes(b))];
  return { swept, once };
}

/** Least squares y = t0 + a · x. */
function line(points) {
  const n = points.length;
  const mx = points.reduce((s, p) => s + p[0], 0) / n;
  const my = points.reduce((s, p) => s + p[1], 0) / n;
  const a = points.reduce((s, p) => s + (p[0] - mx) * (p[1] - my), 0) / points.reduce((s, p) => s + (p[0] - mx) ** 2, 0);
  return { t0: my - a * mx, a };
}

/**
 * Equation 6 with the pilot's measured terms, per run:
 *   t_run = t_0 + N_new · (px / px_pilot) · (a + c(L, m)),   c(L, m) = c_ZW1(L) · c_m(-3) / c_ZW1(-3)
 * i.e. T = Σ_runs (t_0 + N_new · px · 3 / v(L, m)) · repeats with v(L, m) = px_pilot · 3 / (a + c(L, m)).
 * t_0 = start cost of one subprocess; a = per-frame cost outside compressing (corpus decode, write,
 * decode and compare); c = compress wall time per frame. t_0 and a from a line fit over the ZW1 pilot
 * rows (5 to 400 frames): run wall − frames · c = t_0 + frames · a.
 */
export function costModel(pilotCsv) {
  const [head, ...lines] = fs.readFileSync(pilotCsv, 'utf8').split(/\r?\n/).filter((l) => l.trim());
  const cols = head.split(',');
  const rows = lines.map((l) => Object.fromEntries(l.split(',').map((v, i) => [cols[i], v]))).filter((r) => !r.error);
  const perFrame = (r) => Number(r.comp_wall_ns) / 1e9 / Number(r.frames);
  const zw1 = rows.filter((r) => r.way === 'ZW1' && r.drain === '0' && r.rep === '1');
  const { t0, a } = line(zw1.map((r) => [Number(r.frames), Number(r.run_wall_ms) / 1000 - Number(r.frames) * perFrame(r)]));
  const cLevel = new Map(zw1.map((r) => [Number(r.level), perFrame(r)]));
  const base = cLevel.get(-3);
  const atBase = rows.filter((r) => Number(r.level) === -3 && r.rep === '1');
  const ratio = (config) => {
    const row = atBase.find((r) => r.way === config.way && Object.entries(config)
      .filter(([k]) => !['folder', 'level', 'rep', 'frames_cap', 'way'].includes(k))
      .every(([k, v]) => String(r[k]) === String(v)));
    return row ? perFrame(row) / base : null;
  };
  const missing = new Set();
  const cost = (config, level, nNew, px) => {
    const k = ratio(config) ?? (missing.add(config.way), 1);
    const c = config.way === 'ZW0' ? k * base : (cLevel.get(level) ?? base) * k;
    return t0 + nNew * (px / PILOT_PX) * (a + c);
  };
  return { t0, a, cLevel, cost, missing };
}

/** A seeded generator (mulberry32), so the held-out cells are the same on every build of the list. */
function seeded(seed) {
  let s = seed >>> 0;
  return () => {
    s = (s + 0x6d2b79f5) >>> 0;
    let t = s;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/** results/z-runlist.json: cells (way, options, level, size; 6 contents × 3 repeats each), cheapest first. */
export function buildRunList(budgetS = BUDGET_S) {
  const model = costModel(path.join(RESULTS, 'z-pilot.csv'));
  const { swept, once } = wayConfigs();
  const cellCost = (config, level, size) => {
    const [w, h] = size.split('x').map(Number);
    return SCENARIOS.reduce((s, sc) => s + model.cost(config, level, CorpusFolder.of(CORPUS, `${sc}-${size}`).nNew, w * h), 0) * REPEATS;
  };
  const make = (config, level, size) => ({ ...config, level, size, cost_s: cellCost(config, level, size) });
  const floor = [...swept.flatMap((c) => LEVELS.map((l) => make(c, l, FLOOR_SIZE))), ...once.map((c) => make(c, -3, FLOOR_SIZE))];
  for (const cell of floor) cell.status = 'run';
  const floorS = floor.reduce((s, c) => s + c.cost_s, 0);
  const larger = [...swept.flatMap((c) => LEVELS.flatMap((l) => LARGER_SIZES.map((z) => make(c, l, z)))), ...once.flatMap((c) => LARGER_SIZES.map((z) => make(c, -3, z)))]
    .sort((x, y) => x.cost_s - y.cost_s);
  let total = floorS;
  for (const cell of larger) {
    if (total + cell.cost_s <= budgetS) {
      cell.status = 'run';
      total += cell.cost_s;
    } else {
      cell.status = 'predicted';
    }
  }
  // Hold-out: 20 % of the cells that run at more than one size, drawn from the larger sizes (the floor stays in the fit).
  const groupKey = (c) => JSON.stringify({ ...c, size: undefined, cost_s: undefined, status: undefined });
  const sizesRun = new Map();
  for (const c of [...floor, ...larger].filter((x) => x.status === 'run')) sizesRun.set(groupKey(c), (sizesRun.get(groupKey(c)) ?? 0) + 1);
  const multi = [...floor, ...larger].filter((c) => c.status === 'run' && sizesRun.get(groupKey(c)) > 1);
  const pool = multi.filter((c) => c.size !== FLOOR_SIZE);
  const random = seeded(HOLD_OUT_SEED);
  const shuffled = pool.map((c) => [random(), c]).sort((x, y) => x[0] - y[0]).map(([, c]) => c);
  for (const c of shuffled.slice(0, Math.round(HOLD_OUT_SHARE * multi.length))) c.status = 'held-out';
  const cells = [...floor, ...larger].sort((x, y) => x.cost_s - y.cost_s).map((c, i) => ({ id: i + 1, ...c, cost_s: Math.round(c.cost_s * 10) / 10 }));
  const count = (s) => cells.filter((c) => c.status === s).length;
  const list = {
    created: new Date().toISOString(),
    rule: 'coverage floor: every runnable way and option level x every Z1 level x 6 contents at 640x360; then the larger sizes cell by cell, cheapest predicted cell first, while T_total <= 12 h; cells left out are predicted by the size equations (eq. 1 eta, eq. 3 px-linear kappa_c); 20 % of the cells that run at more than one size are held out (seeded, drawn from the larger sizes)',
    equation: 'T = sum over runs of (t_0 + N_new * (px / px_pilot) * (a + c(L, m))) * repeats; c(L, m) = c_ZW1(L) * c_m(-3) / c_ZW1(-3); equation 6 with v(L, m) = px_pilot * 3 / (a + c(L, m)) plus t_0 per subprocess',
    coefficients: { t_0_s: model.t0, a_s_per_frame: model.a, px_pilot: PILOT_PX, c_ZW1_s_per_frame: Object.fromEntries(model.cLevel), pilot: 'results/z-pilot.csv' },
    repeats: REPEATS,
    contents: SCENARIOS,
    hold_out: { share: HOLD_OUT_SHARE, seed: HOLD_OUT_SEED, generator: 'mulberry32' },
    budget_h: budgetS / 3600,
    T_floor_h: floorS / 3600,
    T_total_h: total / 3600,
    counts: { cells: cells.length, run: count('run'), held_out: count('held-out'), predicted: count('predicted'), runs: (count('run') + count('held-out')) * SCENARIOS.length * REPEATS },
    ways_without_pilot_row: [...model.missing],
    not_run: NOT_RUN,
    cells,
  };
  fs.writeFileSync(RUNLIST, `${JSON.stringify(list, null, 1)}\n`);
  return list;
}

/** The runs of the list, cheapest cell first: every content, repeats 1 to 3. */
export function listRuns() {
  const list = JSON.parse(fs.readFileSync(RUNLIST, 'utf8'));
  return list.cells.filter((c) => c.status !== 'predicted').flatMap((c) => SCENARIOS.flatMap((sc) => Array.from({ length: REPEATS }, (_, i) => {
    const { id, size, status, cost_s: _cost, ...config } = c;
    return { ...config, folder: `${sc}-${size}`, rep: i + 1, cell: id, held_out: status === 'held-out' ? 1 : 0 };
  })));
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
    const row = { ...run, ...(json ?? {}), folder: run.folder, way: run.way, level: run.level, rep: run.rep, run_wall_ms: wallMs, rate, error };
    for (const key of ['layout', 'mode', 'tile', 'wlog', 'ldm', 'dict', 'workers', 'kz', 'theta', 'drain']) row[key] = json?.[key] ?? run[key] ?? '';
    if (json) {
      const folder = CorpusFolder.of(CORPUS, run.folder);
      row.known_n_new = folder.nNew;
      row.known_d_wait = folder.dWait;
      row.known_m_comp = json.m0_private + json.m_cctx_est + json.bc * json.px * 3;
      row.known_m_dec = json.m0_private + json.m_dctx_est + json.bd * json.px * 3 + json.b_max;
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
  const base = rows.filter((x) => Number(x.level) === -3);
  const ratioOf = (config) => {
    const row = base.find((r) => r.way === config.way && Object.entries(config)
      .filter(([k]) => !['folder', 'level', 'rep', 'frames_cap', 'way'].includes(k))
      .every(([k, v]) => String(r[k]) === String(v)));
    return row ? speed(row) / vBase : null;
  };
  const plan = fullPlan();
  let seconds = 0;
  const cells = new Map();
  const missing = new Set();
  for (const run of plan) {
    const folder = CorpusFolder.of(CORPUS, run.folder);
    const [w, h] = run.folder.split('-').pop().split('x').map(Number);
    const raw = folder.nNew * w * h * 3;
    const r = ratioOf(run) ?? (missing.add(run.way), 1);
    const v = (vLevel.get(run.level) ?? vBase) * r;
    const s = raw / v;
    seconds += s;
    const cell = `L=${run.level} ${w}x${h}`;
    cells.set(cell, (cells.get(cell) ?? 0) + s);
  }
  return { runs: plan.length, seconds, vLevel, cells, missing, rows };
}

/** results/z-pilot.md: the pilot table, the step2z output on the pilot CSV, and T. */
async function report() {
  const csvFile = path.join(RESULTS, 'z-pilot.csv');
  const e = estimate(csvFile);
  const { run: check } = await import('../spec/step2z.mjs');
  const lines = [];
  const code = check([csvFile, CORPUS], (l) => lines.push(l));
  const mb = (b) => (Number(b) / 1048576).toFixed(1);
  const ms = (ns) => (Number(ns) / 1e6).toFixed(2);
  const table = e.rows.map((r) => `| ${r.way} | ${r.layout}/${r.mode} | ${[r.tile && r.mode.match(/tiles|switch/) ? `tile ${r.tile}` : '', r.wlog !== '0' ? `W ${r.wlog}` : '', r.ldm === '1' ? 'ldm' : '', r.dict !== '0' ? `dict ${r.dict} KB` : '', r.workers !== '0' ? `workers ${r.workers}` : '', r.mode.match(/xor|sub|tiles|rect|prefix|stream|switch/) ? `K_z ${r.kz}` : '', r.mode === 'switch' ? `θ ${r.theta} %` : '', r.drain !== '0' ? `drain ${r.drain}` : ''].filter(Boolean).join(', ')} | ${r.level} | ${r.rep} | ${r.frames} | ${r.bytes_per_frame} | ${ms(r.comp_cpu_per_frame_ns)} | ${mb(r.peak_private - r.m0_private)} | ${mb(r.m_cctx_est)} | ${ms(r.dec_cpu_per_frame_ns)} | ${mb(r.dec_peak_private - r.m0_private)} | ${r.n_dec_worst} | ${r.diff_sum} | ${ms(r.t_ready_ns)} | ${(r.run_wall_ms / 1000).toFixed(1)} |`);
  const hours = e.seconds / 3600;
  const slow = [...e.cells.entries()].sort((a, b) => b[1] - a[1]).slice(0, 15)
    .map(([cell, s]) => `| ${cell} | ${(s / 3600).toFixed(2)} | ${(100 * s / e.seconds).toFixed(1)} % |`);
  const vTable = [...e.vLevel.entries()].sort((a, b) => a[0] - b[0]).map(([l, v]) => `| ${l} | ${(v / 1e6).toFixed(1)} |`);
  const errors = e.rows.length < fs.readFileSync(csvFile, 'utf8').trim().split('\n').length - 1;
  const md = [
    '# Clip experiments, way Z: pilot and run time T',
    '',
    `Pilot on \`scroll-1920x1080\` (N_new = 400, px = 2,073,600, 6,220,800 B per frame): ZW1 at all 33 levels, the baseline 3 times, and one run of every other runnable way at L = -3. Runner: \`node scripts/clip-equation/runs/run-z.mjs --pilot\`; raw rows: \`results/z-pilot.csv\`. Slow levels ran on the first frames only (\`frames_cap\`: 100 frames for L 7 to 12, 20 for L 13 to 18, 5 for L 19 to 22, 40 for the dictionary ways), because v is per byte.`,
    '',
    'Columns: bytes per frame (Σ file sizes on disk / frames); compress CPU per frame (process cycles, user plus kernel, every thread, at the measured cycle rate; corpus decode and file writes excluded); RAM = peak private bytes over M_0; M_cctx = zstd\'s own estimate; decode CPU per frame; decoder RAM = decode-phase peak private bytes over M_0; N_dec = frames decoded to show the worst frame; Σ|diff| = round trip over every pixel; t_ready = last frame compressed to clip.json closed; wall = the whole subprocess.',
    '',
    '| way | layout/mode | options | L | rep | frames | B/frame | comp ms/frame | RAM MB | M_cctx MB | dec ms/frame | dec RAM MB | N_dec | Σ\\|diff\\| | t_ready ms | wall s |',
    '|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|',
    ...table,
    '',
    errors ? 'Some pilot runs failed; see the `error` column of the CSV.' : 'Every pilot run finished.',
    '',
    'Ways not run (docs/clip-recorder-ways.md section 4):',
    ...Object.entries(NOT_RUN).map(([id, why]) => `- ${id}: ${why}.`),
    '',
    '## Check output (`node scripts/clip-equation/spec/step2z.mjs results/z-pilot.csv`)',
    '',
    '```',
    ...lines,
    '```',
    '',
    `Exit code ${code}.`,
    '',
    '### Rows that miss, and their causes (pilot of 2026-11-22)',
    '',
    '- **Baseline equals the corpus.** Same libzstd on both sides (zstd-sys 2.1.3 = zstd 1.5.7; Node `process.versions.zstd` = 1.5.7) and the same frame header (frame header descriptor 0x80: 4-byte content size, no single-segment flag, no checksum, no dictionary; the first 14 bytes are identical). The bodies differ: the corpus writer (`corpus/render.mjs`, `zlib.zstdCompressSync`) drains the output 16 KB at a time, which puts zstd in buffered mode, and its block splitting differs from the one-shot `ZSTD_compress2` of the product (`zstd::bulk::compress`). Proof: Node itself gives 1,782 B for typing-1280x720 frame 0 at chunkSize 16 KB and 64 KB, and 1,770 B at 1 MB and 8 MB, the same as exp-z one-shot; exp-z with `drain=16384` gives D_wait to the byte (49,677,690 B). One-shot gives 49,650,183 B, 27,507 B (0.055 %) fewer. The row stays a miss: the known result assumed the same call, not only the same coder.',
    '- **RAM, ZW17 nbWorkers 1 and 2.** zstd\'s estimate functions cover single-threaded compression only (zstd.h: "Estimate CCtx size is supported for single-threaded compression only"), so M_cctx is the single-thread estimate (0.52 MB). `ZSTD_sizeof_CCtx` after the run is 10.5 MB (1 worker) and 15.1 MB (2 workers), and M_0 + that + B_c · px · 3 matches the measured peak within 1 %. No zstd estimate exists for this row.',
    '- **RAM and decoder RAM, ZW13 with K_z = 1.** `ZSTD_estimateCStreamSize_usingCCtxParams` and `ZSTD_estimateDStreamSize` take no source size, so they count a 2^W window (16 MB at W = 24). With K_z = 1 every zstd frame ends in its first call, so zstd knows the 6.2 MB frame size and sizes the window to it (`ZSTD_sizeof_CCtx` 6.7 MB). Every other stream row matches its estimate exactly (`ZSTD_sizeof_CCtx` = the estimate to the byte for W 23, 24, 25 and 27).',
    '- **Corrections made in this step, before the rerun** (first pilot, kept out of the CSV): long-distance matching crashed inside zstd\'s estimate with a division by zero (`STATUS_INTEGER_DIVIDE_BY_ZERO`) because unset ldm fields stay 0 there; the ldm parameters are now set explicitly on the context and in the estimate. B_c and B_d now count every buffer the way allocates up front (a B,G,R,A buffer is 4/3 of px · 3; the gathered tiles and the delta target are allocated at the start, so a reallocation can no longer double them for a moment). The compress peak is the operating system\'s exact `PeakPagefileUsage` when compressing sets a new peak (the 50 ms sampler missed short peaks). Equation 4 for the decoder gains b_max, the largest clip file, which the viewer holds while it decodes (ZW0 raw: 6.2 MB).',
    '- **CPU.** `GetProcessTimes` advances in clock ticks of 15.6 ms; around one frame (about 2 ms at L = -3) it read 0 for whole runs and 0.5 to 0.8 of the true time over 400 frames. Compress and decode CPU are therefore the process cycle counter (`QueryProcessCycleTime`, user plus kernel, every thread) at the measured cycle rate; the `GetProcessTimes` sums are kept in the `*_gpt_ns` columns.',
    '',
    '## Run time of the full run (equation 6)',
    '',
    '    T = Σ_runs N_new · px · 3 / v(L, m) · repeats',
    '    v(L, m) = v_ZW1(L) · v_m(-3) / v_ZW1(-3)        v = raw bytes per second of the whole run subprocess',
    '',
    `Runs: ${e.runs} (Grid A-Z 4,752 + Grid B ${e.runs / REPEATS - 4752} configurations, × ${REPEATS} repeats), over every folder's own N_new and px.`,
    '',
    `**T = ${hours.toFixed(1)} h** (${Math.round(e.seconds)} s).`,
    '',
    'Assumptions: v per raw byte measured on scroll at 1920 × 1080 holds for every content and size; a way\'s cost ratio to ZW1 at L = -3 holds at every level (for tiles and rectangles zstd sees only the changed part, so at slow levels this overestimates their time). Video frames compress worst, so their v at slow levels is the largest unknown.',
    e.missing.size ? `Ways with no pilot row, counted at the ZW1 speed: ${[...e.missing].join(', ')}.` : '',
    '',
    '| L | v_ZW1(L), MB/s of raw frames |',
    '|---|---|',
    ...vTable,
    '',
    hours > 10 ? '### Slowest level and size cells (all modes, contents and repeats)\n\n| cell | hours | share of T |\n|---|---|---|\n' + slow.join('\n') : '',
    '',
  ].join('\n');
  fs.writeFileSync(path.join(RESULTS, 'z-pilot.md'), md);
  console.log(`wrote z-pilot.md; T = ${hours.toFixed(1)} h; step2z exit ${code}`);
}

async function main() {
  const [flag, arg] = process.argv.slice(2);
  if (flag === '--plan') {
    const plan = fullPlan();
    console.log(`full run: ${plan.length} runs (${plan.length / REPEATS} configurations × ${REPEATS} repeats); not run: ${Object.keys(NOT_RUN).join(', ')}`);
  } else if (flag === '--runlist') {
    const list = buildRunList(arg ? Number(arg) * 3600 : BUDGET_S);
    console.log(`z-runlist.json: T_total ${list.T_total_h.toFixed(2)} h (floor ${list.T_floor_h.toFixed(2)} h); cells ${JSON.stringify(list.counts)}; t_0 ${list.coefficients.t_0_s.toFixed(3)} s, a ${(list.coefficients.a_s_per_frame * 1e3).toFixed(2)} ms/frame; no pilot row: ${list.ways_without_pilot_row.join(', ') || 'none'}`);
  } else if (flag === '--full') {
    await runAll(listRuns(), path.join(RESULTS, 'z-runs.csv'), Number(process.argv[4] ?? Infinity));
  } else if (flag === '--pilot') {
    await runAll(pilotPlan(arg ?? 'scroll-1920x1080'), path.join(RESULTS, 'z-pilot.csv'));
  } else if (flag === '--estimate') {
    const e = estimate(path.join(RESULTS, 'z-pilot.csv'));
    console.log(`T = ${(e.seconds / 3600).toFixed(1)} h over ${e.runs} runs`);
  } else if (flag === '--report') {
    await report();
  } else {
    console.log('usage: node run-z.mjs --plan | --full [--limit N] | --pilot [folder] | --estimate | --report');
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) await main();
