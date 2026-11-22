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
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const DEFAULT_CORPUS = 'C:\\Users\\Adam\\clip-corpus';
const INSTANT_MS = 50;
const RAM_TOLERANCE = 0.10;
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
    this.nNew = lines.map((l) => JSON.parse(l)).filter((r) => r.new).length;
    this.dWait = fs.readdirSync(folder)
      .filter((f) => /^frame-\d{4}\.zst$/.test(f))
      .reduce((sum, f) => sum + fs.statSync(path.join(folder, f)).size, 0);
  }
}

const num = (v) => (v === undefined || v === '' ? NaN : Number(v));
const configKey = (r) => ['folder', 'way', 'level', 'layout', 'mode', 'tile', 'wlog', 'ldm', 'dict', 'workers', 'kz', 'theta', 'drain']
  .map((k) => r[k]).join('|');
const isIndependent = (r) => INDEPENDENT_WAYS.has(r.way);
const within = (known, measured, tol) => Math.abs(measured - known) <= tol * Math.abs(measured);

/** The per-row checks of one run: { scenario, known, measured, ok } or { scenario, note }. */
export function rowChecks(r, folder) {
  const px = num(r.w) * num(r.h);
  // A pilot run may cover only the first `frames_cap` new frames: then N_comp = that cap.
  const cap = num(r.frames_cap);
  const nKnown = Number.isFinite(cap) && cap > 0 ? Math.min(cap, folder.nNew) : folder.nNew;
  const out = [];
  const add = (scenario, known, measured, ok) => out.push({ scenario, known, measured, ok });
  add('frames', nKnown, num(r.frames), num(r.frames) === nKnown);
  add('bytes', `Σ out ${r.bytes_out}`, `on disk ${r.bytes_disk}`, num(r.bytes_out) === num(r.bytes_disk) && num(r.bytes_disk) > 0);
  if (r.way === 'ZW1' && num(r.level) === -3 && num(r.workers) === 0 && r.layout === 'bgr' && nKnown === folder.nNew) {
    if (num(r.drain || 0) === 0) {
      // The product's call (one-shot ZSTD_compress2, as zstd::bulk::compress): the row itself.
      add('baseline', folder.dWait, num(r.bytes_disk), num(r.bytes_disk) === folder.dWait);
    } else {
      // The corpus writer's call reproduced (ZSTD_compressStream2, output drained `drain` bytes
      // at a time, as Node zlib.zstdCompressSync): evidence for the cause of a miss, never a pass.
      out.push({ scenario: 'baseline', note: `evidence, drain=${r.drain} (corpus writer's call) ${r.folder} rep ${r.rep}: D_wait ${folder.dWait}, bytes ${r.bytes_disk}, ${num(r.bytes_disk) === folder.dWait ? 'equal' : 'differ'}` });
    }
  }
  const mComp = num(r.m0_private) + num(r.m_cctx_est) + num(r.bc) * px * 3;
  add('ram', Math.round(mComp), num(r.peak_private), within(mComp, num(r.peak_private), RAM_TOLERANCE));
  add('roundTrip', 0, num(r.diff_sum), num(r.diff_sum) === 0 && num(r.frames_decoded) === nKnown);
  const kz = r.kz === 'all' ? folder.nNew : num(r.kz);
  const independent = isIndependent(r);
  const seekOk = independent ? num(r.n_dec_worst) === 1 : num(r.n_dec_worst) >= 1 && num(r.n_dec_worst) <= kz;
  add('seek', independent ? 1 : `<= ${kz}`, num(r.n_dec_worst), seekOk);
  const mDec = num(r.m0_private) + num(r.m_dctx_est) + num(r.bd) * px * 3;
  add('ramDec', Math.round(mDec), num(r.dec_peak_private), within(mDec, num(r.dec_peak_private), RAM_TOLERANCE));
  const readyMs = num(r.t_ready_ns) / 1e6;
  add('ready', `<= ${INSTANT_MS} ms`, `${readyMs.toFixed(2)} ms`, readyMs <= INSTANT_MS);
  return out;
}

export { CorpusFolder };

/** One Gherkin scenario: its equation and a check per row. */
class Scenario {
  constructor(name, equation) {
    this.name = name;
    this.equation = equation;
    this.passed = 0;
    this.misses = [];
    this.notes = [];
    this.pending = null;
  }

  check(row, known, measured, ok) {
    if (ok) this.passed += 1;
    else this.misses.push(`${row.folder} ${row.way} L=${row.level} ${row.mode}/${row.layout} rep ${row.rep}: known ${known}, measured ${measured}`);
  }

  get ok() {
    return this.pending === null && this.misses.length === 0 && this.passed > 0;
  }

  print(out) {
    const state = this.pending !== null ? 'PENDING' : this.ok ? 'PASS' : 'FAIL';
    out(`${state}  ${this.name}: ${this.equation}  (${this.passed} rows pass, ${this.misses.length} miss)`);
    if (this.pending !== null) out(`        ${this.pending}`);
    if (this.passed === 0 && this.misses.length === 0 && this.pending === null) out('        no rows to check');
    for (const miss of this.misses.slice(0, 8)) out(`        miss: ${miss}`);
    if (this.misses.length > 8) out(`        ... ${this.misses.length - 8} more`);
    for (const note of this.notes.slice(0, 8)) out(`        ${note}`);
  }
}

export function run(argv = process.argv.slice(2), out = console.log) {
  const [csvPath, corpusRoot = DEFAULT_CORPUS] = argv;
  const { rows, error } = readRuns(csvPath);
  out(`step2z: ${rows.length} rows from ${csvPath ?? '(none)'}${error ? ` (${error})` : ''}`);
  const s = {
    frames: new Scenario('frames compressed', 'N_comp = N_new of the folder'),
    bytes: new Scenario('bytes', 'bytes = Σ output sizes = Σ clip file sizes on disk'),
    baseline: new Scenario('baseline equals the corpus', 'ZW1, L = -3, interleaved, independent: bytes = D_wait'),
    determinism: new Scenario('determinism', 'bytes(run 1) = bytes(run 2) = bytes(run 3)'),
    ram: new Scenario('RAM while compressing', 'M_comp = M_0 + M_cctx + B_c · px · 3 within ±10 % of peak private bytes'),
    cpu: new Scenario('CPU', 'C_comp = N_new · (c_0 + px · κ_c), held-out rows within ±10 %'),
    runtime: new Scenario('run time', 'full-run wall time within ±20 % of T (equation 6)'),
    roundTrip: new Scenario('round trip', 'Σ |decoded − corpus| = 0'),
    seek: new Scenario('seek cost', 'N_dec(worst) = 1 independent; <= K_z otherwise'),
    ramDec: new Scenario('decoder RAM', 'M_dec = M_0 + M_dctx + B_d · px · 3 within ±10 % of decode peak private bytes'),
    ready: new Scenario('time until ready', `t_ready = t_close <= ${INSTANT_MS} ms`),
  };
  s.cpu.pending = 'needs κ_c fitted on the full run (step 4); the pilot reports C_comp per frame only';
  s.runtime.pending = 'needs the full run; z-pilot.md gives T';

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

  for (const scenario of Object.values(s)) scenario.print(out);
  const failed = Object.values(s).filter((x) => x.pending === null && !x.ok).length;
  const pending = Object.values(s).filter((x) => x.pending !== null).length;
  out(`step2z: ${Object.keys(s).length - failed - pending} pass, ${failed} fail, ${pending} pending`);
  return failed === 0 ? 0 : 1;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  process.exitCode = run();
}
