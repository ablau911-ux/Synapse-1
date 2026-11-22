// Experiments steps 2 and 3, way D: the checks of scripts/clip-equation/spec/step2d.feature.
// Spec: docs/clip-recorder-experiments-plan.md, "Acceptance (BDD)", D rows of steps 2 and 3.
//
//   node run-spec.mjs step2d <d-runs.csv> [corpus root]
//
// Each check computes its known result from its inputs (the corpus folder on disk,
// the run's own options, the plan's equations), prints the equation, the known value
// and the measured value, and the outcome. Exit code 1 when any check misses. Bytes,
// CPU and RAM have only a prior before the fit: they print the error and are PENDING,
// which is neither a pass nor a miss.
import fs from 'node:fs';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const DEFAULT_CORPUS = 'C:\\Users\\Adam\\clip-corpus';
const PROBE = path.join(HERE, '..', 'probe', 'target', 'release', 'clip-probe.exe');
const FEATURE = path.join(HERE, 'step2d.feature');
const KNOWN_ENCODERS = 4;
const DISTORTION_D = 4;
const PSNR_D4 = 20 * Math.log10(255 / DISTORTION_D); // 36.0896 dB
const PSNR_TOLERANCE_DB = 0.01;
// Priors (docs/clip-recorder-equation.md): section 6 bytes, "Inputs" table CPU.
const PRIOR_MB_PER_MIN = 10.3;
const PRIOR_PX = 1252 * 890;
const C_CONV_NS = 1.65;
const C_ENC_NS = { software: 7.0, hardware: 2.0 };
const T_S = 20;
const PASS = 'PASS';
const FAIL = 'FAIL';
const PENDING = 'PENDING';

/** The CSV the runner appends: a header line, then one row per run; no quoted fields. */
export function readRuns(csvPath) {
  if (!csvPath || !fs.existsSync(csvPath)) return { rows: [], error: `no CSV at ${csvPath ?? '(none given)'}` };
  const lines = fs.readFileSync(csvPath, 'utf8').split(/\r?\n/).filter((l) => l.trim() !== '');
  if (lines.length < 2) return { rows: [], error: `${csvPath} has no rows` };
  const head = lines[0].split(',');
  const rows = lines.slice(1).map((line) => Object.fromEntries(line.split(',').map((v, i) => [head[i], v])));
  return { rows, error: null };
}

/** N_new of a corpus folder, read from its frames.jsonl (not from the run). */
class CorpusFolder {
  static cache = new Map();

  static nNew(root, name) {
    const key = path.join(root, name);
    if (!CorpusFolder.cache.has(key)) {
      const lines = fs.readFileSync(path.join(key, 'frames.jsonl'), 'utf8').split('\n').filter((l) => l.trim());
      CorpusFolder.cache.set(key, lines.map((l) => JSON.parse(l)).filter((r) => r.new).length);
    }
    return CorpusFolder.cache.get(key);
  }
}

/** The scenarios of the feature file with their equation and known result. */
function readScenarios() {
  const text = fs.readFileSync(FEATURE, 'utf8');
  const out = [];
  for (const block of text.split(/^\s*Scenario:\s*/m).slice(1)) {
    const title = block.split(/\r?\n/)[0].trim();
    const note = block.match(/#\s*Equation:\s*(.*?)\s*\|\s*Known:\s*(.*)/);
    out.push({ title, equation: note ? note[1] : '-', known: note ? note[2].trim() : '-' });
  }
  return out;
}

const num = (v) => (v === undefined || v === '' ? NaN : Number(v));
const okRows = (rows) => rows.filter((r) => r.status === 'ok');
const misses = (bad) => (bad.length ? `; misses: ${bad.slice(0, 5).join('; ')}${bad.length > 5 ? ` (+${bad.length - 5})` : ''}` : '');
const pct = (measured, prior) => `${(((measured - prior) / prior) * 100).toFixed(1)} %`;

/** The probe's meter self-check on one corpus frame, run once. */
function meterSelfCheck(root) {
  if (!fs.existsSync(PROBE)) return { error: `no probe at ${PROBE}` };
  const run = spawnSync(PROBE, ['exp-d', 'mode=meter', `folder=${path.join(root, 'typing-1920x1080')}`], { encoding: 'utf8', windowsHide: true });
  const line = (run.stdout || '').split(/\r?\n/).find((l) => l.startsWith('{'));
  if (!line) return { error: `probe gave no JSON (exit ${run.status}): ${(run.stderr || '').trim().slice(0, 200)}` };
  return JSON.parse(line);
}

function checks(rows, root) {
  let meter;
  const getMeter = () => (meter ??= meterSelfCheck(root));
  const needRows = (fn) => () => (rows.length ? fn() : { value: '-', measured: 'no runs', pass: false });
  return {
    'D: encoders': needRows(() => {
      const byEncoder = new Map();
      for (const r of rows) {
        const seen = byEncoder.get(r.encoder) ?? { ok: false, hresult: '' };
        if (r.activated === '1') seen.ok = true;
        else if (!seen.hresult) seen.hresult = r.hresult || r.error;
        byEncoder.set(r.encoder, seen);
      }
      const ok = [...byEncoder].filter(([, s]) => s.ok).map(([e]) => e);
      const bad = [...byEncoder].filter(([, s]) => !s.ok).map(([e, s]) => `${e} ${s.hresult}`);
      return { value: KNOWN_ENCODERS, measured: `${ok.length} (${ok.join(' ')})${misses(bad)}`, pass: ok.length === KNOWN_ENCODERS };
    }),
    'D: frames encoded': needRows(() => {
      const runs = okRows(rows);
      const bad = runs.filter((r) => num(r.frames) !== CorpusFolder.nNew(root, r.folder)).map((r) => `${r.folder} ${r.encoder} q${r.q}: ${r.frames} != ${CorpusFolder.nNew(root, r.folder)}`);
      return { value: 'N_new per folder', measured: `${runs.length - bad.length}/${runs.length} runs equal${misses(bad)}`, pass: runs.length > 0 && bad.length === 0 };
    }),
    'D: keyframes': needRows(() => {
      const runs = okRows(rows);
      const bad = runs.filter((r) => num(r.keyframes) !== Math.ceil(num(r.frames) / num(r.tau_f)))
        .map((r) => `${r.folder} ${r.encoder} q${r.q} tau${r.tau_f}: ${r.keyframes} != ${Math.ceil(num(r.frames) / num(r.tau_f))}`);
      return { value: 'ceil(N_enc / tau_f)', measured: `${runs.length - bad.length}/${runs.length} runs equal${misses(bad)}`, pass: runs.length > 0 && bad.length === 0 };
    }),
    'D: B-frames': needRows(() => {
      const runs = okRows(rows);
      const bad = runs.filter((r) => num(r.b_frames) !== 0).map((r) => `${r.folder} ${r.encoder} q${r.q}: ${r.b_frames}`);
      return { value: 0, measured: `${runs.length - bad.length}/${runs.length} runs with 0${misses(bad)}`, pass: runs.length > 0 && bad.length === 0 };
    }),
    'D: meter self-check': () => {
      const m = getMeter();
      if (m.error) return { value: '1; inf; 1', measured: m.error, pass: false };
      const pass = m.ssim_xx === 1 && m.psnr_xx === 'inf' && m.r_xx === 1;
      return { value: '1; inf; 1', measured: `${m.ssim_xx}; ${m.psnr_xx}; ${m.r_xx} (OCR ${m.ocr_chars} chars)`, pass };
    },
    'D: known distortion': () => {
      const m = getMeter();
      if (m.error) return { value: PSNR_D4.toFixed(4), measured: m.error, pass: false };
      return { value: `${PSNR_D4.toFixed(4)} dB`, measured: `${Number(m.psnr_d4).toFixed(4)} dB (clipped pixels ${m.clipped_d4})`, pass: m.clipped_d4 === 0 && Math.abs(m.psnr_d4 - PSNR_D4) <= PSNR_TOLERANCE_DB };
    },
    'D: bytes against the prior': needRows(() => {
      const runs = okRows(rows).filter((r) => r.rate === 'quality' && r.repeat === '1');
      const errors = runs.map((r) => {
        const prior = PRIOR_MB_PER_MIN * 1e6 * (num(r.out_w) * num(r.out_h)) / PRIOR_PX;
        return { r, prior, measured: num(r.bytes) * (60 / T_S) };
      });
      const lines = errors.slice(0, 8).map((e) => `${e.r.folder} ${e.r.encoder} q${e.r.q}: ${(e.measured / 1e6).toFixed(2)} vs ${(e.prior / 1e6).toFixed(2)} MB/min (${pct(e.measured, e.prior)})`);
      return { value: 'prior', measured: `${runs.length} runs; ${lines.join('; ')}`, pass: PENDING };
    }),
    'D: CPU against the prior': needRows(() => {
      const runs = okRows(rows).slice(0, 8);
      const lines = runs.map((r) => {
        const kind = r.encoder === 'DW2' ? 'software' : 'hardware';
        const prior = num(r.n_new) * num(r.out_w) * num(r.out_h) * (C_CONV_NS + C_ENC_NS[kind]);
        return `${r.folder} ${r.encoder} q${r.q}: a_w ${(num(r.a_w_ns) / 1e6).toFixed(0)} vs ${(prior / 1e6).toFixed(0)} ms (${pct(num(r.a_w_ns), prior)})`;
      });
      return { value: 'prior', measured: lines.join('; '), pass: PENDING };
    }),
    'D: RAM against the prior': needRows(() => {
      const runs = okRows(rows).slice(0, 8);
      const lines = runs.map((r) => {
        const prior = num(r.out_w) * num(r.out_h) * 4.5;
        return `${r.folder} ${r.encoder} q${r.q}: M_worker ${(num(r.m_worker) / 1e6).toFixed(1)} vs ${(prior / 1e6).toFixed(1)} MB (${pct(num(r.m_worker), prior)})`;
      });
      return { value: 'prior', measured: lines.join('; '), pass: PENDING };
    }),
  };
}

/** Runs every scenario of step2d.feature; returns the exit code. */
export function main(csvPath, root = DEFAULT_CORPUS) {
  const { rows, error } = readRuns(csvPath);
  console.log(`step2d: corpus ${root}; runs ${csvPath ?? '(none)'}: ${error ?? `${rows.length} rows`}`);
  console.log('action | equation | known | equation value | measured | result');
  const table = checks(rows, root);
  let failed = 0;
  for (const scenario of readScenarios()) {
    const check = table[scenario.title];
    let outcome;
    try {
      outcome = check ? check() : { value: '-', measured: 'no check for this scenario', pass: false };
    } catch (err) {
      outcome = { value: '-', measured: `error: ${err.message}`, pass: false };
    }
    const result = outcome.pass === PENDING ? PENDING : outcome.pass === true ? PASS : FAIL;
    if (result === FAIL) failed += 1;
    console.log(`${scenario.title} | ${scenario.equation} | ${scenario.known} | ${outcome.value} | ${outcome.measured} | ${result}`);
  }
  console.log(failed ? `step2d: ${failed} failed` : 'step2d: all passed');
  return failed ? 1 : 0;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  process.exit(main(process.argv[2], process.argv[3]));
}
