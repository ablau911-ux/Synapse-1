// Synthetic fixtures of the step 4 and step 5 checks (findings of issue #295).
// Spec: docs/clip-recorder-experiments-plan.md, "Measurement rules" M5, M7 and M10, "Method", "Equations" 1 to 7.
//
// Every fixture is made from known numbers, so a check can state the exact result it must get:
// Z rows from zeta(px) = zeta0 * (px / px0)^(eta - 1) and C / frame = c0 + kappa * px; D rows with
// a chosen R, status and PSNR per configuration and q. Corpus folders are real (t and N_new come
// from the corpus), the measured values are not.
import fs from 'node:fs';
import path from 'node:path';
import { CONFIG } from './step4.mjs';

const str = (o) => Object.fromEntries(Object.entries(o).map(([k, v]) => [k, v === undefined || v === null ? '' : String(v)]));
const pxOf = (size) => size.split('x').map(Number).reduce((a, b) => a * b, 1);

export const Z_TRUTH = { zeta0: 0.05, eta: 0.9, c0: 1e5, kappa: 3, m0: 1e7, mCctx: 5e5, frames: 100, meta: 30000 };

/** Bytes per frame of the Z truth at px. */
export const zBytesPerFrame = (px) => Z_TRUTH.zeta0 * (px / CONFIG.px0) ** (Z_TRUTH.eta - 1) * px * 3;

/** One way Z CSV row of the truth; `o` overrides any column. */
export function zRow(o) {
  const px = pxOf((o.folder ?? 'typing-640x360').split('-')[1]);
  const f = Z_TRUTH.frames;
  const mComp = Z_TRUTH.m0 + Z_TRUTH.mCctx + px * 3;
  const mDec = Z_TRUTH.m0 + 1e5 + px * 3;
  return str({
    folder: 'typing-640x360', way: 'ZW12', level: -3, layout: 'bgr', mode: 'prefix', tile: 32, gap: 0, wlog: 21, ldm: 0, dict: 0, workers: 0, kz: 20, theta: 25, drain: 0, frames_cap: '', rep: 1,
    px, frames: f, frames_decoded: f, diff_sum: 0, bytes_out: zBytesPerFrame(px) * f, frame_bytes: zBytesPerFrame(px) * f, bytes_disk: zBytesPerFrame(px) * f + Z_TRUTH.meta, comp_cpu_ns: (Z_TRUTH.c0 + Z_TRUTH.kappa * px) * f, write_cpu_ns: 0,
    peak_private: mComp, m0_private: Z_TRUTH.m0, known_m_comp: mComp, m_cctx_est: Z_TRUTH.mCctx, bc: 1,
    dec_cpu_ns: 2 * px * f, dec_peak_private: mDec, known_m_dec: mDec, t_ready_ns: 1e6, run_wall_ms: 1000, cell: '', held_out: 0, error: '',
    ...o,
  });
}

/**
 * The size fixture (#17, #20, #21): ZW12 (prefix, window resolved by the probe from the frame size)
 * at 640x360 and 1280x720 in the fit, 1920x1080 held out, 506x333 predicted; content typing; 3 repeats.
 */
export function sizeFixture() {
  const cells = [
    { id: 1, way: 'ZW12', kz: 20, level: -3, size: '640x360', status: 'run', wlog: 21 },
    { id: 2, way: 'ZW12', kz: 20, level: -3, size: '1280x720', status: 'run', wlog: 23 },
    { id: 3, way: 'ZW12', kz: 20, level: -3, size: '1920x1080', status: 'held-out', wlog: 24 },
    { id: 4, way: 'ZW12', kz: 20, level: -3, size: '506x333', status: 'predicted' },
  ];
  const zRows = cells.filter((c) => c.status !== 'predicted').flatMap((c) => [1, 2, 3].map((rep) => zRow({
    folder: `typing-${c.size}`, wlog: c.wlog, rep, cell: c.id, held_out: c.status === 'held-out' ? 1 : 0,
  })));
  const runlist = { contents: ['typing'], repeats: 3, counts: { runs: 9 }, cells: cells.map(({ wlog: _w, ...c }) => c) };
  return { zRows, runlist };
}

/** The fitted terms of equation 4 (plan c50439ec): M_train = mu * S_samples and the decoder overhead m_d0. */
export const RAM_TRUTH = { mu: 1.9, md0: 8e5 };

/** Training rows only: S_samples = min(frames, 20) * px * 3 for a dictionary way. */
export const sSamples = (frames, px) => Math.min(frames, CONFIG.dict_train_frames) * px * 3;

/**
 * The equation 4 fixture: ZW16 (dictionary 110 KB) at 640x360 and 1280x720 in the fit and at
 * 1920x1080 held out, ZW1 at 640x360 in the fit; RAM above M_0 (ram_os) = the run's equation part
 * + mu * S_samples (ZW16 only); decoder peak = known_m_dec + m_d0. 3 repeats, content typing.
 */
export function ramFixture() {
  const cells = [
    { id: 11, way: 'ZW16', dict: 110, level: -3, size: '640x360', status: 'run' },
    { id: 12, way: 'ZW16', dict: 110, level: -3, size: '1280x720', status: 'run' },
    { id: 13, way: 'ZW16', dict: 110, level: -3, size: '1920x1080', status: 'held-out' },
    { id: 14, way: 'ZW1', level: -3, size: '640x360', status: 'run' },
  ];
  const zRows = cells.flatMap((c) => [1, 2, 3].map((rep) => {
    const r = zRow({ folder: `typing-${c.size}`, way: c.way, mode: 'whole', wlog: 0, dict: c.dict ?? 0, rep, cell: c.id, held_out: c.status === 'held-out' ? 1 : 0 });
    const px = Number(r.px);
    const ramOs = Number(r.known_m_comp) - Number(r.m0_private) + (c.dict ? RAM_TRUTH.mu * sSamples(Number(r.frames), px) : 0);
    return { ...r, ram_os: String(ramOs), peak_private: String(Number(r.m0_private) + ramOs), dec_peak_private: String(Number(r.known_m_dec) + RAM_TRUTH.md0) };
  }));
  return { zRows, runlist: { contents: ['typing'], repeats: 3, counts: { runs: 12 }, cells } };
}

/** N_new and N_inst of a corpus folder (the measurement rows of spec/step2d.mjs compare with them). */
function corpusCounts(folder) {
  const lines = fs.readFileSync(path.join(CONFIG.corpus_root, folder, 'frames.jsonl'), 'utf8').split('\n').filter((l) => l.trim());
  return { nNew: lines.filter((l) => JSON.parse(l).new === true).length, nInst: lines.length };
}

/** Bytes of frames.jsonl and clip.json beside the MP4 payload of a D clip (M4). */
export const D_META = 50000;

/**
 * One way D CSV row that keeps every measurement rule of spec/step2d.mjs (two processes, RAM base and
 * OS peak of both phases, retained bytes, beta > 0, frames mapped by time); `o` overrides any column.
 */
export function dRow(o) {
  const folder = o.folder ?? 'typing-640x360';
  const { nNew, nInst } = corpusCounts(folder);
  const n = 100;
  const r = {
    grid: 'A', folder, scenario: 'typing', width: 640, height: 360, sigma: 2, out_w: 1280, out_h: 720,
    encoder: 'DW1', q: 50, tau_f: 20, hint: 1, rate: 'quality', beta: 0.1, repeat: 1, status: 'ok', error: '',
    n_new: n, frames: n, keyframes: Math.ceil(n / 20), bytes: 1e6, a_rt_ns: 1e8, a_w_ns: 5e8, m_live: 2e7, m_worker: 1e8,
    ssim_mean: 0.99, psnr_mean: 40, psnr_global: 40, r: 1, run_wall_ms: 1000,
    live_pid: 11, worker_pid: 12, input: 'bgra', live_os_peak_at_m0: 1e7, live_os_peak_private: 3e7, worker_os_peak_at_m0: 1e7, worker_os_peak_private: 1.2e8,
    index_bytes: D_META - 5000, clip_json_bytes: 5000, index_lines: nInst, frames_mapped: nNew, frames_missing: 0, frames_extra: 0, frames_mistimed: 0,
    ...o,
  };
  const bytes = Number(r.bytes);
  return str({
    ...r, frame_bytes: o.frame_bytes ?? bytes, bytes_disk: o.bytes_disk ?? bytes + D_META, bytes_on_disk: o.bytes_on_disk ?? o.bytes_disk ?? bytes + D_META,
    way_cpu_ns: o.way_cpu_ns ?? Number(r.a_rt_ns) + Number(r.a_w_ns), live_ram_os: o.live_ram_os ?? r.m_live, worker_ram_os: o.worker_ram_os ?? r.m_worker,
    ram_os: o.ram_os ?? Number(r.m_live) + Number(r.m_worker),
  });
}

/** H1: ZW9 with merge gap 0, 1 and 2 (3 repeats each) are three configurations, never one with 9 repeats. */
export function gapFixture() {
  return [0, 1, 2].flatMap((gap) => [1, 2, 3].map((rep) => zRow({ way: 'ZW9', mode: 'rect', wlog: 0, gap, rep })));
}

/**
 * H5: eligibility by requested configuration. typing q 50: repeat 3 refused q and was recorded under
 * its actual q 40; scroll q 50: repeat 2 missing; ui q 50: complete (control). Requested settings in *_req.
 */
export function requestedFixture() {
  const req = (r) => ({ ...r, q_req: r.q_req ?? r.q, tau_f_req: r.tau_f, hint_req: r.hint, rate_req: r.rate, beta_req: r.beta });
  const row = (scenario, repeat, o = {}) => req(dRow({ folder: `${scenario}-640x360`, scenario, q: 50, repeat, ...o }));
  return [
    row('typing', 1), row('typing', 2), row('typing', 3, { q: 40, q_req: 50, status: 'setting_refused:q', bytes: 9e9 }),
    // typing q 40 requested and run as such: its 3 valid repeats must stay one complete configuration (round 3).
    row('typing', 1, { q: 40 }), row('typing', 2, { q: 40 }), row('typing', 3, { q: 40 }),
    row('scroll', 1), row('scroll', 3),
    row('ui', 1), row('ui', 2), row('ui', 3),
  ];
}

/**
 * H5 round 4: a refusal into an existing valid group. typing, scroll, ui and flash run valid at q 40
 * (3 repeats each); typing requested q 50 ran 2 valid repeats, its repeat 3 was refused and recorded
 * under q 40. `withRefusal` false drops that one row, so both fits can be compared.
 */
export function refusalIntoValidFixture(withRefusal = true) {
  const req = (r) => ({ ...r, q_req: r.q_req ?? r.q, tau_f_req: r.tau_f, hint_req: r.hint, rate_req: r.rate, beta_req: r.beta });
  const row = (scenario, q, repeat, o = {}) => req(dRow({ folder: `${scenario}-640x360`, scenario, q, repeat, r: scenario === 'flash' ? 'n/a' : 1, ...o }));
  const rows = ['typing', 'scroll', 'ui', 'flash'].flatMap((s) => [1, 2, 3].map((k) => row(s, 40, k)));
  rows.push(row('typing', 50, 1), row('typing', 50, 2));
  if (withRefusal) rows.push(row('typing', 40, 3, { q_req: 50, status: 'setting_refused:q', bytes: 9e9, r: 0.5 }));
  return rows;
}

/**
 * H6: validity blocks, a known-result miss is reported. D typing q 50 has pass_keyframes = 0 (#297) and
 * stays eligible; D scroll q 50 has status ocr_error; Z ZW1 has pass_ramOs = 0 (RAM-base rule breach).
 */
export function validityFixture() {
  const d = [1, 2, 3].flatMap((repeat) => [
    dRow({ folder: 'typing-640x360', scenario: 'typing', repeat, pass_keyframes: 0, pass_frames: 1 }),
    dRow({ folder: 'scroll-640x360', scenario: 'scroll', repeat, status: repeat === 2 ? 'ocr_error' : 'ok' }),
    // Round 3: D measurement rules of spec/step2d.mjs. ui: RAM of the two phases does not add up (M3); flash: CPU is not live + worker (M2).
    dRow({ folder: 'ui-640x360', scenario: 'ui', repeat, ram_os: 5e7 }),
    dRow({ folder: 'flash-640x360', scenario: 'flash', repeat, r: 'n/a', way_cpu_ns: 1 }),
  ]);
  const z = [1, 2, 3].map((rep) => zRow({ way: 'ZW1', mode: 'whole', wlog: 0, rep, pass_ramOs: 0, pass_ram: 0 }));
  return { d, z };
}

/**
 * The target fixture (#8, #15, M5, M7) at 640x360, encoder DW1, 3 repeats:
 *   q 50: every text scenario R = 1, every repeat ok; flash PSNR inf (MSE 0)       -> flash q50 kept
 *   q 60: typing R = 0.9 (scroll, ui R = 1); flash PSNR inf                          -> flash q60 dropped (#15)
 *   q 70: typing repeat 2 failed; scroll repeat 3 has no R; ui refused q (M5)       -> all three dropped (#8)
 */
export function targetFixture() {
  const rows = [];
  const text = ['typing', 'scroll', 'ui'];
  const add = (scenario, q, per) => { for (const repeat of [1, 2, 3]) rows.push(dRow({ folder: `${scenario}-640x360`, scenario, q, repeat, bytes: 1e6 * Math.exp(0.02 * q), ssim_mean: 1 - 0.2 * Math.exp(-0.04 * q), psnr_mean: 20 + 0.3 * q, psnr_global: 20 + 0.3 * q, ...per(repeat) })); };
  for (const s of text) add(s, 50, () => ({ r: 1 }));
  add('flash', 50, () => ({ r: 'n/a', psnr_mean: 'inf', psnr_global: 'inf', mse_y: 0 }));
  for (const s of text) add(s, 60, () => ({ r: s === 'typing' ? 0.9 : 1 }));
  add('flash', 60, () => ({ r: 'n/a', psnr_mean: 'inf', psnr_global: 'inf', mse_y: 0 }));
  add('typing', 70, (rep) => (rep === 2 ? { status: 'error', error: 'encoder failed', r: '' } : { r: 1 }));
  add('scroll', 70, (rep) => ({ r: rep === 3 ? '' : 1 }));
  add('ui', 70, () => ({ status: 'setting_refused:q', r: 1 }));
  return rows;
}

/**
 * Synthetic D configurations for fitDBytes (#18, M5): P = p_c * px * exp(a q), K = k_c * px * exp(b q)
 * with b != a, bytes = (n - n_key) P + n_key K. `sweeps` = the q values with a GOP sweep (tau_f 20, 40,
 * 100, 200); every q of 30..90 runs at tau_f 20; hint-off rows must not enter the fit.
 */
export const D_BYTES_TRUTH = { p: { typing: 2e-3, scroll: 5e-3 }, k: { typing: 1.5e-2, scroll: 2e-2 }, a: 0.031, b: 0.024 };

export function dBytesCase(sweeps) {
  const n = 400;
  const pxOut = 1280 * 720;
  const T = D_BYTES_TRUTH;
  const make = (content, q, tau, hint = '1') => {
    const nk = Math.ceil(n / tau);
    const P = T.p[content] * pxOut * Math.exp(T.a * q);
    const K = T.k[content] * pxOut * Math.exp(T.b * q);
    const bytes = ((n - nk) * P + nk * K) * (hint === '1' ? 1 : 1.5);
    return { kind: 'D', way: 'DW1', content, folder: `${content}-640x360`, size: '640x360', px_out: pxOut, n_new: n, eligible: true, in_fit: true, status: 'ok',
      options: { encoder: 'DW1', q: String(q), tau_f: String(tau), hint, rate: 'quality' }, fit: { bytes: { mean: bytes, spread: 0 }, keyframes: nk }, truth: bytes };
  };
  const train = [];
  for (const c of ['typing', 'scroll']) {
    for (const q of [30, 50, 70, 90]) train.push(make(c, q, 20));
    for (const q of sweeps) for (const tau of [40, 100, 200]) train.push(make(c, q, tau));
    train.push(make(c, 70, 20, '0'), make(c, 90, 40, '0'));
  }
  return { train, make };
}