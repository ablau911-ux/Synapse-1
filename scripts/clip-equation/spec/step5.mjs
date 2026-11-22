// Experiments step 5: the checks of scripts/clip-equation/spec/step5.feature.
// Spec: docs/clip-recorder-experiments-plan.md, "Method" 4 (J), "Procedure" 5, "Acceptance (BDD)"
// step 5, "Choosing the configuration".
//
//   node run-spec.mjs step5 [results/fit.json]
//
// Reads the input CSVs named in fit.json itself, results/results.csv and the results file
// written by fit/results.mjs. It regroups the CSV rows into configurations, recomputes bytes per
// minute, J per corpus folder, the Pareto front by brute force and the best way, and compares.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { readRuns } from './step2d.mjs';
import { relErr, runFeature } from './feature.mjs';
import { CONFIG, keyOf, loadInputs } from './step4.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const BASE = path.resolve(HERE, '..');
const FEATURE = path.join(HERE, 'step5.feature');
const RESULTS_MODULE = path.join(BASE, 'fit', 'results.mjs');
const num = (v) => (v === undefined || v === '' ? NaN : Number(v));
const mean = (a) => a.reduce((s, v) => s + v, 0) / a.length;

/** t of a corpus folder in seconds, from its meta.json. */
function clipSeconds(folder) {
  const meta = JSON.parse(fs.readFileSync(path.join(CONFIG.corpus_root, folder, 'meta.json'), 'utf8'));
  return (meta.instants * meta.instant_ms) / 1000;
}

/** N_new of a corpus folder: the lines of its frames.jsonl marked new (never the run's n_new column, which a capped pilot run sets to the cap). */
function corpusNNew(folder) {
  const text = fs.readFileSync(path.join(CONFIG.corpus_root, folder, 'frames.jsonl'), 'utf8');
  return text.split('\n').filter((l) => l.includes('"new":true')).length;
}

/** The CSV rows grouped by configuration key, with bytes per minute recomputed here. */
function groupInputs(inputs) {
  const groups = new Map();
  const add = (kind, r, bytes, frames) => {
    const key = keyOf(kind, r);
    const g = groups.get(key) ?? { key, folder: r.folder, rows: 0, bytes: [], frames, nNew: corpusNNew(r.folder) };
    g.rows += 1;
    if (Number.isFinite(bytes)) g.bytes.push(bytes);
    groups.set(key, g);
  };
  for (const r of inputs.z) add('Z', r, num(r.bytes_out), num(r.frames));
  for (const r of inputs.d) add('D', r, r.status === 'ok' ? num(r.bytes) : NaN, num(r.n_new));
  for (const g of groups.values()) g.bytesMin = g.bytes.length ? mean(g.bytes) * (g.nNew / g.frames) * 60 / clipSeconds(g.folder) : NaN;
  return groups;
}

/** results.csv as rows of objects. */
function readResultsCsv(file) {
  return fs.existsSync(file) ? readRuns(file).rows : null;
}

/** Brute-force Pareto front on bytes, CPU and RAM (all minimised) within each folder. */
function bruteFront(rows) {
  const dominates = (b, a) => b.b <= a.b && b.c <= a.c && b.m <= a.m && (b.b < a.b || b.c < a.c || b.m < a.m);
  return rows.map((a) => !rows.some((b) => b !== a && dominates(b, a)));
}

function checks(ctx) {
  const { fit, csvRows, md, groups, R, error } = ctx;
  const need = (fn) => () => (error ? { value: '-', measured: error, pass: false } : fn());
  const kept = () => csvRows.filter((r) => r.O === 'kept');
  const byFolder = (rows) => rows.reduce((m, r) => m.set(r.folder, [...(m.get(r.folder) ?? []), r]), new Map());
  return {
    'rows equal configurations': need(() => {
      const tableRows = (md.split('<!-- table:configurations:start -->')[1] ?? '').split('<!-- table:configurations:end -->')[0]
        .split(/\r?\n/).filter((l) => l.startsWith('| ')).length - 1;
      return { value: groups.size, measured: `results.csv ${csvRows.length}; results file table ${tableRows}`, pass: csvRows.length === groups.size && tableRows === groups.size };
    }),
    'every CSV row present': need(() => {
      const keys = new Set(csvRows.map((r) => r.key));
      const missing = [...groups.keys()].filter((k) => !keys.has(k));
      const runs = csvRows.reduce((s, r) => s + num(r.runs), 0);
      const total = [...groups.values()].reduce((s, g) => s + g.rows, 0);
      const wrong = csvRows.filter((r) => groups.get(r.key)?.rows !== num(r.runs)).map((r) => r.key);
      return { value: total, measured: `sum(runs) ${runs}; missing ${missing.length}; wrong counts ${wrong.length}${missing.length ? ` (${missing.slice(0, 3).join(', ')})` : ''}`, pass: runs === total && missing.length === 0 && wrong.length === 0 };
    }),
    'per-minute values': need(() => {
      const bad = csvRows.filter((r) => {
        const want = groups.get(r.key)?.bytesMin;
        const have = num(r.bytes_min);
        return Number.isFinite(want) ? !(relErr(have, want) <= 1e-9) : Number.isFinite(have);
      }).map((r) => r.key);
      return { value: 'mean(bytes) * N_new / frames * 60 / t', measured: `${csvRows.length - bad.length}/${csvRows.length} rows equal${bad.length ? `; misses: ${bad.slice(0, 3).join(', ')}` : ''}`, pass: bad.length === 0 };
    }),
    'J for every kept row': need(() => {
      const bad = [];
      for (const [folder, rows] of byFolder(kept())) {
        const star = { b: Math.min(...rows.map((r) => num(r.bytes_min))), c: Math.min(...rows.map((r) => num(r.cpu_s_min))), m: Math.min(...rows.map((r) => num(r.ram_peak))) };
        for (const r of rows) {
          const j = num(r.bytes_min) / star.b + num(r.cpu_s_min) / star.c + num(r.ram_peak) / star.m;
          if (!(relErr(num(r.J), j) <= CONFIG.j_tolerance)) bad.push(`${folder} ${r.key}: ${r.J} != ${j}`);
        }
      }
      const dropped = csvRows.filter((r) => r.O !== 'kept');
      const badDropped = dropped.filter((r) => r.J !== '' || !r.clause).map((r) => r.key);
      return { value: 'J = bytes/bytes* + CPU/CPU* + RAM/RAM*', measured: `${kept().length} kept with J (${bad.length} wrong); ${dropped.length} dropped, ${badDropped.length} without clause or with J${bad.length ? `; misses: ${bad.slice(0, 3).join('; ')}` : ''}`, pass: bad.length === 0 && badDropped.length === 0 && csvRows.length > 0 };
    }),
    'J* = 3': need(() => {
      const synthetic = [
        { folder: 'f', bytes: 1234567, cpu: 0.873, ram: 5.2e6 },
        { folder: 'f', bytes: 2234567, cpu: 0.9, ram: 7.1e6 },
        { folder: 'f', bytes: 1300000, cpu: 3.2, ram: 5.3e6 },
      ];
      const js = R.costJ(synthetic);
      const real = [];
      for (const [folder, rows] of byFolder(kept())) {
        const best = rows.reduce((a, b) => (num(b.J) < num(a.J) ? b : a));
        const leastAll = ['bytes_min', 'cpu_s_min', 'ram_peak'].every((c) => rows.every((r) => num(best[c]) <= num(r[c])));
        if (leastAll) real.push(`${folder}: ${best.J}`);
        if (leastAll && num(best.J) !== 3) return { value: 3, measured: `${folder}: least on all three but J ${best.J}`, pass: false };
      }
      return { value: 3, measured: `synthetic J = ${js[0]}; real folders with a row least on all three: ${real.join('; ') || 'none'}`, pass: js[0] === 3 && js[1] > 3 && js[2] > 3 };
    }),
    'Pareto front': need(() => {
      let front = 0;
      const bad = [];
      for (const [folder, rows] of byFolder(kept())) {
        const flags = bruteFront(rows.map((r) => ({ b: num(r.bytes_min), c: num(r.cpu_s_min), m: num(r.ram_peak) })));
        rows.forEach((r, i) => { if ((r.pareto === '1') !== flags[i]) bad.push(`${folder} ${r.key}`); front += flags[i] ? 1 : 0; });
      }
      return { value: 'non-dominated set', measured: `${front} front rows over ${byFolder(kept()).size} folders; ${bad.length} flags differ${bad.length ? `: ${bad.slice(0, 3).join(', ')}` : ''}`, pass: bad.length === 0 && kept().length > 0 };
    }),
    'best way and its equations': need(() => {
      const best = R.bestWay(kept().map((r) => ({ cfg: r.cfg, folder: r.folder, J: num(r.J) })), csvRows.map((r) => ({ cfg: r.cfg, folder: r.folder, kept: r.O === 'kept' })));
      const named = /Best way \(least J\): `([^`]+)`/.exec(md)?.[1];
      const eqs = (md.split('## Equations of the best way')[1] ?? '').split(/\r?\n## /)[0];
      const eqLines = eqs.split(/\r?\n/).filter((l) => l.includes('=')).length;
      return { value: best?.cfg ?? 'none', measured: `results file names ${named ?? 'nothing'} (mean J ${best?.score?.toFixed(4)} over ${best?.folders?.length} folders); ${eqLines} equation lines`, pass: !!best && named === best.cfg && eqLines > 0 };
    }),
    'results file header': need(() => {
      const head = md.split('\n## ')[0];
      const pilot = [...fit.inputs.z, ...fit.inputs.d].some((i) => /pilot/i.test(path.basename(i.path)));
      const named = [...fit.inputs.z, ...fit.inputs.d].every((i) => head.includes(`${path.basename(i.path)} (${i.rows} rows)`));
      return { value: pilot ? 'PILOT, not final' : 'full', measured: `inputs named with rows: ${named}; PILOT label: ${head.includes('PILOT, not final')}`, pass: named && head.includes('PILOT, not final') === pilot };
    }),
  };
}

/** Runs every scenario of step5.feature; returns the exit code. */
export async function main(fitPath = path.join(BASE, CONFIG.fit_out)) {
  const ctx = { error: '' };
  try { ctx.R = await import(pathToFileURL(RESULTS_MODULE).href); } catch (err) { ctx.error = `${path.relative(BASE, RESULTS_MODULE)}: ${err.message.split('\n')[0]}`; }
  const csvFile = path.join(BASE, CONFIG.results_csv);
  const mdFile = path.resolve(BASE, CONFIG.results_md);
  if (!fs.existsSync(fitPath)) ctx.error += `; no fit file at ${fitPath}`;
  else ctx.fit = JSON.parse(fs.readFileSync(fitPath, 'utf8'));
  ctx.csvRows = readResultsCsv(csvFile);
  if (!ctx.csvRows) ctx.error += `; no ${path.relative(BASE, csvFile)}`;
  ctx.md = fs.existsSync(mdFile) ? fs.readFileSync(mdFile, 'utf8') : null;
  if (ctx.md === null) ctx.error += `; no ${mdFile}`;
  if (ctx.fit) ctx.groups = groupInputs(loadInputs(ctx.fit));
  ctx.error = ctx.error.replace(/^; /, '');
  console.log(`step5: fit ${fitPath}; results ${mdFile}`);
  return runFeature('step5', FEATURE, checks(ctx));
}
