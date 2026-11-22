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
import { D_BYTES_TRUTH, RAM_TRUTH, dBytesConfigs, ramFixture, sizeFixture, targetFixture, zBytesPerFrame } from './step4.fixtures.mjs';

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

/**
 * The D configurations whose every repeat succeeded and, on a text scenario, read R (a number, or
 * n/a on every repeat): plan M10, "a configuration needs all of its repeats to succeed".
 */
export function completeD(rows) {
  const groups = new Map();
  for (const r of rows) groups.set(keyOf('D', r), [...(groups.get(keyOf('D', r)) ?? []), r]);
  const text = new Set(CONFIG.text_scenarios);
  const isNa = (r) => String(r.r ?? '').trim().toLowerCase() === 'n/a';
  return new Set([...groups].filter(([, rs]) => rs.every((r) => r.status === 'ok')
    && (!text.has(rs[0].scenario) || rs.every(isNa) || rs.every((r) => r.r !== '' && Number.isFinite(Number(r.r))))).map(([k]) => k));
}

/** The D hold-out redone here: complete base keys sorted, shuffled by mulberry32, the first share taken. */
export function drawD(rows) {
  const base = CONFIG.d_base;
  const complete = completeD(rows);
  const eligible = [...new Set(rows.filter((r) => complete.has(keyOf('D', r)) && r.tau_f === base.tau_f && r.hint === base.hint && r.rate === base.rate).map((r) => keyOf('D', r)))].sort();
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

/** The rows of issue #295 (#8, #10, #15 to #18, #20, M7), on synthetic fixtures with known results. */
function findingChecks(F, inputs) {
  const need = (fn) => () => (F?.buildFitFromRows ? fn() : { value: '-', measured: 'fit.mjs has no buildFitFromRows', pass: false });
  const noRunlist = { cells: [], contents: [] };
  const target = () => F.buildFitFromRows({ zRows: [], dRows: targetFixture(), runlist: noRunlist });
  const cfg = (f, scenario, q) => f.configurations.find((c) => c.kind === 'D' && c.content === scenario && c.options.q === String(q));
  return {
    '#8 failed repeats, missing R and refused settings block a configuration': need(() => {
      const f = target();
      const bad = [];
      for (const [s, q, failed] of [['typing', 70, 1], ['scroll', 70, 0], ['ui', 70, 3]]) {
        const c = cfg(f, s, q);
        if (!c || c.O !== false || c.in_fit !== false || c.failed_runs !== failed) bad.push(`${s} q${q}: O ${c?.O}, in_fit ${c?.in_fit}, failed_runs ${c?.failed_runs}`);
      }
      if (!cfg(f, 'typing', 50)?.O) bad.push('typing q50 not kept');
      const n = f.failures?.d;
      if (!n || n.runs_failed !== 4 || n.configs_blocked !== 3) bad.push(`failure counts ${JSON.stringify(n)}`);
      return { value: 'typing, scroll, ui at q 70 dropped and out of the fit; 4 failed runs, 3 blocked', measured: bad.length ? bad.join('; ') : 'as known', pass: bad.length === 0 };
    }),
    '#10 FULL only when the unique keys match both run plans': need(() => {
      const { zRows, runlist } = sizeFixture();
      const dRows = targetFixture();
      const dRunlist = { runs: dRows.map((r) => Object.fromEntries([...CONFIG.d_key_columns, 'repeat'].map((k) => [k, r[k]]))) };
      const label = (z, d, dl) => F.fitLabel({ pilot: false, zRows: z, dRows: d, runlist, dRunlist: dl }).label;
      const cases = [
        ['complete', label(zRows, dRows, dRunlist), true],
        ['Z duplicate in place of a repeat', label([...zRows.slice(0, -1), { ...zRows[0] }], dRows, dRunlist), false],
        ['one D run missing', label(zRows, dRows.slice(1), dRunlist), false],
        ['no D run plan', label(zRows, dRows, null), false],
      ];
      const bad = cases.filter(([, got, full]) => full !== (got === 'FULL'));
      return { value: cases.map(([n, , full]) => `${n}: ${full ? 'FULL' : 'not FULL'}`).join('; '), measured: cases.map(([n, got]) => `${n}: ${got}`).join('; '), pass: bad.length === 0 };
    }),
    '#15 a non-text row passes O only where every text scenario passes at its q': need(() => {
      const f = target();
      const a = cfg(f, 'flash', 50);
      const b = cfg(f, 'flash', 60);
      const say = (c) => (c?.O ? 'kept' : `dropped (${c?.clause})`);
      return { value: 'flash q50 kept, flash q60 dropped', measured: `flash q50 ${say(a)}; flash q60 ${say(b)}`, pass: a?.O === true && b?.O === false };
    }),
    'M7 PSNR is inf at MSE 0': need(() => {
      const f = target();
      const a = cfg(f, 'flash', 50);
      const nums = numbers(f.equations.eq7_d_quality.coefficients);
      const bad = nums.filter((v) => !Number.isFinite(v)).length;
      return { value: 'PSNR inf; quality coefficients finite', measured: `flash q50 PSNR ${a?.psnr}; ${nums.length} quality coefficients, ${bad} not finite`, pass: a?.psnr === 'inf' && nums.length > 0 && bad === 0 };
    }),
    '#16 held-out rows never change a coefficient': need(() => {
      if (!inputs.z.length || !inputs.d.length) return { value: '-', measured: 'needs the input CSVs of fit.json', pass: false };
      const zRows = inputs.z.map((r) => ({ ...r }));
      const zKey = keyOf('Z', zRows[0]);
      for (const r of zRows) if (keyOf('Z', r) === zKey) r.held_out = '1';
      const base = { zRows, dRows: inputs.d.map((r) => ({ ...r })), runlist: noRunlist };
      const a = F.buildFitFromRows(base);
      const heldD = new Set(a.validation.d.held_out_keys);
      const x = (v) => String(Number(v) * 1.7);
      const b = F.buildFitFromRows({
        ...base,
        zRows: zRows.map((r) => (keyOf('Z', r) !== zKey ? r : { ...r, bytes_out: x(r.bytes_out), comp_cpu_ns: x(r.comp_cpu_ns), peak_private: x(r.peak_private), m0_private: x(r.m0_private), known_m_comp: x(r.known_m_comp), dec_cpu_ns: x(r.dec_cpu_ns) })),
        dRows: base.dRows.map((r) => (!heldD.has(keyOf('D', r)) ? r : { ...r, bytes: x(r.bytes), a_rt_ns: x(r.a_rt_ns), a_w_ns: x(r.a_w_ns), m_live: x(r.m_live), m_worker: x(r.m_worker), ssim_mean: '0.5', psnr_mean: '10', psnr_global: '10', r: r.r === '1' ? '0.5' : '1' })),
      });
      const ser = (e) => JSON.stringify(e, (k, v) => (typeof v === 'number' && !Number.isFinite(v) ? String(v) : v));
      const changed = Object.keys(a.equations).filter((id) => ser(a.equations[id]) !== ser(b.equations[id]));
      return { value: 'every equation identical', measured: `1 Z and ${heldD.size} D held-out configurations changed; equations changed: ${changed.join(', ') || 'none'}`, pass: heldD.size > 0 && changed.length === 0 };
    }),
    '#17 size groups follow the run list cell, not the resolved window': need(() => {
      const { zRows, runlist } = sizeFixture();
      const f = F.buildFitFromRows({ zRows, dRows: [], runlist });
      const rows = f.validation.z.rows.filter((v) => v.quantity === 'bytes');
      const truth = zBytesPerFrame(1920 * 1080);
      const ok = rows.length === 1 && Number.isFinite(rows[0].predicted) && relErr(rows[0].predicted, truth) <= CONFIG.self_test_tolerance;
      return { value: `held-out bytes/frame at 1920x1080 = ${truth.toFixed(1)}`, measured: rows.map((v) => `${v.verdict}, predicted ${v.predicted}`).join('; ') || 'no held-out bytes row', pass: ok };
    }),
    '#18 D bytes: P and K apart, predicted with the requested GOP': need(() => {
      if (!F.fitDBytes) return { value: '-', measured: 'fit.mjs has no fitDBytes', pass: false };
      const { train, held } = dBytesConfigs();
      const m = F.fitDBytes(train);
      const rho = m.coefficients?.K_over_P?.DW1 ?? {};
      const errs = [...held.map((c) => relErr(m.predict(c), c.truth)), ...Object.entries(D_BYTES_TRUTH.rho).map(([c, v]) => relErr(rho[c], v)), relErr(m.coefficients?.a_E?.DW1, D_BYTES_TRUTH.a)];
      const w = Math.max(...errs);
      return { value: 'bytes at tau_f 100 and 200, K/P and a_E within 1e-6', measured: `worst relative error ${w}`, pass: Number.isFinite(w) && w <= CONFIG.self_test_tolerance };
    }),
    'eq 4 fitted terms: mu and m_d0 from training rows': need(() => {
      const { zRows, runlist } = ramFixture();
      const f = F.buildFitFromRows({ zRows, dRows: [], runlist });
      const k = f.equations.eq4_z_ram.coefficients;
      const held = f.validation.z.rows.filter((v) => v.quantity === 'ram' || v.quantity === 'ram_dec');
      const errs = [relErr(k.mu_train, RAM_TRUTH.mu), relErr(k.m_d0_bytes, RAM_TRUTH.md0), ...held.map((v) => relErr(v.predicted, v.measured))];
      const w = Math.max(...errs);
      return { value: `mu ${RAM_TRUTH.mu}, m_d0 ${RAM_TRUTH.md0} B; held-out ZW16 1920x1080 RAM and decoder RAM`, measured: `mu ${k.mu_train}, m_d0 ${k.m_d0_bytes}; ${held.length} held-out rows (${held.map((v) => v.verdict).join(', ')}); worst relative error ${w}`, pass: held.length === 2 && held.every((v) => v.verdict === 'within') && Number.isFinite(w) && w <= CONFIG.self_test_tolerance };
    }),
    'eq 4 fitted terms on the input CSVs': need(() => {
      if (!inputs.z.length) return { value: '-', measured: 'needs the Z input CSVs of fit.json', pass: false };
      const f = F.buildFitFromRows({ zRows: inputs.z, dRows: [], runlist: { cells: [], contents: [] } });
      const e = f.equations.eq4_z_ram;
      const dictRows = inputs.z.filter((r) => Number(r.dict) > 0 && !r.error).length;
      const muOk = dictRows ? Number.isFinite(e.coefficients.mu_train) : e.coefficients.mu_train === undefined && e.pending.some((p) => p.part === 'mu');
      const res = `M_comp residuals rms ${(100 * e.residuals.rms_rel).toFixed(2)} %, max ${(100 * e.residuals.max_abs_rel).toFixed(2)} %; M_dec rms ${(100 * e.residuals.dec.rms_rel).toFixed(2)} %, max ${(100 * e.residuals.dec.max_abs_rel).toFixed(2)} %`;
      return { value: 'mu from the ZW16 rows, m_d0 from every decoder row (training only)', measured: `${dictRows} ZW16 rows: mu ${e.coefficients.mu_train ?? 'pending'}; m_d0 ${e.coefficients.m_d0_bytes} B; ${res}`, pass: muOk && Number.isFinite(e.coefficients.m_d0_bytes) };
    }),
    '#20 predicted cells appear with status and uncertainty': need(() => {
      const { zRows, runlist } = sizeFixture();
      const f = F.buildFitFromRows({ zRows, dRows: [], runlist });
      const rows = f.predicted?.rows ?? [];
      const want = runlist.cells.filter((c) => c.status === 'predicted').length * runlist.contents.length;
      const covered = rows.reduce((s, r) => s + (r.content === 'all' ? runlist.contents.length : 1), 0);
      const r = rows.find((x) => x.cell === 4 && x.content === 'typing');
      const truth = zBytesPerFrame(506 * 333);
      const ok = covered === want && r?.status === 'predicted' && relErr(r.bytes_per_frame, truth) <= CONFIG.self_test_tolerance && Number.isFinite(r.bytes_rel_unc);
      return { value: `${want} cell x content; bytes/frame ${truth.toFixed(1)} with uncertainty`, measured: r ? `${covered} covered; ${r.status}, bytes/frame ${r.bytes_per_frame}, uncertainty ${r.bytes_rel_unc}` : `${covered} covered, no row for cell 4`, pass: ok };
    }),
  };
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
  const tauSweep = (encoder) => {
    const g = new Map();
    for (const c of inFit('D').filter((x) => x.options.encoder === encoder && x.options.hint === CONFIG.d_base.hint && x.options.rate === CONFIG.d_base.rate)) { const k = `${c.options.encoder}|${c.folder}|${c.options.q}`; g.set(k, (g.get(k) ?? new Set()).add(c.options.tau_f)); }
    return [...g.values()].some((s) => s.size >= 2);
  };
  return {
    ...findingChecks(F, inputs),
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
    'eq 4 Z RAM': need(() => equation(fit, 'eq4_z_ram', (p) => (p === 'mu' && !inFit('Z').some((c) => c.fit?.s_train > 0)) || (p === 'm_d0' && !inFit('Z').length))),
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
    'eq 7 D bytes': need(() => equation(fit, 'eq7_d_bytes', (p) => /^K\/P \((.+)\)$/.test(p) && !tauSweep(/^K\/P \((.+)\)$/.exec(p)[1]))),
    'eq 7 D quality': need(() => equation(fit, 'eq7_d_quality', () => false)),
    'eq 7 D q_min': need(() => {
      const e = fit.equations?.eq7_d_qmin;
      if (!e) return { value: '-', measured: 'no eq7_d_qmin in fit.json', pass: false };
      const text = new Set(CONFIG.text_scenarios);
      const groups = new Map();
      const complete = completeD(inputs.d);
      const held = new Set(drawD(inputs.d).held);
      for (const r of inputs.d.filter((x) => complete.has(keyOf('D', x)) && !held.has(keyOf('D', x)) && text.has(x.scenario) && String(x.r).toLowerCase() !== 'n/a')) {
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
      const v = validation(fit, 'Z', held, ['bytes', 'cpu', 'ram', 'ram_dec']);
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
