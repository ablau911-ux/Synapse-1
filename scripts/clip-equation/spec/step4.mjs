// Experiments step 4: the checks of scripts/clip-equation/spec/step4.feature.
// Spec: docs/clip-recorder-experiments-plan.md, "Equations" 1 to 7, "How absolute and predictable
// is reached" 3, "Run design" (hold-out), "Acceptance (BDD)" step 4.
//
//   node run-spec.mjs step4 [results/fit.json]
//
// The fitter is proven first on synthetic data made exactly from known coefficients. Then the
// fit file written by fit/fit.mjs is checked against its input CSVs, which this check reads
// itself: row counts, the round trip, t_ready, q_min, the D hold-out draw (mulberry32, redone
// here) and that no held-out configuration is in a fit set. A part that needs data the input
// does not have (two sizes, a tau_f sweep, a complete run) is PENDING, and only when the data
// really lacks it; otherwise a missing coefficient is a miss.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { readRuns } from './step2d.mjs';
import { PENDING, relErr, runFeature } from './feature.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const BASE = path.resolve(HERE, '..');
const FEATURE = path.join(HERE, 'step4.feature');
export const CONFIG = JSON.parse(fs.readFileSync(path.join(BASE, 'fit', 'fit-config.json'), 'utf8'));
const FIT_MODULE = path.join(BASE, 'fit', 'fit.mjs');

export const keyOf = (kind, row) => `${kind}|${CONFIG[kind === 'Z' ? 'z_key_columns' : 'd_key_columns'].map((c) => row[c] ?? '').join('|')}`;
const finite = (v) => typeof v === 'number' && Number.isFinite(v);
const sizeOf = (folder) => folder.slice(folder.indexOf('-') + 1);
const contentOf = (folder) => folder.slice(0, folder.indexOf('-'));

/** Every number inside a coefficient object, flattened. */
function numbers(obj) {
  if (typeof obj === 'number') return [obj];
  if (obj && typeof obj === 'object') return Object.values(obj).flatMap(numbers);
  return [];
}

/** The input CSV rows named in fit.json, read here (not from fit.json). */
export function loadInputs(fit) {
  const read = (list) => list.flatMap((i) => readRuns(i.path).rows);
  return { z: read(fit.inputs.z), d: read(fit.inputs.d) };
}

/** mulberry32, as in runs/run-z.mjs: the same seed gives the same draw. */
function mulberry32(seed) {
  let s = seed >>> 0;
  return () => {
    s = (s + 0x6d2b79f5) >>> 0;
    let t = s;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/** The D hold-out redone here: eligible keys sorted, shuffled by mulberry32, the first share taken. */
export function drawD(rows) {
  const base = CONFIG.d_base;
  const eligible = [...new Set(rows.filter((r) => r.status === 'ok' && r.tau_f === base.tau_f && r.hint === base.hint && r.rate === base.rate).map((r) => keyOf('D', r)))].sort();
  const random = mulberry32(CONFIG.hold_out.seed);
  const shuffled = eligible.map((k) => [random(), k]).sort((a, b) => a[0] - b[0]).map(([, k]) => k);
  return { eligible, held: shuffled.slice(0, Math.round(CONFIG.hold_out.share * eligible.length)) };
}

/** Synthetic data from known coefficients, fitted back. */
function selfTests(F) {
  const tol = CONFIG.self_test_tolerance;
  const sizes = [506 * 333, 640 * 360, 1280 * 720, 1920 * 1080];
  const qs = [30, 40, 50, 60, 70, 80, 90, 100];
  const worst = (pairs) => Math.max(...pairs.map(([fit, truth]) => relErr(fit, truth)));
  const out = (pairs, label) => {
    const w = worst(pairs);
    return { value: `${pairs.length} coefficients`, measured: `${label}; worst relative error ${w.toExponential(2)}`, pass: Number.isFinite(w) && w <= tol };
  };
  return {
    power() {
      const zeta0 = { a: 0.05, b: 0.12, c: 0.3 };
      const eta = 0.93;
      const pts = Object.entries(zeta0).flatMap(([g, z]) => sizes.map((px) => {
        const x = Math.log(px / CONFIG.px0);
        return { g, x, y: Math.log(z) + (eta - 1) * x };
      }));
      const fit = F.fitSharedSlope(pts);
      return out([[fit.slope + 1, eta], ...Object.entries(zeta0).map(([g, z]) => [Math.exp(fit.intercept[g]), z])], `eta ${(fit.slope + 1).toFixed(9)}`);
    },
    cpu() {
      const c0 = 120000;
      const kappa = { a: 3.1, b: 5.7, c: 11.2 };
      const pts = Object.entries(kappa).flatMap(([g, k]) => sizes.map((px) => ({ g, x: px, y: c0 + k * px })));
      const fit = F.fitSharedIntercept(pts);
      return out([[fit.c0, c0], ...Object.entries(kappa).map(([g, k]) => [fit.slope[g], k])], `c_0 ${fit.c0.toFixed(3)} ns`);
    },
    d() {
      const pairs = [];
      const p = { typing: 2e-3, scroll: 5e-3 };
      const a = 0.031;
      const fp = F.fitSharedSlope(Object.entries(p).flatMap(([g, v]) => qs.map((q) => ({ g, x: q, y: Math.log(v) + a * q }))));
      pairs.push([fp.slope, a], ...Object.entries(p).map(([g, v]) => [Math.exp(fp.intercept[g]), v]));
      const s = { typing: 0.2, scroll: 0.35 };
      const g0 = 0.045;
      const fs2 = F.fitSharedSlope(Object.entries(s).flatMap(([g, v]) => qs.map((q) => ({ g, x: q, y: Math.log(v * Math.exp(-g0 * q)) }))));
      pairs.push([-fs2.slope, g0], ...Object.entries(s).map(([g, v]) => [Math.exp(fs2.intercept[g]), v]));
      const m = { typing: 30, scroll: 34 };
      const l = 0.21;
      const fq = F.fitSharedSlope(Object.entries(m).flatMap(([g, v]) => qs.map((q) => ({ g, x: q, y: v + l * q }))));
      pairs.push([fq.slope, l], ...Object.entries(m).map(([g, v]) => [fq.intercept[g], v]));
      const [e0, A, B] = [5e5, 4.2, 0.013];
      const rows = sizes.flatMap((px) => qs.map((q) => [1, px, px * q]));
      const fe = F.leastSquares(rows, rows.map(([, px, pq]) => e0 + A * px + B * pq));
      pairs.push([fe.beta[0], e0], [fe.beta[1], A], [fe.beta[2], B]);
      const [m0, m1] = [3e7, 12.5];
      const fm = F.leastSquares(sizes.map((px) => [1, px]), sizes.map((px) => m0 + m1 * px));
      pairs.push([fm.beta[0], m0], [fm.beta[1], m1]);
      return out(pairs, 'P, Q_ssim, Q_psnr, c_enc, M_enc');
    },
  };
}

/** A fitted equation entry: coefficients finite, residuals listed, every pending part justified by the data. */
function equation(fit, id, justify) {
  const e = fit.equations?.[id];
  if (!e) return { value: '-', measured: `no equation ${id} in fit.json`, pass: false };
  const nums = numbers(e.coefficients);
  const bad = nums.filter((v) => !Number.isFinite(v)).length;
  const pending = e.pending ?? [];
  const unjustified = pending.filter((p) => !justify(p.part));
  const fitted = nums.length > 0 && bad === 0 && (e.residuals?.n ?? 0) > 0;
  const res = e.residuals ? `residuals n ${e.residuals.n}, max ${(100 * e.residuals.max_abs_rel).toFixed(2)} %, rms ${(100 * e.residuals.rms_rel).toFixed(2)} %` : 'no residuals';
  const measured = `${nums.length} coefficients (${bad} not finite); ${res}${pending.length ? `; pending: full run: ${pending.map((p) => `${p.part} (${p.reason})`).join('; ')}` : ''}${unjustified.length ? `; NOT JUSTIFIED: ${unjustified.map((p) => p.part).join(', ')}` : ''}`;
  if (unjustified.length || bad) return { value: e.equation, measured, pass: false };
  if (!fitted) return { value: e.equation, measured, pass: pending.length ? PENDING : false };
  return { value: e.equation, measured, pass: pending.length ? PENDING : true };
}

/** The validation table of one way: verdicts consistent with the tolerances, held-out rows outside the fit. */
function validation(fit, kind, heldKeys, quantities) {
  const v = fit.validation?.[kind.toLowerCase()];
  if (!v) return { ok: false, text: `no validation.${kind.toLowerCase()} in fit.json` };
  const configs = new Map(fit.configurations.map((c) => [c.key, c]));
  const problems = [];
  let within = 0;
  let notPredictable = 0;
  let pending = 0;
  for (const key of heldKeys) {
    if (configs.get(key)?.in_fit !== false) problems.push(`${key} in the fit set`);
    for (const q of quantities) {
      const row = v.rows.find((r) => r.key === key && r.quantity === q);
      if (!row) { problems.push(`${key} ${q}: no prediction`); continue; }
      const tol = CONFIG.tolerance[q];
      const err = q === 'ssim' ? Math.abs(row.predicted - row.measured) : relErr(row.predicted, row.measured);
      if (row.verdict === 'within') { within += 1; if (!(err <= tol)) problems.push(`${key} ${q}: within but error ${err}`); }
      else if (row.verdict === 'not predictable') { notPredictable += 1; if (!(err > tol) || row.spread === undefined) problems.push(`${key} ${q}: not predictable without a miss or spread`); }
      else if (String(row.verdict).startsWith('pending')) pending += 1;
      else problems.push(`${key} ${q}: verdict ${row.verdict}`);
    }
  }
  return { ok: problems.length === 0, pending, text: `${heldKeys.length} held out; ${within} within, ${notPredictable} not predictable, ${pending} pending${problems.length ? `; misses: ${problems.slice(0, 4).join('; ')}` : ''}` };
}

function checks(fit, F, fitError, inputs) {
  const need = (fn) => () => (fit ? fn() : { value: '-', measured: fitError, pass: false });
  const needF = (fn) => () => (F ? fn() : { value: '-', measured: `no fitter: ${fitError}`, pass: false });
  const t = F ? selfTests(F) : null;
  const inFit = (kind) => (fit?.configurations ?? []).filter((c) => c.kind === kind && c.in_fit);
  const groupsWithSizes = () => {
    const g = new Map();
    for (const c of inFit('Z')) { const k = `${c.key.replace(/^Z\|[^|]*/, '')}|${c.content}`; g.set(k, (g.get(k) ?? new Set()).add(c.size)); }
    return [...g.values()].some((s) => s.size >= 2);
  };
  const dPxCount = () => new Set(inFit('D').map((c) => c.px)).size;
  const tauSweep = () => {
    const g = new Map();
    for (const c of inFit('D')) { const k = `${c.options.encoder}|${c.folder}|${c.options.q}`; g.set(k, (g.get(k) ?? new Set()).add(c.options.tau_f)); }
    return [...g.values()].some((s) => s.size >= 2);
  };
  return {
    'fitter self-test: power law in px': needF(() => t.power()),
    'fitter self-test: px-linear CPU': needF(() => t.cpu()),
    'fitter self-test: D curves': needF(() => t.d()),
    'fit file': need(() => {
      const bad = [];
      for (const i of [...fit.inputs.z, ...fit.inputs.d]) {
        const n = readRuns(i.path).rows.length;
        if (n !== i.rows) bad.push(`${path.basename(i.path)} ${i.rows} != ${n} now`);
      }
      const pilot = [...fit.inputs.z, ...fit.inputs.d].some((i) => /pilot/i.test(path.basename(i.path)));
      const labelOk = pilot === String(fit.label).startsWith('PILOT');
      return { value: 'rows equal', measured: `${[...fit.inputs.z, ...fit.inputs.d].map((i) => `${path.basename(i.path)} ${i.rows}`).join(', ')}; label "${fit.label}"${bad.length ? `; changed: ${bad.join('; ')}` : ''}`, pass: bad.length === 0 && labelOk };
    }),
    'eq 1 Z bytes': need(() => equation(fit, 'eq1_z_bytes', (p) => p === 'eta' && !groupsWithSizes())),
    'eq 2 Z quality': need(() => {
      const bad = inputs.z.filter((r) => r.diff_sum !== '0');
      const listed = fit.equations?.eq2_z_quality?.coefficients?.runs_exact;
      return { value: 'diff_sum = 0', measured: `${inputs.z.length - bad.length}/${inputs.z.length} runs exact; fit.json says ${listed}`, pass: inputs.z.length > 0 && bad.length === 0 && listed === inputs.z.length };
    }),
    'eq 3 Z CPU': need(() => equation(fit, 'eq3_z_cpu', (p) => p === 'c_0' && !groupsWithSizes())),
    'eq 4 Z RAM': need(() => equation(fit, 'eq4_z_ram', () => false)),
    'eq 5 Z time until ready': need(() => {
      const limit = CONFIG.t_ready_limit_ms * 1e6;
      const bad = inputs.z.filter((r) => !(Number(r.t_ready_ns) <= limit));
      return { value: `<= ${CONFIG.t_ready_limit_ms} ms`, measured: `${inputs.z.length - bad.length}/${inputs.z.length} runs; max ${(Math.max(...inputs.z.map((r) => Number(r.t_ready_ns))) / 1e6).toFixed(2)} ms`, pass: inputs.z.length > 0 && bad.length === 0 };
    }),
    'eq 6 run time': need(() => {
      const e = fit.equations?.eq6_run_time;
      if (!e) return { value: '-', measured: 'no eq6_run_time in fit.json', pass: false };
      const runsPlanned = e.coefficients?.runs_planned;
      if (!(inputs.z.length >= runsPlanned)) return { value: `T_total ${e.coefficients?.T_total_h?.toFixed(2)} h`, measured: `pending: full run (${inputs.z.length} of ${runsPlanned} planned Z runs)`, pass: PENDING };
      const err = relErr(e.coefficients.T_measured_h, e.coefficients.T_total_h);
      return { value: `T_total ${e.coefficients.T_total_h.toFixed(2)} h`, measured: `wall ${e.coefficients.T_measured_h.toFixed(2)} h (${(100 * err).toFixed(1)} %)`, pass: err <= CONFIG.run_time_tolerance };
    }),
    'eq 7 D bytes': need(() => equation(fit, 'eq7_d_bytes', (p) => p === 'K/P' && !tauSweep())),
    'eq 7 D quality': need(() => equation(fit, 'eq7_d_quality', () => false)),
    'eq 7 D q_min': need(() => {
      const e = fit.equations?.eq7_d_qmin;
      if (!e) return { value: '-', measured: 'no eq7_d_qmin in fit.json', pass: false };
      const text = new Set(CONFIG.text_scenarios);
      const groups = new Map();
      for (const r of inputs.d.filter((x) => x.status === 'ok' && text.has(x.scenario))) {
        const g = `${r.encoder} ${sizeOf(r.folder)} tau${r.tau_f} hint${r.hint} ${r.rate} beta${r.beta}`;
        const byQ = groups.get(g) ?? new Map();
        const cell = byQ.get(r.q) ?? new Map();
        cell.set(r.scenario, Math.min(cell.get(r.scenario) ?? 1, Number(r.r)));
        byQ.set(r.q, cell);
        groups.set(g, byQ);
      }
      const bad = [];
      for (const [g, byQ] of groups) {
        const ok = [...byQ].filter(([, cell]) => CONFIG.text_scenarios.every((s) => cell.get(s) === 1)).map(([q]) => Number(q));
        const failed = [...byQ].filter(([, cell]) => [...cell.values()].some((r) => r !== 1)).length;
        const want = ok.length ? Math.min(...ok) : failed === byQ.size ? 'not reached' : 'pending: full run';
        if (String(e.coefficients?.[g]) !== String(want)) bad.push(`${g}: ${e.coefficients?.[g]} != ${want}`);
      }
      const values = Object.entries(e.coefficients ?? {}).map(([g, v]) => `${g}: ${v}`).join('; ');
      return { value: 'recomputed from the CSV', measured: `${groups.size} groups; ${values}${bad.length ? `; misses: ${bad.join('; ')}` : ''}`, pass: groups.size > 0 && bad.length === 0 };
    }),
    'eq 7 D CPU': need(() => equation(fit, 'eq7_d_cpu', (p) => dPxCount() < 2 && /^(c_comp|c_file|e0|e1|e2|c_enc)/.test(p))),
    'eq 7 D RAM': need(() => equation(fit, 'eq7_d_ram', (p) => dPxCount() < 2 && /^(M_live|m0|m1|M_enc)/.test(p))),
    'Z validation': need(() => {
      const held = [...new Set(inputs.z.filter((r) => r.held_out === '1').map((r) => keyOf('Z', r)))];
      if (!held.length) return { value: 'within 10 %', measured: `pending: full run (0 held-out Z rows in the input; held-out cells in the run list: ${fit.validation?.z?.runlist_held_out ?? '?'})`, pass: PENDING };
      const v = validation(fit, 'Z', held, ['bytes', 'cpu', 'ram']);
      return { value: 'within 10 %', measured: v.text, pass: v.ok ? (v.pending ? PENDING : true) : false };
    }),
    'D validation': need(() => {
      const { eligible, held } = drawD(inputs.d);
      const listed = fit.validation?.d?.held_out_keys ?? [];
      const same = held.length === listed.length && held.every((k) => listed.includes(k));
      if (!eligible.length) return { value: 'mulberry32 20261122', measured: 'no eligible D configuration', pass: false };
      const v = validation(fit, 'D', held, ['bytes', 'cpu', 'ram', 'ssim']);
      return { value: `${held.length} of ${eligible.length} (seed ${CONFIG.hold_out.seed})`, measured: `draw ${same ? 'reproduced' : `differs: fit.json ${listed.length}`}; ${v.text}`, pass: same && v.ok ? (v.pending ? PENDING : true) : false };
    }),
  };
}

/** Runs every scenario of step4.feature; returns the exit code. */
export async function main(fitPath = path.join(BASE, CONFIG.fit_out)) {
  let F = null;
  let fit = null;
  let fitError = '';
  try { F = await import(pathToFileURL(FIT_MODULE).href); } catch (err) { fitError = `${path.relative(BASE, FIT_MODULE)}: ${err.message.split('\n')[0]}`; }
  if (fs.existsSync(fitPath)) fit = JSON.parse(fs.readFileSync(fitPath, 'utf8'));
  else fitError = `${fitError ? `${fitError}; ` : ''}no fit file at ${fitPath}`;
  const inputs = fit ? loadInputs(fit) : { z: [], d: [] };
  console.log(`step4: fit ${fitPath}${fit ? ` (${fit.label}; z rows ${inputs.z.length}, d rows ${inputs.d.length})` : ''}`);
  return runFeature('step4', FEATURE, checks(fit, F, fitError || 'no fit file', inputs));
}

export { contentOf, sizeOf };
