#!/usr/bin/env node
// Experiments step 4: fit and validate the equations of docs/clip-recorder-experiments-plan.md.
//
//   node scripts/clip-equation/fit/fit.mjs --z <z CSV>[,<z CSV>] --d <d CSV>[,<d CSV>] [--runlist <json>] [--out <json>]
//
// Reads the run CSVs of way Z (runs/run-z.mjs) and way D (runs/run-d.mjs), groups the repeats of
// each configuration (mean and spread = half the range), converts to per-minute values, applies
// the target O = [R = 1], fits equations 1 to 7 on the fit set and predicts the held-out
// configurations, which are never in a fit set. Writes results/fit.json. Node built-ins only.
//
// Per-minute values: bytes/min = mean(bytes) * (N_new / frames) * 60 / t (frames < N_new only in
// the pilot's capped runs). CPU = process cycle-counter CPU to create the clip: way Z compress +
// write (comp_cpu_ns + write_cpu_ns), way D live + worker (a_rt_ns + a_w_ns), in s per minute.
// RAM = peak private bytes above the process base: way Z peak_private - m0_private, way D
// m_live + m_worker (equation 7, M_D = M_live + M_worker).
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { readRuns } from '../spec/step2d.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const BASE = path.resolve(HERE, '..');
export const CONFIG = JSON.parse(fs.readFileSync(path.join(HERE, 'fit-config.json'), 'utf8'));

const num = (v) => (v === undefined || v === '' ? NaN : Number(v));
const mean = (a) => a.reduce((s, v) => s + v, 0) / a.length;
const spread = (a) => (Math.max(...a) - Math.min(...a)) / 2;
const stat = (a) => ({ mean: mean(a), spread: spread(a) });
const contentOf = (folder) => folder.slice(0, folder.indexOf('-'));
const sizeOf = (folder) => folder.slice(folder.indexOf('-') + 1);

// ---------- fitting primitives (proven by spec/step4.mjs on synthetic data) ----------

/** y = b(g) + slope * x: one slope shared by all groups, one intercept per group (two-pass sums). */
export function fitSharedSlope(points) {
  const groups = new Map();
  for (const p of points) groups.set(p.g, [...(groups.get(p.g) ?? []), p]);
  let wxx = 0;
  let wxy = 0;
  const means = new Map();
  for (const [g, ps] of groups) {
    const mx = mean(ps.map((p) => p.x));
    const my = mean(ps.map((p) => p.y));
    means.set(g, { mx, my });
    for (const p of ps) { wxx += (p.x - mx) ** 2; wxy += (p.x - mx) * (p.y - my); }
  }
  const identifiable = wxx > 1e-12 * Math.max(1, points.length);
  const slope = identifiable ? wxy / wxx : NaN;
  const intercept = Object.fromEntries([...means].map(([g, m]) => [g, identifiable ? m.my - slope * m.mx : NaN]));
  const residuals = identifiable ? points.map((p) => p.y - (intercept[p.g] + slope * p.x)) : [];
  return { slope, intercept, identifiable, residuals };
}

/** y = c0 + slope(g) * x: one intercept shared by all groups, one slope per group (closed form). */
export function fitSharedIntercept(points) {
  const groups = new Map();
  for (const p of points) {
    const s = groups.get(p.g) ?? { n: 0, sx: 0, sy: 0, sxx: 0, sxy: 0 };
    s.n += 1; s.sx += p.x; s.sy += p.y; s.sxx += p.x * p.x; s.sxy += p.x * p.y;
    groups.set(p.g, s);
  }
  let top = 0;
  let bottom = 0;
  for (const s of groups.values()) { top += s.sy - (s.sxy * s.sx) / s.sxx; bottom += s.n - (s.sx * s.sx) / s.sxx; }
  const identifiable = bottom > 1e-9 * Math.max(1, points.length);
  const c0 = identifiable ? top / bottom : NaN;
  const slope = Object.fromEntries([...groups].map(([g, s]) => [g, (s.sxy - (identifiable ? c0 : 0) * s.sx) / s.sxx]));
  return { c0, slope, identifiable };
}

/** Least squares X beta = y by normal equations on scaled columns; beta null when X has not full rank. */
export function leastSquares(X, y) {
  const k = X[0]?.length ?? 0;
  const scale = Array.from({ length: k }, (_, j) => Math.max(...X.map((r) => Math.abs(r[j]))) || 1);
  const A = Array.from({ length: k }, () => new Array(k + 1).fill(0));
  X.forEach((row, i) => {
    const r = row.map((v, j) => v / scale[j]);
    for (let a = 0; a < k; a++) { for (let b = 0; b < k; b++) A[a][b] += r[a] * r[b]; A[a][k] += r[a] * y[i]; }
  });
  const diag = Math.max(...A.map((r, i) => Math.abs(r[i])), 0);
  for (let c = 0; c < k; c++) {
    let p = c;
    for (let r = c + 1; r < k; r++) if (Math.abs(A[r][c]) > Math.abs(A[p][c])) p = r;
    if (Math.abs(A[p][c]) <= 1e-10 * diag || X.length < k) return { beta: null, rankOk: false };
    [A[c], A[p]] = [A[p], A[c]];
    for (let r = 0; r < k; r++) {
      if (r === c) continue;
      const f = A[r][c] / A[c][c];
      for (let j = c; j <= k; j++) A[r][j] -= f * A[c][j];
    }
  }
  const beta = A.map((r, i) => r[k] / r[i] / scale[i]);
  return { beta, rankOk: true };
}

/** mulberry32, as in runs/run-z.mjs. */
export function mulberry32(seed) {
  let s = seed >>> 0;
  return () => {
    s = (s + 0x6d2b79f5) >>> 0;
    let t = s;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/** The first `share` of the keys after a seeded shuffle of the sorted keys. */
export function drawHoldOut(keys, share = CONFIG.hold_out.share, seed = CONFIG.hold_out.seed) {
  const random = mulberry32(seed);
  const sorted = [...keys].sort();
  return sorted.map((k) => [random(), k]).sort((a, b) => a[0] - b[0]).map(([, k]) => k).slice(0, Math.round(share * sorted.length));
}

/** n, max and rms of relative residuals. */
function residualStats(rel) {
  const r = rel.filter(Number.isFinite);
  return { n: r.length, max_abs_rel: r.length ? Math.max(...r.map(Math.abs)) : 0, rms_rel: r.length ? Math.sqrt(mean(r.map((v) => v * v))) : 0 };
}

// ---------- configurations ----------

const keyOf = (kind, row) => `${kind}|${CONFIG[kind === 'Z' ? 'z_key_columns' : 'd_key_columns'].map((c) => row[c] ?? '').join('|')}`;
const cfgOf = (kind, row) => `${kind}|${CONFIG[kind === 'Z' ? 'z_key_columns' : 'd_key_columns'].filter((c) => c !== 'folder').map((c) => row[c] ?? '').join('|')}`;

/** N_new of a corpus folder, from its frames.jsonl: a capped pilot run's n_new column is the cap, not N_new. */
const corpusNNew = (() => {
  const cache = new Map();
  return (folder) => {
    if (!cache.has(folder)) {
      const text = fs.readFileSync(path.join(CONFIG.corpus_root, folder, 'frames.jsonl'), 'utf8');
      cache.set(folder, text.split('\n').filter((l) => l.trim() && JSON.parse(l).new === true).length);
    }
    return cache.get(folder);
  };
})();

/** t of a corpus folder in seconds, from its meta.json. */
const clipSeconds = (() => {
  const cache = new Map();
  return (folder) => {
    if (!cache.has(folder)) {
      const meta = JSON.parse(fs.readFileSync(path.join(CONFIG.corpus_root, folder, 'meta.json'), 'utf8'));
      cache.set(folder, (meta.instants * meta.instant_ms) / 1000);
    }
    return cache.get(folder);
  };
})();

function groupRows(kind, rows) {
  const groups = new Map();
  for (const r of rows) {
    const key = keyOf(kind, r);
    groups.set(key, [...(groups.get(key) ?? []), r]);
  }
  return groups;
}

function options(kind, r) {
  const cols = CONFIG[kind === 'Z' ? 'z_key_columns' : 'd_key_columns'].filter((c) => c !== 'folder' && c !== 'way' && c !== 'encoder');
  return Object.fromEntries(cols.filter((c) => r[c] !== undefined && r[c] !== '').map((c) => [c, r[c]]));
}

/** One configuration of way Z: the repeats of one key. */
function zConfiguration(key, rows, heldCells) {
  const r0 = rows[0];
  const t = clipSeconds(r0.folder);
  const frames = num(r0.frames);
  const nNew = corpusNNew(r0.folder);
  const scale = (nNew / frames) * 60 / t;
  const error = rows.find((r) => r.error)?.error ?? '';
  const exact = rows.every((r) => r.diff_sum === '0' && num(r.frames_decoded) === num(r.frames));
  const held = rows.some((r) => r.held_out === '1' || heldCells.has(String(r.cell)));
  const kept = !error && exact;
  const px = num(r0.px);
  return {
    kind: 'Z', key, cfg: cfgOf('Z', r0), way: r0.way, options: options('Z', r0), folder: r0.folder, content: contentOf(r0.folder), size: sizeOf(r0.folder),
    px, n_new: nNew, t_s: t, runs: rows.length, frames, capped: frames < nNew, status: error ? 'error' : 'ok',
    bytes_min: mean(rows.map((r) => num(r.bytes_out))) * scale,
    cpu_s_min: stat(rows.map((r) => ((num(r.comp_cpu_ns) + num(r.write_cpu_ns)) / 1e9) * scale)),
    ram_peak: stat(rows.map((r) => num(r.peak_private) - num(r.m0_private))),
    ssim: exact ? 1 : NaN, psnr: exact ? 'inf' : NaN, r: exact ? 1 : 0,
    O: kept, clause: error ? `run error: ${error}` : exact ? '' : `R < 1: round trip differs (diff_sum ${rows.map((r) => r.diff_sum).join('/')})`,
    held_out: held, in_fit: kept && !held,
    fit: {
      zeta: mean(rows.map((r) => num(r.bytes_out) / num(r.frames))) / (px * 3),
      bytes_per_frame: stat(rows.map((r) => num(r.bytes_out) / num(r.frames))),
      comp_ns_per_frame: stat(rows.map((r) => num(r.comp_cpu_ns) / num(r.frames))),
      dec_ns_per_px_frame: mean(rows.map((r) => num(r.dec_cpu_ns) / (num(r.frames_decoded) * px))),
      peak_private: stat(rows.map((r) => num(r.peak_private))),
      known_m_comp: mean(rows.map((r) => num(r.known_m_comp))),
      m0: mean(rows.map((r) => num(r.m0_private))), m_cctx: num(r0.m_cctx_est), bc: num(r0.bc),
      ram_rel: rows.map((r) => (num(r.peak_private) - num(r.known_m_comp)) / num(r.known_m_comp)),
      ram_dec_rel: rows.map((r) => (num(r.dec_peak_private) - num(r.known_m_dec)) / num(r.known_m_dec)),
    },
  };
}

/** One configuration of way D: the repeats of one key. */
function dConfiguration(key, rows) {
  const r0 = rows[0];
  const t = clipSeconds(r0.folder);
  const base = { kind: 'D', key, cfg: cfgOf('D', r0), way: r0.encoder, options: options('D', r0), folder: r0.folder, content: contentOf(r0.folder), size: sizeOf(r0.folder), runs: rows.length, t_s: t, n_new: num(r0.n_new), status: r0.status };
  const ok = rows.filter((r) => r.status === 'ok');
  if (!ok.length) return { ...base, px: num(r0.width) * num(r0.height), O: false, clause: `cannot run: ${r0.status}: ${r0.error}`, held_out: false, in_fit: false, text: false };
  const n = num(r0.n_new);
  const pxOut = num(r0.out_w) * num(r0.out_h);
  const rs = ok.map((r) => num(r.r)).filter(Number.isFinite);
  return {
    ...base, px: num(r0.width) * num(r0.height), px_out: pxOut, frames: num(r0.frames),
    text: CONFIG.text_scenarios.includes(r0.scenario),
    bytes_min: mean(ok.map((r) => num(r.bytes))) * 60 / t,
    cpu_s_min: stat(ok.map((r) => ((num(r.a_rt_ns) + num(r.a_w_ns)) / 1e9) * 60 / t)),
    ram_peak: stat(ok.map((r) => num(r.m_live) + num(r.m_worker))),
    ssim: mean(ok.map((r) => num(r.ssim_mean))), psnr: mean(ok.map((r) => num(r.psnr_mean))), r: rs.length ? Math.min(...rs) : NaN,
    held_out: false, in_fit: true,
    fit: {
      bytes: stat(ok.map((r) => num(r.bytes))), y_bytes: mean(ok.map((r) => num(r.bytes))) / (n * pxOut),
      ssim: stat(ok.map((r) => num(r.ssim_mean))),
      a_rt_frame: mean(ok.map((r) => num(r.a_rt_ns) / n)), a_w_frame: mean(ok.map((r) => num(r.a_w_ns) / n)),
      cpu_ns: stat(ok.map((r) => num(r.a_rt_ns) + num(r.a_w_ns))),
      m_live: mean(ok.map((r) => num(r.m_live))), m_worker: mean(ok.map((r) => num(r.m_worker))),
      ram: stat(ok.map((r) => num(r.m_live) + num(r.m_worker))), keyframes: num(r0.keyframes),
    },
  };
}

const qminGroup = (c) => `${c.way} ${c.size} tau${c.options.tau_f ?? ''} hint${c.options.hint ?? ''} ${c.options.rate ?? ''} beta${c.options.beta ?? ''}`;

/** q_min(E, px): the lowest q with R = 1 on every text scenario; "not reached" when every q misses. */
function qMin(dConfigs) {
  const groups = new Map();
  for (const c of dConfigs.filter((x) => x.status === 'ok' && x.text)) {
    const byQ = groups.get(qminGroup(c)) ?? new Map();
    byQ.set(c.options.q, (byQ.get(c.options.q) ?? new Map()).set(c.content, c.r));
    groups.set(qminGroup(c), byQ);
  }
  const out = {};
  for (const [g, byQ] of groups) {
    const ok = [...byQ].filter(([, cell]) => CONFIG.text_scenarios.every((s) => cell.get(s) === 1)).map(([q]) => Number(q));
    const failed = [...byQ].filter(([, cell]) => [...cell.values()].some((r) => r !== 1)).length;
    out[g] = ok.length ? Math.min(...ok) : failed === byQ.size ? 'not reached' : 'pending: full run';
  }
  return out;
}

/** O for D: own R = 1 on a text scenario; q >= q_min(E, px) on the others. */
function applyDTarget(dConfigs, qmin) {
  for (const c of dConfigs.filter((x) => x.status === 'ok')) {
    if (c.text) {
      c.O = c.r === 1;
      c.clause = c.O ? '' : `R = ${Number.isFinite(c.r) ? c.r.toFixed(4) : 'not read'} < 1`;
    } else {
      const q = qmin[qminGroup(c)];
      c.O = typeof q === 'number' && Number(c.options.q) >= q;
      c.clause = c.O ? '' : typeof q === 'number' ? `q ${c.options.q} < q_min ${q}` : `q_min(E, px) ${q ?? 'pending: full run (no text scenario run)'}`;
    }
  }
}

// ---------- equations ----------

const zGroup = (c) => `${c.cfg}|${c.content}`;

function fitZ(z) {
  const fitSet = z.filter((c) => c.in_fit);
  const modes = [...new Set(fitSet.map((c) => c.options.mode ?? ''))];
  const eta = {};
  const zetaFits = {};
  const cpuFits = {};
  const etaMissing = [];
  const etaRes = [];
  const cpuRes = [];
  for (const m of modes) {
    const set = fitSet.filter((c) => (c.options.mode ?? '') === m);
    const f = fitSharedSlope(set.map((c) => ({ g: zGroup(c), x: Math.log(c.px / CONFIG.px0), y: Math.log(c.fit.zeta) })));
    zetaFits[m] = f;
    if (f.identifiable) { eta[m] = f.slope + 1; etaRes.push(...f.residuals.map((r) => Math.exp(r) - 1)); } else etaMissing.push(m);
    const k = fitSharedIntercept(set.map((c) => ({ g: zGroup(c), x: c.px, y: c.fit.comp_ns_per_frame.mean })));
    cpuFits[m] = k;
    for (const c of set) {
      const pred = (k.identifiable ? k.c0 : 0) + k.slope[zGroup(c)] * c.px;
      cpuRes.push((pred - c.fit.comp_ns_per_frame.mean) / c.fit.comp_ns_per_frame.mean);
    }
  }
  const anyEta = Object.keys(eta).length > 0;
  const repeatRes = fitSet.flatMap((c) => [(c.fit.bytes_per_frame.spread) / c.fit.bytes_per_frame.mean]);
  const c0 = Object.fromEntries(modes.filter((m) => cpuFits[m].identifiable).map((m) => [m, cpuFits[m].c0]));
  const kappaD = {};
  for (const c of fitSet) (kappaD[`L${c.options.level}|${c.options.mode}`] ??= []).push(c.fit.dec_ns_per_px_frame);
  const ramRel = z.flatMap((c) => c.fit.ram_rel);
  const ramDecRel = z.flatMap((c) => c.fit.ram_dec_rel);
  const tol = CONFIG.tolerance.ram;
  return {
    fits: { zetaFits, cpuFits },
    eq1_z_bytes: {
      title: 'Z bytes', equation: 'b_k = zeta(L, m, c, px) * px * 3 (+ h); zeta(px) = zeta(px0) * (px / px0)^(eta_m - 1)', fit_n: fitSet.length,
      coefficients: { zeta: Object.fromEntries(fitSet.map((c) => [`${zGroup(c)}@${c.size}`, c.fit.zeta])), eta, ...(etaMissing.length && anyEta ? { eta_missing_modes: etaMissing.join(' ') } : {}) },
      residuals: anyEta ? residualStats(etaRes) : { ...residualStats(repeatRes), note: 'repeat residuals of bytes per frame (zstd is deterministic); eta residuals need two sizes' },
      pending: anyEta ? [] : [{ part: 'eta', reason: 'every configuration ran at one size' }],
    },
    eq2_z_quality: {
      title: 'Z quality', equation: 'sum |decoded - corpus| = 0  =>  SSIM-Y = 1, PSNR-Y = inf, R = 1', fit_n: z.length,
      coefficients: { runs_exact: z.reduce((s, c) => s + (c.r === 1 ? c.runs : 0), 0), runs: z.reduce((s, c) => s + c.runs, 0) },
      residuals: residualStats(z.map((c) => (c.r === 1 ? 0 : 1))), pending: [],
    },
    eq3_z_cpu: {
      title: 'Z CPU', equation: 'C_comp / N_new = c_0(m) + px * kappa_c(L, m, c); C_dec = N_dec * px * kappa_d(L, m)', fit_n: fitSet.length,
      coefficients: {
        c_0_ns: c0,
        kappa_c_ns_per_px: Object.fromEntries(fitSet.map((c) => [zGroup(c), cpuFits[c.options.mode ?? ''].slope[zGroup(c)]])),
        kappa_d_ns_per_px: Object.fromEntries(Object.entries(kappaD).map(([k, v]) => [k, mean(v)])),
      },
      residuals: residualStats(cpuRes),
      pending: Object.keys(c0).length ? [] : [{ part: 'c_0', reason: 'every configuration ran at one size, so c_0 and px * kappa_c are not separable; kappa_c = C / (N_new * px)' }],
    },
    eq4_z_ram: {
      title: 'Z RAM', equation: 'M_comp = M_0 + M_cctx(L, W) + B_c * px * 3; M_dec = M_0 + M_dctx(W) + B_d * px * 3', fit_n: z.length,
      coefficients: {
        M_0_bytes: mean(z.map((c) => c.fit.m0)),
        share_within_10pct_comp: ramRel.filter((v) => Math.abs(v) <= tol).length / ramRel.length,
        share_within_10pct_dec: ramDecRel.filter((v) => Math.abs(v) <= tol).length / ramDecRel.length,
      },
      residuals: residualStats(ramRel), pending: [],
    },
    eq5_z_ready: {
      title: 'Z time until ready', equation: 't_ready = t_close', fit_n: z.length,
      coefficients: { limit_ms: CONFIG.t_ready_limit_ms }, residuals: null, pending: [],
    },
  };
}

function fitD(d, zFits, z) {
  const fitSet = d.filter((c) => c.in_fit && c.status === 'ok');
  const base = fitSet.filter((c) => c.options.tau_f === CONFIG.d_base.tau_f && c.options.hint === CONFIG.d_base.hint && c.options.rate === CONFIG.d_base.rate);
  const encoders = [...new Set(base.map((c) => c.way))];
  const g = (c) => `${c.way}|${c.content}`;
  const bytes = {};
  const ssim = {};
  const psnr = {};
  const res = { bytes: [], ssim: [], psnr: [] };
  for (const e of encoders) {
    const set = base.filter((c) => c.way === e);
    bytes[e] = fitSharedSlope(set.map((c) => ({ g: g(c), x: Number(c.options.q), y: Math.log(c.fit.y_bytes) })));
    ssim[e] = fitSharedSlope(set.filter((c) => c.ssim < 1).map((c) => ({ g: g(c), x: Number(c.options.q), y: Math.log(1 - c.ssim) })));
    psnr[e] = fitSharedSlope(set.map((c) => ({ g: g(c), x: Number(c.options.q), y: c.psnr })));
    res.bytes.push(...bytes[e].residuals.map((r) => Math.exp(r) - 1));
    for (const c of set) {
      if (ssim[e].identifiable && c.ssim < 1) res.ssim.push(Math.abs(1 - Math.exp(ssim[e].intercept[g(c)] + ssim[e].slope * Number(c.options.q)) - c.ssim));
      if (psnr[e].identifiable) res.psnr.push((psnr[e].intercept[g(c)] + psnr[e].slope * Number(c.options.q) - c.psnr) / c.psnr);
    }
  }
  // K and P apart: groups (E, folder, q) with two tau_f or more.
  const sweep = new Map();
  for (const c of fitSet.filter((x) => x.options.rate === 'quality')) sweep.set(`${c.way}|${c.folder}|q${c.options.q}`, [...(sweep.get(`${c.way}|${c.folder}|q${c.options.q}`) ?? []), c]);
  const kp = {};
  for (const [k, set] of sweep) {
    if (new Set(set.map((c) => c.options.tau_f)).size < 2) continue;
    const f = leastSquares(set.map((c) => [c.n_new - c.fit.keyframes, c.fit.keyframes]), set.map((c) => c.fit.bytes.mean));
    if (f.rankOk) kp[k] = { P_bytes: f.beta[0], K_bytes: f.beta[1], K_over_P: f.beta[1] / f.beta[0] };
  }
  // CPU: a_rt / N = c_file + c_comp * px; a_w / N = e0 + A * px_out + B * px_out * q (A = c_dec + c_conv + e1, B = e1 * e2).
  const pxs = new Set(fitSet.map((c) => c.px));
  const onePx = pxs.size < 2;
  const cpu = { pending: [], coefficients: {}, res: [], predict: {} };
  const art = leastSquares(fitSet.map((c) => [1, c.px]), fitSet.map((c) => c.fit.a_rt_frame));
  const artMean = mean(fitSet.map((c) => c.fit.a_rt_frame));
  if (art.rankOk) Object.assign(cpu.coefficients, { c_file_ns: art.beta[0], c_comp_ns_per_px: art.beta[1] });
  else { cpu.coefficients.a_rt_per_frame_ns_at_one_px = artMean; cpu.pending.push({ part: 'c_comp, c_file', reason: 'every D run has one px' }); }
  cpu.predict.a_rt = (c) => (art.rankOk ? art.beta[0] + art.beta[1] * c.px : onePx && pxs.has(c.px) ? artMean : NaN);
  for (const c of fitSet) cpu.res.push((cpu.predict.a_rt(c) - c.fit.a_rt_frame) / c.fit.a_rt_frame);
  const cDec = (() => {
    const w = z.filter((c) => c.in_fit && c.options.mode === 'whole' && c.options.level === '-3' && c.way === 'ZW1');
    return w.length ? mean(w.map((c) => c.fit.dec_ns_per_px_frame)) : NaN;
  })();
  const aw = {};
  for (const e of [...new Set(fitSet.map((c) => c.way))]) {
    const set = fitSet.filter((c) => c.way === e && c.options.rate === 'quality');
    const full = leastSquares(set.map((c) => [1, c.px_out, c.px_out * Number(c.options.q)]), set.map((c) => c.fit.a_w_frame));
    if (full.rankOk) {
      const [e0, A, B] = full.beta;
      const e1 = A - cDec - CONFIG.c_conv_ns_per_px;
      cpu.coefficients[`c_enc(${e})`] = { e0_ns: e0, A_ns_per_px: A, B_ns_per_px_q: B, ...(Number.isFinite(e1) ? { e1_ns_per_px: e1, e2_per_q: B / e1 } : {}) };
      aw[e] = (c) => e0 + A * c.px_out + B * c.px_out * Number(c.options.q);
    } else {
      const outs = new Set(set.map((c) => c.px_out));
      const part = leastSquares(set.map((c) => [1, Number(c.options.q)]), set.map((c) => c.fit.a_w_frame));
      if (outs.size === 1 && part.rankOk) {
        const px = [...outs][0];
        cpu.coefficients[`c_enc(${e})`] = { a_w_per_frame_ns_at_q0: part.beta[0], B_ns_per_px_q: part.beta[1] / px, at_px_out: px };
        aw[e] = (c) => (c.px_out === px ? part.beta[0] + part.beta[1] * Number(c.options.q) : NaN);
      }
      cpu.pending.push({ part: `c_enc(${e}) e0, e1`, reason: `every ${e} run has one px_out, so e0 and A * px are not separable` });
    }
    for (const c of set) if (aw[e]) cpu.res.push((aw[e](c) - c.fit.a_w_frame) / c.fit.a_w_frame);
  }
  cpu.predict.a_w = (c) => (aw[c.way] ? aw[c.way](c) : NaN);
  // RAM: M_live = l0 + l1 * px; M_worker - 4.5 * px_out = m0_E + m1_E * px_out.
  const ram = { pending: [], coefficients: {}, res: [], predict: {} };
  const ml = leastSquares(fitSet.map((c) => [1, c.px]), fitSet.map((c) => c.fit.m_live));
  const mlMean = mean(fitSet.map((c) => c.fit.m_live));
  if (ml.rankOk) Object.assign(ram.coefficients, { M_live_0_bytes: ml.beta[0], M_live_per_px: ml.beta[1] });
  else { ram.coefficients.M_live_bytes_at_one_px = mlMean; ram.pending.push({ part: 'M_live(px)', reason: 'every D run has one px' }); }
  ram.predict.live = (c) => (ml.rankOk ? ml.beta[0] + ml.beta[1] * c.px : onePx && pxs.has(c.px) ? mlMean : NaN);
  const menc = {};
  for (const e of [...new Set(fitSet.map((c) => c.way))]) {
    const set = fitSet.filter((c) => c.way === e);
    const y = (c) => c.fit.m_worker - CONFIG.worker_bytes_per_px * c.px_out;
    const f = leastSquares(set.map((c) => [1, c.px_out]), set.map(y));
    if (f.rankOk) { ram.coefficients[`M_enc(${e})`] = { m0_bytes: f.beta[0], m1_bytes_per_px: f.beta[1] }; menc[e] = (c) => f.beta[0] + f.beta[1] * c.px_out; }
    else {
      const px = set[0].px_out;
      const m = mean(set.map(y));
      ram.coefficients[`M_enc(${e})`] = { M0_w_plus_M_enc_bytes: m, at_px_out: px };
      menc[e] = (c) => (c.px_out === px ? m : NaN);
      ram.pending.push({ part: `M_enc(${e}) m0, m1`, reason: `every ${e} run has one px_out` });
    }
  }
  ram.predict.worker = (c) => (menc[c.way] ? menc[c.way](c) + CONFIG.worker_bytes_per_px * c.px_out : NaN);
  for (const c of fitSet) ram.res.push((ram.predict.live(c) + ram.predict.worker(c) - c.fit.ram.mean) / c.fit.ram.mean);
  const coef = (fits, slopeName, interceptName, map = (v) => v, slopeMap = (v) => v) => Object.fromEntries(Object.entries(fits).filter(([, f]) => f.identifiable)
    .map(([e, f]) => [e, { [slopeName]: slopeMap(f.slope), [interceptName]: Object.fromEntries(Object.entries(f.intercept).map(([k, v]) => [k.split('|')[1], map(v)])) }]));
  return {
    fits: { bytes, ssim, psnr, cpu, ram, kp },
    eq7_d_bytes: {
      title: 'D bytes', equation: 'bytes_D = t * (n_new * P + (n_new / tau_f) * (K - P)); P = p_E,c * px * exp(a_E * q) (fitted at tau_f 20 with K folded in)', fit_n: base.length,
      coefficients: { P_at_tau20: coef(bytes, 'a_E', 'p_E,c', Math.exp), ...(Object.keys(kp).length ? { K_and_P: kp } : {}) },
      residuals: residualStats(res.bytes),
      pending: Object.keys(kp).length ? [] : [{ part: 'K/P', reason: 'no tau_f sweep in the input (grid B, D3)' }],
    },
    eq7_d_quality: {
      title: 'D quality', equation: 'Q_ssim = 1 - s_E,c * exp(-g_E * q); Q_psnr = m_E,c + l_E * q', fit_n: base.length,
      coefficients: { Q_ssim: coef(ssim, 'g_E', 's_E,c', Math.exp, (s) => -s), Q_psnr: coef(psnr, 'l_E', 'm_E,c') },
      residuals: { ...residualStats(res.psnr), max_abs_ssim: res.ssim.length ? Math.max(...res.ssim) : 0, note: 'max_abs_rel and rms_rel are of PSNR; max_abs_ssim is the SSIM error' },
      pending: [],
    },
    eq7_d_cpu: {
      title: 'D CPU', equation: 'a_rt = n_new * px * c_comp(-3) + n_new * c_file; a_w = n_new * px * (c_dec(-3) + c_conv) + n_new * c_enc(E, q, px); c_enc = e0_E + e1_E * px * (1 + e2_E * q)', fit_n: fitSet.length,
      coefficients: { ...cpu.coefficients, c_dec_ns_per_px_from_Z: cDec, c_conv_ns_per_px_prior: CONFIG.c_conv_ns_per_px }, residuals: residualStats(cpu.res), pending: cpu.pending,
    },
    eq7_d_ram: {
      title: 'D RAM', equation: 'M_D = M_live + M_worker; M_worker = M0_w + px * (3 + 1.5) * d + M_enc(E, px); M_enc = m0_E + m1_E * px', fit_n: fitSet.length,
      coefficients: ram.coefficients, residuals: residualStats(ram.res), pending: ram.pending,
    },
  };
}

// ---------- validation ----------

function verdict(quantity, predicted, measured, spreadValue) {
  if (!Number.isFinite(predicted)) return { quantity, predicted: null, measured, verdict: 'pending: full run', spread: spreadValue };
  const err = quantity === 'ssim' ? Math.abs(predicted - measured) : Math.abs(predicted - measured) / Math.abs(measured);
  return { quantity, predicted, measured, error: err, verdict: err <= CONFIG.tolerance[quantity] ? 'within' : 'not predictable', spread: spreadValue };
}

function validateZ(z, zFits) {
  return z.filter((c) => c.held_out && c.status === 'ok').flatMap((c) => {
    const m = c.options.mode ?? '';
    const zf = zFits.zetaFits[m];
    const kf = zFits.cpuFits[m];
    const bpf = zf?.identifiable && Number.isFinite(zf.intercept[zGroup(c)]) ? Math.exp(zf.intercept[zGroup(c)] + zf.slope * Math.log(c.px / CONFIG.px0)) * c.px * 3 : NaN;
    const cpf = kf?.identifiable && Number.isFinite(kf.slope[zGroup(c)]) ? kf.c0 + kf.slope[zGroup(c)] * c.px : NaN;
    return [
      verdict('bytes', bpf, c.fit.bytes_per_frame.mean, c.fit.bytes_per_frame.spread),
      verdict('cpu', cpf, c.fit.comp_ns_per_frame.mean, c.fit.comp_ns_per_frame.spread),
      verdict('ram', c.fit.known_m_comp, c.fit.peak_private.mean, c.fit.peak_private.spread),
    ].map((v) => ({ key: c.key, ...v }));
  });
}

function validateD(d, dFits) {
  const g = (c) => `${c.way}|${c.content}`;
  return d.filter((c) => c.held_out).flatMap((c) => {
    const q = Number(c.options.q);
    const b = dFits.bytes[c.way];
    const s = dFits.ssim[c.way];
    const bytes = b?.identifiable && Number.isFinite(b.intercept[g(c)]) ? Math.exp(b.intercept[g(c)] + b.slope * q) * c.px_out * c.n_new : NaN;
    const ssim = s?.identifiable && Number.isFinite(s.intercept[g(c)]) ? 1 - Math.exp(s.intercept[g(c)] + s.slope * q) : NaN;
    const cpu = (dFits.cpu.predict.a_rt(c) + dFits.cpu.predict.a_w(c)) * c.n_new;
    const ram = dFits.ram.predict.live(c) + dFits.ram.predict.worker(c);
    return [
      verdict('bytes', bytes, c.fit.bytes.mean, c.fit.bytes.spread),
      verdict('cpu', cpu, c.fit.cpu_ns.mean, c.fit.cpu_ns.spread),
      verdict('ram', ram, c.fit.ram.mean, c.fit.ram.spread),
      verdict('ssim', ssim, c.fit.ssim.mean, c.fit.ssim.spread),
    ].map((v) => ({ key: c.key, ...v }));
  });
}

// ---------- main ----------

function parseArgs(argv) {
  const args = {};
  for (let i = 0; i < argv.length; i += 2) args[argv[i].replace(/^--/, '')] = argv[i + 1];
  const list = (v) => (v ? v.split(',').map((p) => path.resolve(p)) : []);
  return { z: list(args.z), d: list(args.d), runlist: path.resolve(BASE, args.runlist ?? CONFIG.runlist), out: path.resolve(BASE, args.out ?? CONFIG.fit_out) };
}

export function buildFit({ z: zPaths, d: dPaths, runlist: runlistPath }) {
  const read = (paths) => paths.map((p) => ({ path: p, rows: readRuns(p).rows }));
  const zIn = read(zPaths);
  const dIn = read(dPaths);
  const runlist = fs.existsSync(runlistPath) ? JSON.parse(fs.readFileSync(runlistPath, 'utf8')) : { cells: [], counts: {} };
  const heldCells = new Set(runlist.cells.filter((c) => c.status === 'held-out').map((c) => String(c.id)));
  const z = [...groupRows('Z', zIn.flatMap((i) => i.rows))].map(([k, rows]) => zConfiguration(k, rows, heldCells));
  const d = [...groupRows('D', dIn.flatMap((i) => i.rows))].map(([k, rows]) => dConfiguration(k, rows));
  const qmin = qMin(d);
  applyDTarget(d, qmin);
  const eligible = d.filter((c) => c.status === 'ok' && c.options.tau_f === CONFIG.d_base.tau_f && c.options.hint === CONFIG.d_base.hint && c.options.rate === CONFIG.d_base.rate).map((c) => c.key);
  const heldD = drawHoldOut(eligible);
  for (const c of d) if (heldD.includes(c.key)) { c.held_out = true; c.in_fit = false; }
  const zf = fitZ(z);
  const df = fitD(d, zf.fits, z);
  const zRows = zIn.reduce((s, i) => s + i.rows.length, 0);
  const pilot = [...zPaths, ...dPaths].some((p) => /pilot/i.test(path.basename(p)));
  const label = pilot ? 'PILOT, not final' : zRows < (runlist.counts?.runs ?? 0) ? 'INCOMPLETE full run, not final' : 'FULL';
  const { fits: _zf, ...zEquations } = zf;
  const { fits: _df, ...dEquations } = df;
  return {
    created: new Date().toISOString(), label,
    inputs: { z: zIn.map((i) => ({ path: i.path, rows: i.rows.length })), d: dIn.map((i) => ({ path: i.path, rows: i.rows.length })), runlist: runlistPath, corpus_root: CONFIG.corpus_root },
    config: CONFIG,
    configurations: [...z, ...d],
    equations: {
      ...zEquations,
      eq6_run_time: {
        title: 'run time', equation: 'T = sum over runs of N_new * px * 3 / v(L, m) * repeats', fit_n: zRows,
        coefficients: { T_total_h: runlist.T_total_h ?? NaN, runs_planned: runlist.counts?.runs ?? 0, T_measured_h: zIn.flatMap((i) => i.rows).reduce((s, r) => s + num(r.run_wall_ms), 0) / 3.6e6 },
        residuals: null, pending: zRows >= (runlist.counts?.runs ?? 0) ? [] : [{ part: 'T', reason: `${zRows} of ${runlist.counts?.runs} planned Z runs` }],
      },
      ...dEquations,
      eq7_d_qmin: { title: 'D q_min', equation: 'q_min(E, px) = min { q : R = 1 on every text scenario }', fit_n: d.length, coefficients: qmin, residuals: null, pending: [] },
    },
    validation: {
      z: { runlist_held_out: heldCells.size, held_out_keys: z.filter((c) => c.held_out).map((c) => c.key), rows: validateZ(z, zf.fits) },
      d: { generator: 'mulberry32', seed: CONFIG.hold_out.seed, share: CONFIG.hold_out.share, eligible: eligible.length, held_out_keys: heldD, rows: validateD(d, df.fits) },
    },
  };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = parseArgs(process.argv.slice(2));
  if (!args.z.length && !args.d.length) {
    console.error('usage: node fit/fit.mjs --z <z CSV>[,..] --d <d CSV>[,..] [--runlist <json>] [--out <json>]');
    process.exit(2);
  }
  const fit = buildFit(args);
  fs.writeFileSync(args.out, `${JSON.stringify(fit, (k, v) => (typeof v === 'number' && !Number.isFinite(v) ? null : v), 1)}\n`);
  const kept = fit.configurations.filter((c) => c.O).length;
  console.log(`fit.json: ${fit.label}; ${fit.configurations.length} configurations (${kept} kept); Z held out ${fit.validation.z.held_out_keys.length}, D held out ${fit.validation.d.held_out_keys.length} of ${fit.validation.d.eligible}; written ${args.out}`);
}
