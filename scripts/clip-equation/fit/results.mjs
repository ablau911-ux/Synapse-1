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

/**
 * The equations of one configuration with its numbers from the persisted fitted models (#21):
 * zeta(px) = zeta(px0) * (px / px0)^(eta - 1) of its size group when eta is fitted, else the
 * configuration's own measured zeta at its one size; kappa_c and c_0 of the fitted CPU model.
 */
export function bestEquations(fit, c) {
  const eq = fit.equations;
  if (c.kind === 'Z') {
    const mode = c.options.mode ?? '';
    const zm = fit.models?.z_bytes?.[mode];
    const lz = zm?.identifiable ? zm.ln_zeta_px0?.[c.group] : null;
    const modelled = typeof lz === 'number' && Number.isFinite(lz);
    const zeta = modelled ? Math.exp(lz + (zm.eta - 1) * Math.log(c.px / zm.px0)) : c.fit.zeta;
    const cm = fit.models?.z_cpu?.[mode];
    const kappa = cm?.kappa_ns_per_px?.[c.group] ?? c.fit.comp_ns_per_frame.mean / c.px;
    const c0 = cm?.identifiable ? cm.c0_ns : undefined;
    const f = c.fit;
    return [
      modelled
        ? `- Bytes (eq. 1): b_k = ζ · px · 3, ζ(px) = ζ(px0) · (px / px0)^(η − 1), ζ(px0) = ${fmt(Math.exp(lz), 6)} (px0 = ${zm.px0}), η = ${fmt(zm.eta, 4)} ⇒ ζ = ${fmt(zeta, 6)} at ${c.size} (px = ${c.px})`
        : `- Bytes (eq. 1): b_k = ζ · px · 3, ζ = ${fmt(zeta, 6)} measured at ${c.size} (px = ${c.px}); η = pending: full run (needs two sizes)`,
      `- Bytes per minute: B_min = 60 · (N_new · ζ · px · 3 + metadata) / t = 60 · (${c.n_new} · ${fmt(zeta, 6)} · ${c.px} · 3 + ${fmt(c.fit.meta ?? 0, 0)}) / ${c.t_s} = ${mb((60 * (c.n_new * zeta * c.px * 3 + (c.fit.meta ?? 0))) / c.t_s)} MB/min (metadata: frames.jsonl and clip.json)`,
      `- Quality (eq. 2): Σ |decoded − corpus| = 0 ⇒ SSIM-Y = 1, PSNR-Y = ∞, R = 1`,
      `- CPU (eq. 3): C_comp / N_new = c_0 + px · κ_c, κ_c = ${fmt(kappa, 4)} ns/px, c_0 = ${c0 === undefined ? 'pending: full run (needs two sizes)' : `${fmt(c0, 0)} ns`}; measured with writing: ${fmt(c.cpu_s_min.mean, 4)} s/min`,
      (() => {
        const mu = eq.eq4_z_ram.coefficients.mu_train;
        const train = f.s_train > 0 ? (mu === undefined ? NaN : mu * f.s_train) : 0;
        const term = f.s_train > 0 ? ` + μ · S_samples (μ = ${mu === undefined ? 'pending: full run' : fmt(mu, 4)}, S_samples = ${f.s_train})` : '';
        return `- RAM (eq. 4): M_comp = M_0 + (M_cctx + B_c · px · 3, as computed by the run)${term} = ${mb(f.m0)} + ${mb(f.m_eq)}${f.s_train > 0 ? ` + ${Number.isFinite(train) ? mb(train) : 'pending'}` : ''} = ${Number.isFinite(train) ? mb(f.m0 + f.m_eq + train) : 'pending'} MB; measured peak ${mb(f.m0 + f.ram_os.mean)} MB`;
      })(),
    ];
  }
  const g = c.content;
  const m = fit.models?.d_bytes ?? {};
  const apart = m.b_E?.[c.way] !== undefined;
  const b = { a_E: apart ? m.a_E[c.way] : m.a_E_folded?.[c.way], 'p_E,c': apart ? m.p_E_c?.[c.way] : m.p_E_c_folded?.[c.way], rho: m.K_over_P_at_q?.[c.way]?.[`${g}|q${c.options.q}`] };
  const s = eq.eq7_d_quality.coefficients.Q_ssim?.[c.way];
  return [
    `- Bytes (eq. 7): bytes = (n_new − n_key) · P + n_key · K, n_key = ⌈n_new / τ_f⌉ = ${Math.ceil(c.n_new / Number(c.options.tau_f))}; P = p_E,c · px · exp(a_E · q), p = ${fmt(b['p_E,c']?.[g], 6)}, a = ${fmt(b.a_E, 5)}; K / P = ${apart ? `apart: K = k_E,c · px · exp(b_E · q), b_E = ${fmt(m.b_E[c.way], 5)}` : b.rho === undefined ? `pending: full run (no GOP sweep at q ${c.options.q}; P fitted at τ_f ${CONFIG.d_base.tau_f} with K folded in)` : `${fmt(b.rho, 4)} measured at q ${c.options.q}`}; bytes on disk add the metadata ${mb(c.fit.meta ?? NaN)} MB (frames.jsonl, clip.json)`,
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
    `Failures kept (plan M10: a configuration needs all of its repeats): way Z ${fit.failures?.z?.runs_failed ?? 0} failed runs, ${fit.failures?.z?.configs_blocked ?? 0} configurations blocked; way D ${fit.failures?.d?.runs_failed ?? 0} failed runs, ${fit.failures?.d?.configs_blocked ?? 0} configurations blocked (failed or refused repeat, or R not read).${fit.coverage ? ` Coverage of the run plans: Z ${fit.coverage.z.unique} unique keys of ${fit.coverage.z.planned} planned (${fit.coverage.z.missing} missing, ${fit.coverage.z.duplicates} duplicates); D ${fit.coverage.d_plan === 'none' ? 'no run plan given' : `${fit.coverage.d.unique} of ${fit.coverage.d.planned} (${fit.coverage.d.missing} missing, ${fit.coverage.d.duplicates} duplicates)`}.` : ''}`,
    '',
    `Checks (plan M10, #295 H6; \`fit/fit-config.json\` "checks"). Validity checks block a run from the fit and from J: a status other than ${CONFIG.checks.ok_statuses.join(', ')} (setting_refused, frame_map, ocr_error, error), a missing repeat of the requested configuration, and the measurement-rule rows ${Object.entries(CONFIG.checks.validity_pass).map(([k, v]) => `${k}: ${v.join(', ')}`).join('; ')}. Every other pass_* row is a known-result equation: a miss is a fact about the way, listed in the configuration table as "known-result misses", and never blocks. Configurations with a known-result miss: ${rows.filter((r) => r.c.known_misses?.length).length}.`,
    '',
    'Definitions. bytes/min = (payload · N_new / frames + metadata) · 60 / t, from bytes_disk (every retained file: frames or MP4, frames.jsonl, clip.json; M4) for both ways (t = 20 s; frames < N_new only in the pilot\'s capped runs). CPU = process cycle-counter CPU to create the clip, in s per minute of clip: way Z compress + write, way D live + worker (a_rt + a_w). RAM = peak private bytes above the process base: way Z peak − M_0, way D M_live + M_worker. Spread = half the range of the repeats. J = bytes/bytes* + CPU/CPU* + RAM/RAM*, the stars being the least values among the kept configurations of the same corpus folder (the same frames).',
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
    out.push(`| ${c.way} | ${cell(optText(c))}${c.capped ? ` (${c.frames} of ${c.n_new} frames)` : ''}${c.held_out ? ' (held out)' : ''} | ${c.size} | ${c.content} | ${c.bytes_min === undefined ? '-' : mb(c.bytes_min)} | ${q} | ${cpu} | ${ram} | ${c.O ? 'kept' : `dropped: ${cell(c.clause)}`}${c.known_misses?.length ? `; known-result misses: ${cell(c.known_misses.join(' '))}` : ''} | ${r.J === null ? '-' : fmt(r.J, 4)} |`);
  }
  out.push('<!-- table:configurations:end -->', '');
  out.push('## Predicted cells (run list, not run)', '', ...predictedLines(fit.predicted));
  return out.join('\n');
}

function csv(rows, predicted) {
  const cols = ['row_type', 'prediction', 'kind', 'key', 'cfg', 'way', 'options', 'folder', 'content', 'size', 'px', 'runs', 'failed_runs', 'frames', 'n_new', 't_s', 'bytes_min', 'ssim', 'psnr', 'r', 'cpu_s_min', 'cpu_spread', 'ram_peak', 'ram_spread', 'O', 'clause', 'J', 'pareto', 'held_out', 'cell', 'bytes_rel_unc', 'cpu_rel_unc', 'ram_rel_unc', 'known_misses'];
  const v = (x) => (x === undefined || x === null || Number.isNaN(x) ? '' : String(x).replace(/[,\r\n]/g, ';'));
  const lines = rows.map(({ c, J, pareto }) => ['measured', '', c.kind, c.key, c.cfg, c.way, optText(c), c.folder, c.content, c.size, c.px, c.runs, c.failed_runs, c.frames, c.n_new, c.t_s, c.bytes_min, c.ssim, c.psnr, c.r,
    c.cpu_s_min?.mean, c.cpu_s_min?.spread, c.ram_peak?.mean, c.ram_peak?.spread, c.O ? 'kept' : 'dropped', c.clause, J, pareto ? 1 : 0, c.held_out ? 1 : 0, c.cell ?? '', '', '', '', (c.known_misses ?? []).join(' ')].map(v).join(','));
  const pred = (predicted?.rows ?? []).map((p) => ['predicted', p.status, 'Z', '', '', p.way, p.options, p.folder, p.content, p.size, p.px, 0, '', '', '', '', p.bytes_min, '', '', '',
    p.cpu_s_min, '', p.ram, '', '', '', '', '', '', p.cell, p.bytes_rel_unc, p.cpu_rel_unc, p.ram_rel_unc].map(v).join(','));
  return `${cols.join(',')}\n${[...lines, ...pred].join('\n')}\n`;
}

/** The predicted cells section: the rows with numbers listed, the pending ones counted by reason. */
function predictedLines(predicted) {
  const rows = predicted?.rows ?? [];
  if (!rows.length) return ['No run list cell is marked predicted.', ''];
  const done = rows.filter((r) => r.status === 'predicted');
  const pending = new Map();
  for (const r of rows.filter((x) => x.status !== 'predicted')) pending.set(r.status, (pending.get(r.status) ?? 0) + (r.content === 'all' ? predicted.contents.length : 1));
  const out = [`Run list cells not run, predicted by the fitted size equations (uncertainty: ${predicted.uncertainty}; ${predicted.note}). All rows are in \`scripts/clip-equation/results/results.csv\` (row_type predicted).`, ''];
  for (const [status, n] of pending) out.push(`- ${status}: ${n} cell × content`);
  out.push(`- predicted: ${done.length} cell × content`, '');
  if (done.length) {
    out.push('| cell | way | options | folder | bytes/min (MB) ± | CPU s/min ± | RAM (MB) ± |', '|---|---|---|---|---|---|---|');
    for (const r of done) out.push(`| ${r.cell} | ${r.way} | ${cell(r.options)} | ${r.folder} | ${mb(r.bytes_min)} ± ${(100 * r.bytes_rel_unc).toFixed(1)} % | ${fmt(r.cpu_s_min, 4)} ± ${(100 * r.cpu_rel_unc).toFixed(1)} % | ${typeof r.ram === 'number' ? `${mb(r.ram)} ± ${(100 * r.ram_rel_unc).toFixed(1)} %` : 'pending'} |`);
    out.push('');
  }
  return out;
}

export function buildResults(fit) {
  const rows = fit.configurations.map((c) => ({ c, J: null, pareto: false }));
  const kept = rows.filter((r) => r.c.O);
  const metrics = kept.map((r) => ({ folder: r.c.folder, bytes: r.c.bytes_min, cpu: r.c.cpu_s_min.mean, ram: r.c.ram_peak.mean }));
  const js = costJ(metrics);
  const front = paretoFlags(metrics);
  kept.forEach((r, i) => { r.J = js[i]; r.pareto = front[i]; });
  const best = bestWay(kept.map((r) => ({ cfg: r.c.cfg, folder: r.c.folder, J: r.J })), rows.map((r) => ({ cfg: r.c.cfg, folder: r.c.folder, kept: !!r.c.O })));
  return { rows, best, md: markdown(fit, rows, best), csv: csv(rows, fit.predicted) };
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
