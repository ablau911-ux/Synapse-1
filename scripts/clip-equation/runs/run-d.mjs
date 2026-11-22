// Experiments steps 2 and 3, way D: the runner of `clip-probe exp-d`
// (docs/clip-recorder-experiments-plan.md, "Run design": grid A-D and the D part of grid B).
//
//   node run-d.mjs grid=A|B|all [repeats=3] [csv=...]      the full grids (resumable)
//   node run-d.mjs grid=pilot [csv=results/d-pilot.csv]      typing and scroll at 1920 x 1080, q 30, 70, 100, 1 repeat
//   node run-d.mjs estimate [csv=results/d-pilot.csv]        T_D of the full grids from the pilot (equation 6 for way D)
//
// One probe process per run, at BelowNormal priority (inherited from this process).
// One CSV row per run with every option, the measured values, the known values and
// pass or fail. Rows already in the CSV are skipped, so a stopped run resumes.
// Bytes come from run 1; bytes_identical records whether a repeat gave the same file.
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const BASE = path.join(HERE, '..');
const PROBE = path.join(BASE, 'probe', 'target', 'release', 'clip-probe.exe');
const CORPUS = JSON.parse(fs.readFileSync(path.join(BASE, 'corpus', 'corpus.json'), 'utf8'));
const ENCODERS = JSON.parse(fs.readFileSync(path.join(BASE, 'results', 'encoders.json'), 'utf8'));
const SCRATCH = path.join(os.tmpdir(), 'clip-exp-d');

// The ways register (docs/clip-recorder-ways.md, section 3b): DW id by vendor of the H.264 encoder.
const WAY_BY_VENDOR = { VEN_8086: 'DW1', '': 'DW2', VEN_10DE: 'DW3', VEN_1414: 'DW4' };
const GRID_A = { q: [30, 40, 50, 60, 70, 80, 90, 100], tau: 20, hint: 1, rate: 'quality' };
const GRID_B = { way: 'DW1', q: 70, tau: [40, 100, 200], beta: [0.05, 0.10, 0.20] };
const PILOT = { folders: ['typing-1920x1080', 'scroll-1920x1080'], q: [30, 70, 100] };
const OCR_FRAMES = 10;
const TEXT_SCENARIOS = new Set(['typing', 'scroll', 'ui']);

const COLUMNS = ['grid', 'folder', 'scenario', 'width', 'height', 'sigma', 'out_w', 'out_h', 'encoder', 'encoder_name',
  'q', 'tau_f', 'hint', 'rate', 'beta', 'repeat', 'status', 'error', 'activated', 'hresult', 'dw4_system_nv12',
  'encoder_used', 'encoder_matches', 'n_new', 'frames', 'keyframes', 'keyframes_segment', 'b_frames', 'b_frames_read',
  'gop_read', 'max_gop', 'hint_accepted', 'rate_read', 'rate_value', 'bytes', 'clip_hash', 'bytes_identical',
  'waiting_bytes', 't_live_ns', 'a_rt_ns', 'm_live', 't_worker_ns', 'a_w_ns', 'm_worker', 'decoded_frames',
  'time_mismatch', 't_decode_ns', 'a_dec_ns', 'm_decode', 't_quality_ns', 't_ocr_ns', 'ssim_mean', 'ssim_min',
  'psnr_mean', 'psnr_min', 'psnr_global', 'identical_frames', 'r', 'r_note', 'flashes', 'wall_ms',
  'known_frames', 'known_keyframes', 'known_b', 'known_flashes', 'pass_frames', 'pass_keyframes', 'pass_b'];

const args = Object.fromEntries(process.argv.slice(2).map((a) => (a.includes('=') ? a.split(/=(.*)/s).slice(0, 2) : [a, '1'])));

/** Unique H.264 encoders of encoders.json by name and vendor, with their DW id. */
function encoders() {
  const seen = new Map();
  for (const e of ENCODERS.filter((x) => x.codec === 'H.264')) {
    const key = `${e.name}|${e.vendor_id}`;
    if (!seen.has(key)) seen.set(key, { way: WAY_BY_VENDOR[e.vendor_id], name: e.name.replace(/[^\x20-\x7E]/g, ''), vendor: e.vendor_id, hw: e.hardware ? 1 : 0 });
  }
  return [...seen.values()].filter((e) => e.way).sort((a, b) => a.way.localeCompare(b.way));
}

function folders() {
  const out = [];
  for (const scenario of Object.keys(CORPUS.scenarios)) {
    for (const s of CORPUS.sizes) out.push({ name: `${scenario}-${s.width}x${s.height}`, scenario, width: s.width, height: s.height, sigma: s.sigma });
  }
  return out;
}

function nNew(folder) {
  const text = fs.readFileSync(path.join(CORPUS.root, folder, 'frames.jsonl'), 'utf8');
  return text.split('\n').filter((l) => l.includes('"new":true')).length;
}

/** Every run of a grid: one object per configuration and repeat. */
function plan(grid, repeats) {
  const runs = [];
  const all = folders();
  const encs = encoders();
  const add = (g, f, e, o) => { for (let r = 1; r <= repeats; r += 1) runs.push({ grid: g, folder: f.name, enc: e, repeat: r, ...o }); };
  if (grid === 'pilot') {
    for (const f of all.filter((x) => PILOT.folders.includes(x.name))) for (const e of encs) for (const q of PILOT.q) add('pilot', f, e, { q, tau: 20, hint: 1, rate: 'quality', beta: 0 });
  }
  if (grid === 'A' || grid === 'all') {
    for (const f of all) for (const e of encs) for (const q of GRID_A.q) add('A', f, e, { q, tau: GRID_A.tau, hint: GRID_A.hint, rate: GRID_A.rate, beta: 0 });
  }
  if (grid === 'B' || grid === 'all') {
    const e = encs.find((x) => x.way === GRID_B.way);
    for (const f of all) {
      for (const tau of GRID_B.tau) add('B-tau', f, e, { q: GRID_B.q, tau, hint: 1, rate: 'quality', beta: 0 });
      add('B-hint', f, e, { q: GRID_B.q, tau: 20, hint: 0, rate: 'quality', beta: 0 });
      for (const beta of GRID_B.beta) add('B-beta', f, e, { q: 0, tau: 20, hint: 1, rate: 'bitrate', beta });
    }
  }
  return runs;
}

const keyOf = (r) => [r.grid, r.folder, r.encoder ?? r.enc.way, r.q, r.tau_f ?? r.tau, r.hint, r.rate, r.beta, r.repeat].join('|');

function readCsv(csv) {
  if (!fs.existsSync(csv)) return [];
  const lines = fs.readFileSync(csv, 'utf8').split(/\r?\n/).filter((l) => l.trim());
  const head = lines[0].split(',');
  return lines.slice(1).map((l) => Object.fromEntries(l.split(',').map((v, i) => [head[i], v])));
}

const clean = (v) => (v === undefined || v === null ? '' : String(v).replace(/[,\r\n]/g, ';'));

function runOne(run, cfg) {
  const scratch = path.join(SCRATCH, `${process.pid}-${Date.now()}`);
  const flash = CORPUS.flash;
  const argv = ['exp-d', `folder=${path.join(CORPUS.root, run.folder)}`, `encoder=${run.enc.way}`, `name=${run.enc.name}`,
    `vendor=${run.enc.vendor}`, `hw=${run.enc.hw}`, `q=${run.q}`, `gop=${run.tau}`, `hint=${run.hint}`, `rate=${run.rate}`,
    `beta=${run.beta}`, `scratch=${scratch}`, `repeat=${run.repeat}`, `window_width=${cfg.width}`,
    `flash=${flash.bgr.join(':')}:${flash.box_left_fraction}:${flash.box_top}:${flash.box_width}:${flash.box_height}:${flash.min_pixels}`];
  const start = Date.now();
  const res = spawnSync(PROBE, argv, { encoding: 'utf8', windowsHide: true, maxBuffer: 1 << 24 });
  const wall = Date.now() - start;
  const line = (res.stdout || '').split(/\r?\n/).find((l) => l.startsWith('{'));
  let out;
  try { out = line ? JSON.parse(line) : { status: 'error', error: `no JSON, exit ${res.status}: ${(res.stderr || '').trim().slice(0, 300)}` }; }
  catch (e) { out = { status: 'error', error: `bad JSON: ${e.message}` }; }
  fs.rmSync(scratch, { recursive: true, force: true });
  return { ...out, wall_ms: wall };
}

function row(run, cfg, out, first) {
  const n = nNew(run.folder);
  const ok = out.status === 'ok';
  const known = { known_frames: n, known_keyframes: ok ? Math.ceil(Number(out.frames) / run.tau) : '', known_b: 0, known_flashes: cfg.scenario === 'flash' ? CORPUS.t_s * CORPUS.flash.per_second : '' };
  const r = {
    ...out, grid: run.grid, folder: run.folder, scenario: cfg.scenario, encoder: run.enc.way, encoder_name: run.enc.name,
    q: run.q, tau_f: run.tau, hint: run.hint, rate: run.rate, beta: run.beta, repeat: run.repeat, ...known,
    bytes_identical: first ? (first.clip_hash === out.clip_hash ? 1 : 0) : (ok ? 1 : ''),
    pass_frames: ok ? (Number(out.frames) === n ? 1 : 0) : '',
    pass_keyframes: ok ? (Number(out.keyframes) === known.known_keyframes ? 1 : 0) : '',
    pass_b: ok ? (Number(out.b_frames) === 0 ? 1 : 0) : '',
  };
  return COLUMNS.map((c) => clean(r[c])).join(',');
}

function runGrid(grid) {
  try { os.setPriority(0, os.constants.priority.PRIORITY_BELOW_NORMAL); } catch { /* keeps normal priority */ }
  const repeats = grid === 'pilot' ? 1 : Number(args.repeats ?? 3);
  const csv = path.resolve(args.csv ?? path.join(BASE, 'results', grid === 'pilot' ? 'd-pilot.csv' : 'd-runs.csv'));
  if (!fs.existsSync(csv)) fs.writeFileSync(csv, `${COLUMNS.join(',')}\n`);
  const done = readCsv(csv);
  const doneKeys = new Set(done.map(keyOf));
  const firsts = new Map(done.filter((r) => r.repeat === '1').map((r) => [keyOf({ ...r, repeat: 1 }), r]));
  const cfgs = Object.fromEntries(folders().map((f) => [f.name, f]));
  const runs = plan(grid, repeats);
  console.log(`exp-d ${grid}: ${runs.length} runs, ${runs.filter((r) => doneKeys.has(keyOf(r))).length} already in ${csv}`);
  let count = 0;
  for (const run of runs) {
    if (doneKeys.has(keyOf(run))) continue;
    const cfg = cfgs[run.folder];
    const out = runOne(run, cfg);
    const firstKey = keyOf({ ...run, repeat: 1 });
    const first = run.repeat > 1 ? firsts.get(firstKey) : null;
    fs.appendFileSync(csv, `${row(run, cfg, out, first)}\n`);
    if (run.repeat === 1) firsts.set(firstKey, out);
    count += 1;
    console.log(`${count} ${run.folder} ${run.enc.way} q${run.q} tau${run.tau} hint${run.hint} ${run.rate} ${run.beta} r${run.repeat}: ${out.status} ${out.bytes ?? ''} B ${out.wall_ms} ms${out.error ? ` (${out.error.slice(0, 120)})` : ''}`);
  }
}

/** T_D of the full D grids (3 repeats) from the pilot rows, by fitted time per pixel. */
function estimate() {
  const csv = path.resolve(args.csv ?? path.join(BASE, 'results', 'd-pilot.csv'));
  const rows = readCsv(csv);
  const ok = rows.filter((r) => r.status === 'ok');
  const per = (rs, f, px) => rs.reduce((s, r) => s + f(r), 0) / rs.reduce((s, r) => s + px(r), 0);
  const pxIn = (r) => Number(r.n_new) * Number(r.width) * Number(r.height);
  const pxOut = (r) => Number(r.n_new) * Number(r.out_w) * Number(r.out_h);
  const ns = (k) => (r) => Number(r[k]);
  const coef = {
    live: per(ok, ns('t_live_ns'), pxIn),
    decode: per(ok, ns('t_decode_ns'), pxOut),
    quality: per(ok, ns('t_quality_ns'), pxOut),
    ocr: ok.filter((r) => TEXT_SCENARIOS.has(r.scenario)).reduce((s, r) => s + Number(r.t_ocr_ns), 0)
      / ok.filter((r) => TEXT_SCENARIOS.has(r.scenario)).reduce((s, r) => s + OCR_FRAMES * Number(r.out_w) * Number(r.out_h), 0),
    worker: {},
    overhead_ns: ok.reduce((s, r) => s + Number(r.wall_ms) * 1e6 - ['t_live_ns', 't_worker_ns', 't_decode_ns', 't_quality_ns', 't_ocr_ns'].reduce((a, k) => a + Number(r[k]), 0), 0) / Math.max(ok.length, 1),
  };
  for (const way of new Set(ok.map((r) => r.encoder))) coef.worker[way] = per(ok.filter((r) => r.encoder === way), ns('t_worker_ns'), pxOut);
  const failRows = rows.filter((r) => r.status !== 'ok');
  const failWall = failRows.length ? failRows.reduce((s, r) => s + Number(r.wall_ms), 0) / failRows.length * 1e6 : 0;
  const repeats = Number(args.repeats ?? 3);
  const parts = { live: 0, worker: 0, decode: 0, quality: 0, ocr: 0, overhead: 0, failed: 0 };
  const byWay = {};
  const sizes = Object.fromEntries(folders().map((f) => [f.name, f]));
  for (const run of [...plan('A', repeats), ...plan('B', repeats)]) {
    const f = sizes[run.folder];
    const n = nNew(run.folder);
    const outPx = (f.width * f.sigma) * (f.height * f.sigma);
    if (!(run.enc.way in coef.worker)) { parts.failed += failWall; continue; }
    const t = {
      live: n * f.width * f.height * coef.live,
      worker: n * outPx * coef.worker[run.enc.way],
      decode: n * outPx * coef.decode,
      quality: n * outPx * coef.quality,
      ocr: TEXT_SCENARIOS.has(f.scenario) ? OCR_FRAMES * outPx * coef.ocr : 0,
      overhead: coef.overhead_ns,
    };
    for (const k of Object.keys(t)) parts[k] += t[k];
    byWay[run.enc.way] = (byWay[run.enc.way] ?? 0) + Object.values(t).reduce((a, b) => a + b, 0);
  }
  const total = Object.values(parts).reduce((a, b) => a + b, 0);
  const h = (v) => (v / 3.6e12).toFixed(2);
  console.log(JSON.stringify({
    runs: plan('A', repeats).length + plan('B', repeats).length,
    coef_ns_per_px: { live: coef.live, decode: coef.decode, quality: coef.quality, ocr_per_frame_px: coef.ocr, worker: coef.worker },
    overhead_ms_per_run: coef.overhead_ns / 1e6,
    failed_run_ms: failWall / 1e6,
    hours: Object.fromEntries([...Object.entries(parts).map(([k, v]) => [k, h(v)]), ['total', h(total)]]),
    hours_by_encoder: Object.fromEntries(Object.entries(byWay).map(([k, v]) => [k, h(v)])),
  }, null, 1));
}

if (args.estimate) estimate();
else runGrid(args.grid ?? 'pilot');
