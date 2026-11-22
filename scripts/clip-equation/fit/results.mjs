#!/usr/bin/env node
// Experiments step 5: the results file from results/fit.json.
//
//   node scripts/clip-equation/fit/results.mjs [results/fit.json]
//
// Writes docs/clip-recorder-experiments-results.md and results/results.csv: one row per
// configuration, J for every kept row, the Pareto front of bytes, CPU and RAM, every equation
// with its coefficients, the validation table, and the best way by least J with its equations.
// J and the front are taken within one corpus folder, because only runs on the same frames are
// comparable (plan, "How absolute and predictable is reached" 1).
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const BASE = path.resolve(HERE, '..');
const CONFIG = JSON.parse(fs.readFileSync(path.join(HERE, 'fit-config.json'), 'utf8'));
const LARGE_MAP = 40;

/** J = bytes / bytes* + CPU / CPU* + RAM / RAM*, the stars being the least values of the same folder. */
export function costJ(rows) {
  const star = new Map();
  for (const r of rows) {
    const s = star.get(r.folder) ?? { b: Infinity, c: Infinity, m: Infinity };
    star.set(r.folder, { b: Math.min(s.b, r.bytes), c: Math.min(s.c, r.cpu), m: Math.min(s.m, r.ram) });
  }
  return rows.map((r) => { const s = star.get(r.folder); return r.bytes / s.b + r.cpu / s.c + r.ram / s.m; });
}

/** True for each row that no row of the same folder dominates on bytes, CPU and RAM. */
export function paretoFlags(rows) {
  const dominates = (b, a) => b.bytes <= a.bytes && b.cpu <= a.cpu && b.ram <= a.ram && (b.bytes < a.bytes || b.cpu < a.cpu || b.ram < a.ram);
  return rows.map((a) => !rows.some((b) => b !== a && b.folder === a.folder && dominates(b, a)));
}

/**
 * The best way: among configurations kept in every folder they ran in, the least mean J over the
 * folders they all share; when they share none, over the folders shared by those that ran in the
 * most folders. kept: [{ cfg, folder, J }]; all: [{ cfg, folder, kept }].
 */
export function bestWay(kept, all) {
  const dropped = new Set(all.filter((r) => !r.kept).map((r) => r.cfg));
  const folders = new Map();
  for (const r of kept) if (!dropped.has(r.cfg)) folders.set(r.cfg, new Set([...(folders.get(r.cfg) ?? []), r.folder]));
  if (!folders.size) return null;
  const shared = (cfgs) => [...folders.get(cfgs[0])].filter((f) => cfgs.every((c) => folders.get(c).has(f)));
  let cands = [...folders.keys()];
  let common = shared(cands);
  if (!common.length) {
    const most = Math.max(...cands.map((c) => folders.get(c).size));
    cands = cands.filter((c) => folders.get(c).size === most);
    common = shared(cands);
  }
  const score = (cfg) => {
    const js = kept.filter((r) => r.cfg === cfg && (common.length ? common.includes(r.folder) : true)).map((r) => r.J);
    return js.reduce((s, v) => s + v, 0) / js.length;
  };
  const ranked = cands.map((cfg) => ({ cfg, score: score(cfg) })).sort((a, b) => a.score - b.score || a.cfg.localeCompare(b.cfg));
  return { ...ranked[0], folders: common.length ? common : [...folders.get(ranked[0].cfg)], ranked };
}

const fmt = (v, d = 3) => (v === null || v === undefined || Number.isNaN(v) ? '-' : typeof v === 'number' ? (Math.abs(v) >= 1e5 || (Math.abs(v) < 1e-3 && v !== 0) ? v.toExponential(d) : Number(v.toFixed(d)).toString()) : String(v));
const mb = (v) => (v / 1e6).toFixed(2);
const optText = (c) => Object.entries(c.options).map(([k, v]) => `${k}=${v}`).join(' ');
const cell = (s) => String(s).replace(/\|/g, '/').replace(/,/g, ';');

/** Coefficients as markdown lines; a map larger than LARGE_MAP entries is counted, not listed. */
function coefficientLines(obj, indent = '') {
  return Object.entries(obj ?? {}).flatMap(([k, v]) => {
    if (v && typeof v === 'object') {
      const n = Object.keys(v).length;
      if (n > LARGE_MAP) return [`${indent}- ${k}: ${n} values (all in \`scripts/clip-equation/results/fit.json\`)`];
      return [`${indent}- ${k}:`, ...coefficientLines(v, `${indent}  `)];
    }
    return [`${indent}- ${k} = ${fmt(v, 6)}`];
  });
}

/** The equations of one configuration with its fitted numbers. */
function bestEquations(fit, c) {
  const eq = fit.equations;
  if (c.kind === 'Z') {
    const g = `${c.cfg}|${c.content}`;
    const zeta = eq.eq1_z_bytes.coefficients.zeta[`${g}@${c.size}`];
    const eta = eq.eq1_z_bytes.coefficients.eta?.[c.options.mode];
    const kappa = eq.eq3_z_cpu.coefficients.kappa_c_ns_per_px[g];
    const c0 = eq.eq3_z_cpu.coefficients.c_0_ns?.[c.options.mode];
    const f = c.fit;
    return [
      `- Bytes (eq. 1): b_k = ζ · px · 3, ζ = ${fmt(zeta, 6)} at ${c.size} (px = ${c.px}); ζ(px) = ζ · (px / ${c.px})^(η − 1), η = ${eta === undefined ? 'pending: full run (needs two sizes)' : fmt(eta, 4)}`,
      `- Bytes per minute: B_min = 60 · n_new · ζ · px · 3 = 60 · ${fmt(c.n_new / c.t_s, 3)} · ${fmt(zeta, 6)} · ${c.px} · 3 = ${mb(60 * (c.n_new / c.t_s) * zeta * c.px * 3)} MB/min`,
      `- Quality (eq. 2): Σ |decoded − corpus| = 0 ⇒ SSIM-Y = 1, PSNR-Y = ∞, R = 1`,
      `- CPU (eq. 3): C_comp / N_new = c_0 + px · κ_c, κ_c = ${fmt(kappa, 4)} ns/px, c_0 = ${c0 === undefined ? 'pending: full run (needs two sizes)' : `${fmt(c0, 0)} ns`}; measured with writing: ${fmt(c.cpu_s_min.mean, 4)} s/min`,
      `- RAM (eq. 4): M_comp = M_0 + M_cctx + B_c · px · 3 = ${mb(f.m0)} + ${mb(f.m_cctx)} + ${f.bc} · ${c.px} · 3 = ${mb(f.known_m_comp)} MB; measured peak ${mb(f.peak_private.mean)} MB`,
    ];
  }
  const g = c.content;
  const b = eq.eq7_d_bytes.coefficients.P_at_tau20?.[c.way];
  const s = eq.eq7_d_quality.coefficients.Q_ssim?.[c.way];
  return [
    `- Bytes (eq. 7): P = p_E,c · px · exp(a_E · q), p = ${fmt(b?.['p_E,c']?.[g], 6)}, a = ${fmt(b?.a_E, 5)}`,
    `- Quality (eq. 7): Q_ssim = 1 − s · exp(−g · q), s = ${fmt(s?.['s_E,c']?.[g], 5)}, g = ${fmt(s?.g_E, 5)}`,
    `- CPU and RAM (eq. 7): see the equations section (${c.way})`,
  ];
}

function markdown(fit, rows, best) {
  const inputs = [...fit.inputs.z, ...fit.inputs.d].map((i) => `\`${path.basename(i.path)} (${i.rows} rows)\``).join(', ');
  const kept = rows.filter((r) => r.c.O);
  const zN = rows.filter((r) => r.c.kind === 'Z').length;
  const dRows = rows.filter((r) => r.c.kind === 'D');
  const out = [
    `# Clip recorder: experiment results${fit.label === 'FULL' ? '' : ` — ${fit.label}`}`,
    '',
    `Status ${fit.created.slice(0, 16).replace('T', ' ')} UTC: **${fit.label}**. Built from ${inputs}; run list \`${path.basename(fit.inputs.runlist)}\`. Generated by \`scripts/clip-equation/fit/fit.mjs\` and \`fit/results.mjs\`; checked by \`spec/run-spec.mjs step4\` and \`step5\`. Plan: \`docs/clip-recorder-experiments-plan.md\`.`,
    '',
    `Configurations: ${rows.length} (way Z ${zN}, way D ${dRows.length}); kept (O = [R = 1]): ${kept.length}; way D kept: ${dRows.filter((r) => r.c.O).length} of ${dRows.length}.`,
    '',
    'Definitions. bytes/min = mean(bytes) · (N_new / frames) · 60 / t (t = 20 s; frames < N_new only in the pilot\'s capped runs). CPU = process cycle-counter CPU to create the clip, in s per minute of clip: way Z compress + write, way D live + worker (a_rt + a_w). RAM = peak private bytes above the process base: way Z peak − M_0, way D M_live + M_worker. Spread = half the range of the repeats. J = bytes/bytes* + CPU/CPU* + RAM/RAM*, the stars being the least values among the kept configurations of the same corpus folder (the same frames).',
    '',
    '## Best way',
    '',
  ];
  if (best) {
    const c = rows.find((r) => r.c.cfg === best.cfg)?.c;
    out.push(`Best way (least J): \`${best.cfg}\` — ${c.way} ${optText(c)}; mean J ${fmt(best.score, 4)} over ${best.folders.length} folder(s): ${best.folders.join(', ')}.`, '');
    out.push('Runner-up configurations by mean J:', '', ...best.ranked.slice(1, 6).map((r) => `- \`${r.cfg}\`: ${fmt(r.score, 4)}`), '');
    out.push('## Equations of the best way', '');
    for (const folder of best.folders) {
      const r = rows.find((x) => x.c.cfg === best.cfg && x.c.folder === folder);
      out.push(`${folder}:`, '', ...bestEquations(fit, r.c), '');
    }
  } else out.push('Best way (least J): `none` — no configuration is kept.', '');
  out.push('## Pareto front (bytes, CPU, RAM) per corpus folder', '', '| folder | way | options | bytes/min (MB) | CPU s/min | RAM (MB) | J |', '|---|---|---|---|---|---|---|');
  for (const r of kept.filter((x) => x.pareto).sort((a, b) => a.c.folder.localeCompare(b.c.folder) || a.J - b.J)) {
    out.push(`| ${r.c.folder} | ${r.c.way} | ${cell(optText(r.c))} | ${mb(r.c.bytes_min)} | ${fmt(r.c.cpu_s_min.mean, 4)} | ${mb(r.c.ram_peak.mean)} | ${fmt(r.J, 4)} |`);
  }
  out.push('', '## Equations with their fitted coefficients', '');
  for (const [id, e] of Object.entries(fit.equations)) {
    out.push(`### ${e.title} (\`${id}\`)`, '', `    ${e.equation}`, '');
    out.push(...coefficientLines(e.coefficients));
    if (e.residuals) out.push(`- residuals: n ${e.residuals.n}, max ${(100 * e.residuals.max_abs_rel).toFixed(2)} %, rms ${(100 * e.residuals.rms_rel).toFixed(2)} %${e.residuals.max_abs_ssim !== undefined ? `, max SSIM error ${fmt(e.residuals.max_abs_ssim, 5)}` : ''}${e.residuals.note ? ` (${e.residuals.note})` : ''}`);
    for (const p of e.pending ?? []) out.push(`- pending: full run: ${p.part} (${p.reason})`);
    out.push('');
  }
  out.push('## Validation (held-out configurations, never in a fit set, never refitted)', '');
  out.push(`Way Z: ${fit.validation.z.held_out_keys.length} held-out configurations (run list held-out cells: ${fit.validation.z.runlist_held_out}). Way D: ${fit.validation.d.held_out_keys.length} of ${fit.validation.d.eligible} configurations, ${fit.validation.d.generator} seed ${fit.validation.d.seed}, share ${fit.validation.d.share}. Targets: bytes, CPU, RAM within ±${100 * CONFIG.tolerance.bytes} %, SSIM within ±${CONFIG.tolerance.ssim}.`, '');
  out.push('| configuration | quantity | predicted | measured | error | spread | verdict |', '|---|---|---|---|---|---|---|');
  for (const v of [...fit.validation.z.rows, ...fit.validation.d.rows]) {
    out.push(`| ${cell(v.key)} | ${v.quantity} | ${fmt(v.predicted, 5)} | ${fmt(v.measured, 5)} | ${v.error === undefined ? '-' : v.quantity === 'ssim' ? fmt(v.error, 5) : `${(100 * v.error).toFixed(1)} %`} | ${fmt(v.spread, 5)} | ${v.verdict} |`);
  }
  out.push('', '## Every configuration', '', '<!-- table:configurations:start -->');
  out.push('| way | options | size | content | bytes/min (MB) | SSIM / PSNR / R | CPU s/min (mean ± spread) | RAM peak MB (mean ± spread) | O | J |', '|---|---|---|---|---|---|---|---|---|---|');
  for (const r of rows) {
    const c = r.c;
    const q = c.status === 'ok' ? `${fmt(c.ssim, 5)} / ${fmt(c.psnr, 2)} / ${fmt(c.r, 4)}` : '-';
    const cpu = c.cpu_s_min ? `${fmt(c.cpu_s_min.mean, 4)} ± ${fmt(c.cpu_s_min.spread, 4)}` : '-';
    const ram = c.ram_peak ? `${mb(c.ram_peak.mean)} ± ${mb(c.ram_peak.spread)}` : '-';
    out.push(`| ${c.way} | ${cell(optText(c))}${c.capped ? ` (${c.frames} of ${c.n_new} frames)` : ''}${c.held_out ? ' (held out)' : ''} | ${c.size} | ${c.content} | ${c.bytes_min === undefined ? '-' : mb(c.bytes_min)} | ${q} | ${cpu} | ${ram} | ${c.O ? 'kept' : `dropped: ${cell(c.clause)}`} | ${r.J === null ? '-' : fmt(r.J, 4)} |`);
  }
  out.push('<!-- table:configurations:end -->', '');
  return out.join('\n');
}

function csv(rows) {
  const cols = ['kind', 'key', 'cfg', 'way', 'options', 'folder', 'content', 'size', 'px', 'runs', 'frames', 'n_new', 't_s', 'bytes_min', 'ssim', 'psnr', 'r', 'cpu_s_min', 'cpu_spread', 'ram_peak', 'ram_spread', 'O', 'clause', 'J', 'pareto', 'held_out'];
  const v = (x) => (x === undefined || x === null || Number.isNaN(x) ? '' : String(x).replace(/[,\r\n]/g, ';'));
  const lines = rows.map(({ c, J, pareto }) => [c.kind, c.key, c.cfg, c.way, optText(c), c.folder, c.content, c.size, c.px, c.runs, c.frames, c.n_new, c.t_s, c.bytes_min, c.ssim, c.psnr, c.r,
    c.cpu_s_min?.mean, c.cpu_s_min?.spread, c.ram_peak?.mean, c.ram_peak?.spread, c.O ? 'kept' : 'dropped', c.clause, J, pareto ? 1 : 0, c.held_out ? 1 : 0].map(v).join(','));
  return `${cols.join(',')}\n${lines.join('\n')}\n`;
}

export function buildResults(fit) {
  const rows = fit.configurations.map((c) => ({ c, J: null, pareto: false }));
  const kept = rows.filter((r) => r.c.O);
  const metrics = kept.map((r) => ({ folder: r.c.folder, bytes: r.c.bytes_min, cpu: r.c.cpu_s_min.mean, ram: r.c.ram_peak.mean }));
  const js = costJ(metrics);
  const front = paretoFlags(metrics);
  kept.forEach((r, i) => { r.J = js[i]; r.pareto = front[i]; });
  const best = bestWay(kept.map((r) => ({ cfg: r.c.cfg, folder: r.c.folder, J: r.J })), rows.map((r) => ({ cfg: r.c.cfg, folder: r.c.folder, kept: !!r.c.O })));
  return { rows, best, md: markdown(fit, rows, best), csv: csv(rows) };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const fitPath = path.resolve(process.argv[2] ?? path.join(BASE, CONFIG.fit_out));
  const fit = JSON.parse(fs.readFileSync(fitPath, 'utf8'));
  const { rows, best, md, csv: text } = buildResults(fit);
  const mdFile = path.resolve(BASE, CONFIG.results_md);
  fs.writeFileSync(mdFile, md);
  fs.writeFileSync(path.join(BASE, CONFIG.results_csv), text);
  console.log(`results: ${fit.label}; ${rows.length} rows, ${rows.filter((r) => r.c.O).length} kept; best ${best?.cfg ?? 'none'} (mean J ${best?.score?.toFixed(4)}); written ${mdFile} and ${CONFIG.results_csv}`);
}
