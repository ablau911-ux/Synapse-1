// Experiments steps 2 and 3, way Z: the checks of scripts/clip-equation/spec/step2z.feature.
// Spec: docs/clip-recorder-experiments-plan.md, "Acceptance (BDD)", Z rows of steps 2 and 3.
//
//   node step2z.mjs <z-runs.csv> [corpus root]
//
// Each check computes its known result from its inputs (the corpus folder on disk,
// zstd's own estimates printed by the probe, the plan's equations), prints the
// equation, the known value and the measured value, and the outcome. Exit code 1
// when any check misses. CPU and run time need the fit and the full run: they are
// printed as PENDING and do not pass.
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import * as runner from '../runs/run-z.mjs';

const DEFAULT_CORPUS = 'C:\\Users\\Adam\\clip-corpus';
const INSTANT_MS = 50;
const RAM_TOLERANCE = 0.10;
// Dictionary training uses the first 20 new frames (plan factor Z4; exp_z.rs DICT_TRAIN_FRAMES).
const DICT_TRAIN_FRAMES = 20;
// Ways with independent frames (seek 1); every other way keeps the previous frame or a stream.
const INDEPENDENT_WAYS = new Set(['ZW0', 'ZW1', 'ZW2', 'ZW3', 'ZW4', 'ZW5', 'ZW6', 'ZW16', 'ZW17']);

/** The CSV the runner appends: a header line, then one row per run; no quoted fields. */
export function readRuns(csvPath) {
  if (!csvPath || !fs.existsSync(csvPath)) return { rows: [], error: `no CSV at ${csvPath ?? '(none given)'}` };
  const lines = fs.readFileSync(csvPath, 'utf8').split(/\r?\n/).filter((l) => l.trim() !== '');
  if (lines.length < 2) return { rows: [], error: `${csvPath} has no rows` };
  const head = lines[0].split(',');
  const rows = lines.slice(1).map((line) => Object.fromEntries(line.split(',').map((v, i) => [head[i], v])));
  return { rows, error: null };
}

/** N_new and D_wait of a corpus folder, read from the folder itself (not from the run). */
class CorpusFolder {
  static cache = new Map();

  static of(root, name) {
    const key = path.join(root, name);
    if (!CorpusFolder.cache.has(key)) CorpusFolder.cache.set(key, new CorpusFolder(key));
    return CorpusFolder.cache.get(key);
  }

  constructor(folder) {
    const lines = fs.readFileSync(path.join(folder, 'frames.jsonl'), 'utf8').split('\n').filter((l) => l.trim());
    this.lines = lines.map((l) => JSON.parse(l));
    this.nNew = this.lines.filter((r) => r.new).length;
    this.dWait = fs.readdirSync(folder)
      .filter((f) => /^frame-\d{4}\.zst$/.test(f))
      .reduce((sum, f) => sum + fs.statSync(path.join(folder, f)).size, 0);
  }

  /** Instants whose picture is one of the first `frames` new frames: the lines of the clip's frames.jsonl. */
  instantsFor(frames) {
    return this.lines.filter((l) => Number.isInteger(l.frame) && l.frame < frames).length;
  }
}

const num = (v) => (v === undefined || v === '' ? NaN : Number(v));
const isIndependent = (r) => INDEPENDENT_WAYS.has(r.way);
const within = (known, measured, tol) => Math.abs(measured - known) <= tol * Math.abs(measured);

/**
 * The known compress RAM of one row (equation 4 with the step 2a corrections), the one
 * calculation both the checks and the runner's saved `known_m_comp` use (#295 finding 19).
 * Returns { scenario, known, label }.
 */
export function ramKnown(r) {
  const px = num(r.w) * num(r.h);
  const frames = num(r.bc) * px * 3;
  if (num(r.workers) > 0) {
    // zstd's estimates cover one thread only: M_cctx = ZSTD_sizeof_CCtx, read after the last frame.
    const known = num(r.m0_private) + num(r.cctx_sizeof) + frames;
    return { scenario: 'ramWorkers', known, label: Math.round(known) };
  }
  if (r.mode === 'stream') {
    // The stream estimate counts a window of 2^W. With K_z = 1 each zstd frame ends in its first call,
    // so zstd knows the frame size and the window is W_eff = min(2^W, px · 3) (ZSTD_resetCCtx_internal:
    // windowSize = MIN(1 << windowLog, pledgedSrcSize)); with K_z > 1 the size is unknown: W_eff = 2^W.
    const window = 2 ** num(r.wlog);
    const wEff = num(r.kz) === 1 ? Math.min(window, px * 3) : window;
    const known = num(r.m0_private) + num(r.m_cctx_est) - window + wEff + frames;
    return { scenario: 'ramStream', known, label: `${Math.round(known)} (W_eff ${wEff})` };
  }
  const known = num(r.m0_private) + num(r.m_cctx_est) + frames;
  return { scenario: 'ram', known, label: Math.round(known) };
}

/**
 * The known decoder RAM of one row before the fitted m_d0 (equation 4): M_0 + M_dctx + B_d · px · 3 + b_max.
 * M_dctx is the probe's estimate (single-pass DCtx for one stream with K_z = 1). One calculation for the
 * checks and the runner's saved `known_m_dec`.
 */
export function ramDecKnown(r) {
  return num(r.m0_private) + num(r.m_dctx_est) + num(r.bd) * num(r.w) * num(r.h) * 3 + (num(r.b_max) || 0);
}

/** The per-row checks of one run: { scenario, known, measured, ok }, { scenario, note } or { scenario, known, measured, fit: true }. */
export function rowChecks(r, folder) {
  const px = num(r.w) * num(r.h);
  // A pilot run may cover only the first `frames_cap` new frames: then N_comp = that cap.
  const cap = num(r.frames_cap);
  const nKnown = Number.isFinite(cap) && cap > 0 ? Math.min(cap, folder.nNew) : folder.nNew;
  const out = [];
  const add = (scenario, known, measured, ok) => out.push({ scenario, known, measured, ok });
  add('frames', nKnown, num(r.frames), num(r.frames) === nKnown);
  add('bytes', `Σ out ${r.bytes_out}`, `on disk ${r.bytes_disk}`, num(r.bytes_out) === num(r.bytes_disk) && num(r.bytes_disk) > 0);
  // M4: every retained file counts: the frame files, frames.jsonl (one line per instant) and clip.json.
  const files = num(r.frame_bytes) + num(r.index_bytes) + num(r.clip_json_bytes);
  const lines = folder.instantsFor(nKnown);
  add('bytesAll', `frames ${r.frame_bytes} + frames.jsonl ${r.index_bytes} (${lines} lines) + clip.json ${r.clip_json_bytes}`,
    `on disk ${r.bytes_disk}, ${r.index_lines} lines`,
    num(r.bytes_disk) === files && num(r.index_bytes) > 0 && num(r.clip_json_bytes) > 0 && num(r.index_lines) === lines);
  if (r.way === 'ZW1' && num(r.level) === -3 && num(r.workers) === 0 && r.layout === 'bgr' && nKnown === folder.nNew) {
    if (num(r.drain || 0) === 0) {
      // The product's one-shot call: a known difference from D_wait (block splits of the call), reported, not pass/fail.
      out.push({ scenario: 'oneShot', note: `one-shot ZSTD_compress2: frame files ${r.frame_bytes} B, D_wait ${folder.dWait} B, difference ${num(r.frame_bytes) - folder.dWait} B (${r.folder})` });
    } else {
      // The corpus writer's call (ZSTD_compressStream2, output drained 16 KB at a time, as Node
      // zlib.zstdCompressSync) gives D_wait to the byte: strict, never loosened.
      add('baseline', folder.dWait, num(r.frame_bytes), num(r.frame_bytes) === folder.dWait);
    }
  }
  // M1: every way receives B,G,R,A; its conversion is timed (none only for the B,G,R,A layout).
  const converts = r.layout !== 'bgra';
  add('inputFormat', `input bgra, convert cycles ${converts ? '> 0' : '= 0'}`, `input ${r.input}, convert cycles ${r.convert_cycles}`,
    r.input === 'bgra' && (converts ? num(r.convert_cycles) > 0 : num(r.convert_cycles) === 0));
  // M2: the way's CPU is all of its work: conversion, context and compression, writing, dictionary training.
  const parts = ['convert', 'comp', 'write', 'train'].reduce((s, p) => s + num(r[`${p}_cycles`]), 0);
  add('cpuTotal', `Σ cycles ${parts}`, num(r.way_cycles), num(r.way_cycles) === parts && parts > 0);
  if (num(r.dict) > 0) add('dictCpu', 'train cycles > 0, inside the way\'s CPU', `train ${r.train_cycles}, way ${r.way_cycles}`, num(r.train_cycles) > 0 && num(r.way_cycles) >= num(r.train_cycles));
  // M3: RAM = the OS peak of private bytes after the work minus M_0, taken after the input is prepared.
  add('ramOs', `peak ${r.os_peak_private} − M_0 ${r.m0_private}, peak > peak at M_0 ${r.os_peak_at_m0}`, `ram ${r.ram_os}, peak used ${r.peak_private}`,
    num(r.ram_os) === num(r.os_peak_private) - num(r.m0_private) && num(r.peak_private) === num(r.os_peak_private) && num(r.os_peak_private) > num(r.os_peak_at_m0));
  // M8: ldm set explicitly on the context: 1 enabled, 2 disabled (0 would leave zstd's automatic choice).
  if (r.mode !== 'raw') add('ldm', num(r.ldm) === 1 ? 1 : 2, num(r.ldm_read), num(r.ldm_read) === (num(r.ldm) === 1 ? 1 : 2));
  // M8: ZW9 codes the listed rectangles; ZW9b one global box.
  if (r.mode === 'rect' || r.mode === 'box') {
    const box = r.mode === 'box';
    add('rectList', `Σ listed rectangle bytes ${r.rect_listed_bytes}${box ? ', one box per frame' : ''}`, `Σ bytes compressed ${r.rect_src_bytes}, at most ${r.rects_max} per frame`,
      num(r.rect_listed_bytes) === num(r.rect_src_bytes) && (box ? r.way === 'ZW9b' && num(r.rects_max) <= 1 : r.way === 'ZW9' && num(r.gap) >= 0));
  }
  const ram = ramKnown(r);
  if (num(r.dict) > 0) {
    // M_comp(ZW16) gains M_train = μ · S_samples, μ fitted in step 4: known after the fit.
    const samples = Math.min(DICT_TRAIN_FRAMES, nKnown) * px * 3;
    out.push({ scenario: 'ramDict', fit: true, known: `${ram.label} + μ · S_samples (S_samples ${samples})`, measured: num(r.peak_private) });
  } else {
    add(ram.scenario, ram.label, num(r.peak_private), within(ram.known, num(r.peak_private), RAM_TOLERANCE));
  }
  // The decoder context zstd allocated equals its estimate (one stream with K_z = 1: single pass, no window).
  if (r.mode !== 'raw') add('dctx', num(r.m_dctx_est), num(r.dctx_sizeof), num(r.dctx_sizeof) === num(r.m_dctx_est));
  if (r.known_m_comp !== undefined) add('ramShared', Math.round(ram.known), num(r.known_m_comp), num(r.known_m_comp) === ram.known);
  const rate = num(r.rate);
  if (rate > 0) {
    // CPU = process cycles (QueryProcessCycleTime, user plus kernel, every thread) / rate · 10^9; one
    // thread's CPU cannot exceed the wall time of the same calls (2 % for the 50 ms memory sampler).
    const workers = num(r.workers) > 0;
    const ns = (num(r.comp_cycles) / rate) * 1e9;
    const ok = Math.abs(ns - num(r.comp_cpu_ns)) <= 1 + 1e-9 * ns && (workers || num(r.comp_cpu_ns) <= 1.02 * num(r.comp_wall_ns));
    add('cpuCycles', `${Math.round(ns)} ns${workers ? '' : ` <= 1.02 · wall ${r.comp_wall_ns}`}`, num(r.comp_cpu_ns), ok);
  }
  add('roundTrip', 0, num(r.diff_sum), num(r.diff_sum) === 0 && num(r.frames_decoded) === nKnown);
  const kz = r.kz === 'all' ? folder.nNew : num(r.kz);
  const independent = isIndependent(r);
  const seekOk = independent ? num(r.n_dec_worst) === 1 : num(r.n_dec_worst) >= 1 && num(r.n_dec_worst) <= kz;
  add('seek', independent ? 1 : `<= ${kz}`, num(r.n_dec_worst), seekOk);
  // + b_max: the compressed frame the viewer reads is held too (largest clip file, known after compressing).
  // M_dec gains m_d0, fitted in step 4: known after the fit.
  out.push({ scenario: 'ramDec', fit: true, known: `${Math.round(ramDecKnown(r))} + m_d0`, measured: num(r.dec_peak_private) });
  const readyMs = num(r.t_ready_ns) / 1e6;
  add('ready', `<= ${INSTANT_MS} ms`, `${readyMs.toFixed(2)} ms`, readyMs <= INSTANT_MS);
  return out;
}

/**
 * M9 and #295 findings 1 and 2, on the runner itself: the run list is floor + 4 h with 20 % of the
 * multi-size cells held out (mulberry32, seed 20261122), and every row of the CSV is found again by
 * the key the runner plans with, so a restart never repeats a finished run.
 */
export function runnerChecks(rows, s) {
  const listFile = runner.RUNLIST;
  if (!listFile || !fs.existsSync(listFile)) {
    s.runList.check({ folder: 'z-runlist.json' }, 'a run list', 'none', false);
  } else {
    const list = JSON.parse(fs.readFileSync(listFile, 'utf8'));
    const live = list.cells.filter((c) => c.status !== 'predicted');
    const groupKey = (c) => JSON.stringify({ ...c, id: undefined, size: undefined, cost_s: undefined, status: undefined });
    const sizes = new Map();
    for (const c of live) sizes.set(groupKey(c), (sizes.get(groupKey(c)) ?? 0) + 1);
    const multi = live.filter((c) => sizes.get(groupKey(c)) > 1).length;
    const held = list.cells.filter((c) => c.status === 'held-out').length;
    const knownHeld = Math.round(0.2 * multi);
    const extra = list.budget_h - list.T_floor_h;
    s.runList.check({ folder: 'z-runlist.json' },
      `budget = T_floor + 4 h = ${(list.T_floor_h + 4).toFixed(2)} h, T_total <= budget, held-out = round(0.2 · ${multi}) = ${knownHeld}, mulberry32 seed 20261122`,
      `budget ${list.budget_h.toFixed(2)} h, T_total ${list.T_total_h.toFixed(2)} h, held-out ${held}, ${list.hold_out?.generator} seed ${list.hold_out?.seed}`,
      Math.abs(extra - 4) < 1e-9 && list.T_total_h <= list.budget_h && held === knownHeld && held > 0
        && list.hold_out?.seed === 20261122 && list.hold_out?.generator === 'mulberry32');
  }
  // Planning twice gives the same keys and no key twice.
  const first = runner.listRuns().map(runner.plannedKey);
  const second = runner.listRuns().map(runner.plannedKey);
  const unique = new Set(first).size;
  s.resume.check({ folder: 'run list' }, `${first.length} unique keys, the same twice`, `${unique} unique, ${second.length} on the second plan`,
    unique === first.length && first.every((k, i) => k === second[i]));
  // Every saved row is found by the planned key of the run it came from (pilot plan of its folder, or the run list).
  const planned = new Set(first);
  for (const folder of new Set(rows.map((r) => r.folder))) for (const run of runner.pilotPlan(folder)) planned.add(runner.plannedKey(run));
  const saved = rows.map(runner.rowKey);
  const missing = rows.filter((r, i) => !planned.has(saved[i]));
  s.resume.check({ folder: 'CSV' }, `${rows.length} saved rows found by their planned key`, `${rows.length - missing.length} found${missing.length ? `; first missing ${saved[rows.indexOf(missing[0])]}` : ''}`, missing.length === 0 && rows.length > 0);
  const savedUnique = new Set(saved).size;
  s.resume.check({ folder: 'CSV' }, `${rows.length} saved keys, no key twice`, `${savedUnique} unique`, savedUnique === rows.length);
}

export { CorpusFolder };

const PROBE = path.join(path.dirname(fileURLToPath(import.meta.url)), '..', 'probe', 'target', 'release', 'clip-probe.exe');

/** One small Z run (ZW1, L = -3, 20 frames of typing at 640 x 360) by the probe itself; its JSON line. */
function smallZRun(corpusRoot) {
  if (!fs.existsSync(PROBE)) return { error: `no probe at ${PROBE}` };
  const rate = /rate=\d+/.exec(spawnSync(PROBE, ['exp-rate'], { encoding: 'utf8', windowsHide: true }).stdout || '')?.[0] ?? 'rate=0';
  const out = path.join(os.tmpdir(), `clip-exp-z-check-${process.pid}`);
  try {
    const res = spawnSync(PROBE, ['exp-z', `folder=${path.join(corpusRoot, 'typing-640x360')}`, `out=${out}`, rate, 'way=ZW1', 'level=-3', 'rep=1', 'frames=20'], { encoding: 'utf8', windowsHide: true });
    const line = (res.stdout || '').split(/\r?\n/).find((l) => l.startsWith('{'));
    return line ? JSON.parse(line) : { error: `no JSON, exit ${res.status}` };
  } finally {
    fs.rmSync(out, { recursive: true, force: true });
  }
}

/** Round 2 of #295 (Codex re-review at cfe995d4): M6 no sampler thread in Z's measured work; M3 run list identity. */
function roundTwoChecks(rows, s, corpusRoot) {
  const small = smallZRun(corpusRoot);
  s.samplerFree.check({ folder: 'typing-640x360', way: 'ZW1', level: -3, mode: 'whole', layout: 'bgr', rep: 1 }, '0 samplers',
    small.error ?? `${small.samplers_in_work ?? 'not reported'} samplers, way_cpu_ns ${small.way_cpu_ns}`, small.samplers_in_work === 0);
  for (const r of rows.filter((x) => x.samplers_in_work !== undefined && x.samplers_in_work !== '')) s.samplerFree.check(r, '0 samplers', r.samplers_in_work, r.samplers_in_work === '0');
  if (typeof runner.runListIdentity !== 'function') {
    s.listId.check({ folder: 'z-runlist.json' }, 'hash', 'run-z.mjs has no run list identity', false);
    return;
  }
  const list = JSON.parse(fs.readFileSync(runner.RUNLIST, 'utf8'));
  const own = runner.runListIdentity(list);
  const preflight = runner.preflightRunList({ write: false });
  s.listId.check({ folder: 'z-runlist.json' }, `hash ${own}; plan now ${preflight.planId}`, `file ${list.hash ?? 'none'}${preflight.mismatches.length ? `; differs: ${preflight.mismatches.join(', ')}` : ''}`,
    list.hash === own && own === preflight.planId && preflight.mismatches.length === 0);
  const full = rows.filter((r) => r.cell);
  const bad = full.filter((r) => r.runlist_hash !== list.hash);
  if (full.length) s.listId.check({ folder: 'CSV' }, `${full.length} full-run rows name ${list.hash}`, `${full.length - bad.length} do`, bad.length === 0);
}

/** One Gherkin scenario: its equation and a check per row. */
class Scenario {
  constructor(name, equation) {
    this.name = name;
    this.equation = equation;
    this.passed = 0;
    this.misses = [];
    this.notes = [];
    this.pending = null;
    this.fitRows = [];
  }

  /** A row whose known value needs a coefficient fitted in step 4: printed, never a pass or a fail here. */
  fitRow(row, known, measured) {
    this.fitRows.push(`${row.folder} ${row.way} L=${row.level} ${row.mode}/${row.layout} rep ${row.rep}: known ${known}, measured ${measured}`);
  }

  check(row, known, measured, ok) {
    if (ok) this.passed += 1;
    else this.misses.push(`${row.folder} ${row.way} L=${row.level} ${row.mode}/${row.layout} rep ${row.rep}: known ${known}, measured ${measured}`);
  }

  get ok() {
    return this.pending === null && this.misses.length === 0 && this.passed > 0;
  }

  /** Counted neither as a pass nor as a failure: printed as INFO (a known difference the plan reports). */
  get counted() {
    return this.pending === null && !this.info;
  }

  print(out) {
    const state = this.info ? 'INFO' : this.fit ? 'PENDING-FIT' : this.pending !== null ? 'PENDING' : this.ok ? 'PASS' : 'FAIL';
    out(`${state}  ${this.name}: ${this.equation}  (${this.passed} rows pass, ${this.misses.length} miss)`);
    if (this.pending !== null) out(`        ${this.pending}`);
    if (this.passed === 0 && this.misses.length === 0 && this.pending === null) out('        no rows to check');
    for (const miss of this.misses.slice(0, 8)) out(`        miss: ${miss}`);
    if (this.misses.length > 8) out(`        ... ${this.misses.length - 8} more`);
    for (const note of this.notes.slice(0, 8)) out(`        ${note}`);
    for (const row of this.fitRows.slice(0, 4)) out(`        ${row}`);
    if (this.fitRows.length) out(`        ${this.fitRows.length} rows, checked on the held-out rows in step 4`);
  }
}

export function run(argv = process.argv.slice(2), out = console.log) {
  const [csvPath, corpusRoot = DEFAULT_CORPUS] = argv;
  const { rows, error } = readRuns(csvPath);
  out(`step2z: ${rows.length} rows from ${csvPath ?? '(none)'}${error ? ` (${error})` : ''}`);
  const s = {
    frames: new Scenario('frames compressed', 'N_comp = N_new of the folder'),
    bytes: new Scenario('bytes', 'bytes = Σ output sizes = Σ clip file sizes on disk'),
    bytesAll: new Scenario('bytes of every retained file', 'bytes = Σ frame files + frames.jsonl (one line per instant) + clip.json'),
    baseline: new Scenario('baseline equals the corpus', 'ZW1, L = -3, the corpus writer\'s call (16 KB output drain): frame bytes = D_wait'),
    oneShot: new Scenario('one-shot call', 'ZW1, L = -3, one ZSTD_compress2 per frame: frame bytes − D_wait, reported'),
    inputFormat: new Scenario('input format', 'every way receives B,G,R,A; the conversion it needs is timed (convert cycles > 0 unless the layout is B,G,R,A)'),
    cpuTotal: new Scenario('CPU of all the way\'s work', 'way cycles = convert + compress (with context creation) + write + dictionary training'),
    dictCpu: new Scenario('dictionary training CPU', 'dict > 0: train cycles > 0 and inside the way\'s CPU'),
    ramOs: new Scenario('RAM from the OS peak', 'RAM = PeakPagefileUsage after the work − M_0 (M_0 after the input is prepared); the work sets the peak'),
    ldm: new Scenario('ldm set explicitly', 'ZSTD_c_enableLongDistanceMatching read back = 1 when ldm=1, 2 when ldm=0'),
    rectList: new Scenario('ZW9 rectangle list, ZW9b one box', 'Σ bytes compressed = Σ rows · bytes of the listed rectangles; ZW9b at most 1 rectangle per frame'),
    ramShared: new Scenario('saved RAM prediction', 'known_m_comp of the runner = the check\'s equation 4 (with the workers and W_eff corrections)'),
    runList: new Scenario('run list', 'budget = T_floor + 4 h; T_total <= budget; held-out = round(0.2 · multi-size cells); mulberry32 seed 20261122'),
    resume: new Scenario('resume keys', 'planned key = saved key for every row; planning twice gives the same unique keys'),
    determinism: new Scenario('determinism', 'bytes(run 1) = bytes(run 2) = bytes(run 3)'),
    ram: new Scenario('RAM while compressing', 'M_comp = M_0 + M_cctx + B_c · px · 3 within ±10 % of peak private bytes (one thread, frames)'),
    ramWorkers: new Scenario('RAM, nbWorkers > 0', 'M_comp = M_0 + ZSTD_sizeof_CCtx + B_c · px · 3 within ±10 %'),
    ramStream: new Scenario('RAM, one stream', 'M_comp = M_0 + M_cctx − 2^W + W_eff + B_c · px · 3 within ±10 %; W_eff = min(2^W, px · 3) when K_z = 1, else 2^W'),
    cpuCycles: new Scenario('CPU from the cycle counter', 'C = cycles / rate · 10^9 (rate from clip-probe exp-rate); one thread: C <= 1.02 · wall'),
    cpu: new Scenario('CPU', 'C_comp = N_new · (c_0 + px · κ_c), held-out rows within ±10 %'),
    runtime: new Scenario('run time', 'full-run wall time within ±20 % of T (equation 6)'),
    roundTrip: new Scenario('round trip', 'Σ |decoded − corpus| = 0'),
    seek: new Scenario('seek cost', 'N_dec(worst) = 1 independent; <= K_z otherwise'),
    ramDec: new Scenario('decoder RAM', 'M_dec = M_0 + M_dctx + B_d · px · 3 + b_max + m_d0 (m_d0 fitted) within ±10 % of decode peak private bytes'),
    ramDict: new Scenario('RAM with a trained dictionary', 'M_comp(ZW16) = M_0 + M_cctx + B_c · px · 3 + μ · S_samples (μ fitted), S_samples = N_train · px · 3'),
    dctx: new Scenario('decoder context', 'M_dctx (zstd estimate; one stream with K_z = 1: ZSTD_estimateDCtxSize, single pass) = ZSTD_sizeof_DCtx after the last frame'),
    ready: new Scenario('time until ready', `t_ready = t_close <= ${INSTANT_MS} ms`),
    samplerFree: new Scenario('no memory sampler in the measured work', 'memory samplers running during the way\'s measured work = 0 (RAM is the OS peak; CPU measured as in way D)'),
    listId: new Scenario('run list identity', 'z-runlist.json hash = hash of its plan = hash of the plan rebuilt now (budget, factors, hold-out seed); every full-run row names it'),
  };
  s.cpu.pending = 'needs κ_c fitted on the full run (step 4); the pilot reports C_comp per frame only';
  s.runtime.pending = 'needs the full run; z-pilot.md gives T';
  s.oneShot.info = true;
  for (const name of ['ramDec', 'ramDict']) {
    s[name].fit = true;
    s[name].pending = 'known after the fit (step 4)';
  }

  const configKey = (r) => runner.rowKey({ ...r, rep: '' });
  const byConfig = new Map();
  for (const r of rows) {
    let checks;
    try {
      checks = rowChecks(r, CorpusFolder.of(corpusRoot, r.folder));
    } catch (e) {
      s.frames.check(r, 'corpus folder', `unreadable: ${e.message}`, false);
      continue;
    }
    for (const c of checks) {
      if (c.note) s[c.scenario].notes.push(c.note);
      else if (c.fit) s[c.scenario].fitRow(r, c.known, c.measured);
      else s[c.scenario].check(r, c.known, c.measured, c.ok);
    }
    const key = configKey(r);
    if (!byConfig.has(key)) byConfig.set(key, []);
    byConfig.get(key).push(r);
  }
  for (const group of byConfig.values()) {
    if (group.length < 2) continue;
    const sizes = new Set(group.map((r) => r.bytes_disk));
    s.determinism.check(group[0], `one size over ${group.length} runs`, [...sizes].join(' / '), sizes.size === 1);
  }

  try {
    runnerChecks(rows, s);
  } catch (e) {
    s.resume.check({ folder: 'runner' }, 'the runner\'s plan and keys', `error: ${e.message}`, false);
  }
  roundTwoChecks(rows, s, corpusRoot);

  for (const scenario of Object.values(s)) scenario.print(out);
  const failed = Object.values(s).filter((x) => x.counted && !x.ok).length;
  const pending = Object.values(s).filter((x) => x.pending !== null).length;
  const info = Object.values(s).filter((x) => x.info).length;
  out(`step2z: ${Object.keys(s).length - failed - pending - info} pass, ${failed} fail, ${pending} pending, ${info} info`);
  return failed === 0 ? 0 : 1;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  process.exitCode = run();
}
