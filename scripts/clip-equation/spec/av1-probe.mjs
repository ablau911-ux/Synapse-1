// Checks of the AV1 window-recording probe (docs/research/window-recording/probe-plan.md,
// "Probe interface", "Proof" and "Rules"). Runs `clip-probe exp-av1` in replay mode on four
// corpus scenes, judges each output folder against the contract, then prints the reported
// (not judged) measures from probe.json as one table.
//   node scripts/clip-equation/spec/run-spec.mjs av1-probe out=<dir> [probe=<exe>] [root=<corpus>]
import fs from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';
import crypto from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { runFeature } from './feature.mjs';

const BASE = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const options = Object.fromEntries(process.argv.slice(3).map(arg => {
  const split = arg.indexOf('=');
  return [arg.slice(0, split), arg.slice(split + 1)];
}));
const ROOT = options.root || JSON.parse(fs.readFileSync(path.join(BASE, 'corpus/corpus.json'), 'utf8')).root;
const OUT = path.resolve(options.out || path.join(BASE, '../../.runs/av1-probe'));
const PROBE = path.resolve(options.probe || path.join(BASE, 'probe/target/release/clip-probe.exe'));
const SCENES = ['typing-1920x1080', 'scroll-1920x1080', 'ui-1920x1080', 'mix-1920x1080'];
const SETTLE_SCENE = 'mix-1920x1080'; // the scene whose corpus must hold at least one settle point
const INSTANT_MS = 50;                // 20 instants per second (contract: t_ms = k*50)
const SETTLE_INSTANTS = 10;           // probe default settle=<instants>: on screen this many instants, the changed one included
const READ_BACK_MS = 500;
const MB = 1048576; // the plan's targets were measured in units of 1048576 bytes
const KINDS = ['enc', 'same', 'lost'];
const VALID_KIND_VALUE = {
  enc: v => Array.isArray(v) && v.length === 2 && v.every(n => Number.isInteger(n) && n >= 0),
  same: v => v === true,
  lost: v => typeof v === 'string',
};
const BIG_OUTPUT = 64 * 1024 * 1024;

const readJsonl = file => fs.readFileSync(file, 'utf8').split(/\r?\n/).filter(line => line.trim()).map(line => JSON.parse(line));
const pad = (n, width) => String(n).padStart(width, '0');
/** The one kind (enc, same or lost) an index line carries, or null when it has none or several. */
const kindOf = line => { const present = KINDS.filter(key => key in line); return present.length === 1 ? present[0] : null; };

/** One corpus scene: its known source pictures, the probe run on it, and the probe's output. */
class Scene {
  constructor(name) {
    this.name = name;
    this.source = path.join(ROOT, name);
    this.out = path.join(OUT, name);
    this.corpus = readJsonl(path.join(this.source, 'frames.jsonl'));
    this.error = null;
    this.lines = [];
    this.probe = null;
    this.hashes = new Map(); // corpus frame number -> SHA-256 of its pixels
  }

  /** Runs the probe on this scene and loads frames.jsonl and probe.json; keeps the first failure. */
  record() {
    try {
      if (fs.existsSync(this.out) && fs.readdirSync(this.out).length) throw new Error(`output folder not empty: ${this.out}`);
      const run = spawnSync(PROBE, ['exp-av1', `out=${this.out}`, `folder=${this.source}`], { encoding: 'utf8', maxBuffer: BIG_OUTPUT });
      if (run.error || run.status !== 0) throw new Error(run.error?.message || run.stderr.trim() || `exit ${run.status}`);
      this.lines = readJsonl(path.join(this.out, 'frames.jsonl'));
      this.probe = JSON.parse(fs.readFileSync(path.join(this.out, 'probe.json'), 'utf8'));
    } catch (error) { this.error = error.message; }
  }

  get instants() { return this.corpus.length; }

  /** SHA-256 of the B,G,R bytes shown at instant k, computed once per corpus frame number. A new
   *  frame number can hold the same pixels, so pictures are compared by content, not by number. */
  pictureHash(k) {
    const frame = this.corpus[k].frame;
    if (!this.hashes.has(frame)) this.hashes.set(frame, crypto.createHash('sha256').update(this.picture(k)).digest('hex'));
    return this.hashes.get(frame);
  }

  /** True when instant k shows a different picture than instant k-1; instant 0 counts as a change. */
  changed(k) { return k === 0 || this.pictureHash(k) !== this.pictureHash(k - 1); }

  /** How many instants directly after k show the same picture as k. */
  unchangedAfter(k) {
    let n = 0;
    while (k + 1 + n < this.instants && this.pictureHash(k + 1 + n) === this.pictureHash(k)) n += 1;
    return n;
  }

  /** The B,G,R bytes the corpus captured for instant k. */
  picture(k) {
    return zlib.zstdDecompressSync(fs.readFileSync(path.join(this.source, `frame-${pad(this.corpus[k].frame, 4)}.zst`)));
  }

  partPath(part) { return path.join(this.out, 'video', `part-${pad(part, 4)}.ivf`); }

  partFiles() {
    const dir = path.join(this.out, 'video');
    return fs.existsSync(dir) ? fs.readdirSync(dir).filter(f => /^part-\d+\.ivf$/.test(f)).sort() : [];
  }

  /** Instant numbers of the files stills/still-NNNNNNNN.webp. */
  stillInstants() {
    const dir = path.join(this.out, 'stills');
    if (!fs.existsSync(dir)) return [];
    return fs.readdirSync(dir).map(f => f.match(/^still-(\d{8})\.webp$/)).filter(Boolean).map(m => Number(m[1])).sort((a, b) => a - b);
  }

  stillPath(k) { return path.join(this.out, 'stills', `still-${pad(k, 8)}.webp`); }

  encLines() { return this.lines.filter(line => kindOf(line) === 'enc'); }
}

/** Runs `check(scene)` -> { problem?, note } on every scene; a scene whose run failed is a miss. */
function judge(check) {
  const problems = [], notes = [];
  for (const scene of scenes) {
    if (scene.error) { problems.push(`${scene.name}: ${scene.error}`); continue; }
    const { problem, note } = check(scene);
    if (problem) problems.push(`${scene.name}: ${problem}`);
    else notes.push(`${scene.name}: ${note}`);
  }
  return { pass: problems.length === 0, value: '-', measured: problems.length ? problems.join('; ') : notes.join('; ') };
}

const firstOf = (list, n = 3) => `${list.slice(0, n).join(', ')}${list.length > n ? ` (+${list.length - n} more)` : ''}`;

function everyInstant(scene) {
  if (scene.lines.length !== scene.instants) return { problem: `frames.jsonl has ${scene.lines.length} lines, scene has ${scene.instants} instants` };
  const bad = [];
  scene.lines.forEach((line, k) => {
    const kind = kindOf(line);
    if (line.instant !== k || line.t_ms !== k * INSTANT_MS) bad.push(`line ${k}: instant=${line.instant} t_ms=${line.t_ms}`);
    else if (!kind) bad.push(`instant ${k}: not exactly one of enc, same, lost`);
    else if (!VALID_KIND_VALUE[kind](line[kind])) bad.push(`instant ${k}: bad ${kind} value ${JSON.stringify(line[kind])}`);
  });
  if (bad.length) return { problem: firstOf(bad) };
  const p = scene.probe;
  if (p.instants !== scene.instants) return { problem: `probe.json instants=${p.instants}, scene has ${scene.instants}` };
  if (p.encoded + p.same + p.lost !== p.instants) return { problem: `probe.json encoded+same+lost=${p.encoded}+${p.same}+${p.lost}, instants=${p.instants}` };
  return { note: `${scene.instants} instants = ${p.encoded} enc + ${p.same} same + ${p.lost} lost` };
}

function nothingLostSilently(scene) {
  const lost = scene.lines.filter(line => kindOf(line) === 'lost');
  const unnamed = lost.filter(line => typeof line.lost !== 'string' || !line.lost.trim()).map(line => line.instant);
  if (unnamed.length) return { problem: `lost without cause at instants ${firstOf(unnamed)}` };
  const causes = [...new Set(lost.map(line => line.lost))];
  return { note: `lost=${lost.length}${causes.length ? ` (${firstOf(causes)})` : ''}` };
}

/** Decodes every picture of one part: { codec, pictures } or { error }. */
function decodePart(file) {
  const run = spawnSync('ffprobe', ['-v', 'error', '-count_frames', '-select_streams', 'v:0',
    '-show_entries', 'stream=nb_read_frames,codec_name', '-of', 'csv=p=0', file], { encoding: 'utf8' });
  if (run.error || run.status !== 0) return { error: run.error?.message || run.stderr.trim() || `ffprobe exit ${run.status}` };
  const fields = run.stdout.trim().split(/\r?\n/)[0]?.split(',') ?? [];
  return { codec: fields.find(f => !/^\d+$/.test(f)), pictures: Number(fields.find(f => /^\d+$/.test(f))) };
}

function videoDecodes(scene) {
  const parts = new Map();
  for (const file of scene.partFiles()) {
    const result = decodePart(path.join(scene.out, 'video', file));
    if (result.error) return { problem: `${file}: ${result.error}` };
    if (result.codec !== 'av1') return { problem: `${file}: codec ${result.codec}` };
    parts.set(file, result.pictures);
  }
  const total = [...parts.values()].reduce((sum, n) => sum + n, 0);
  const enc = scene.encLines();
  if (total !== enc.length) return { problem: `ffprobe decodes ${total} pictures in ${parts.size} parts, index has ${enc.length} enc lines` };
  if (enc[0]?.enc[0] !== 0 || enc[0]?.enc[1] !== 0) return { problem: `first enc is ${JSON.stringify(enc[0]?.enc)}, not [0,0]` };
  const bad = [], seen = new Set();
  let previousPart = null;
  for (const line of enc) {
    const [part, index] = line.enc;
    const file = path.basename(scene.partPath(part));
    if (!parts.has(file) || index >= parts.get(file)) bad.push(`instant ${line.instant}: [${part},${index}] does not exist`);
    if (part !== previousPart && index !== 0) bad.push(`instant ${line.instant}: part ${part} starts at index ${index}`);
    if (seen.has(`${part},${index}`)) bad.push(`instant ${line.instant}: [${part},${index}] used twice`);
    seen.add(`${part},${index}`);
    previousPart = part;
  }
  if (bad.length) return { problem: firstOf(bad) };
  return { note: `${total} av1 pictures in ${parts.size} parts = ${enc.length} enc lines` };
}

function decodeStill(file) {
  const run = spawnSync('ffmpeg', ['-v', 'error', '-i', file, '-f', 'rawvideo', '-pix_fmt', 'bgr24', '-'], { maxBuffer: BIG_OUTPUT });
  if (run.error || run.status !== 0) throw new Error(run.error?.message || String(run.stderr).trim() || `ffmpeg exit ${run.status}`);
  return run.stdout;
}

function stillExact(scene) {
  const files = scene.stillInstants();
  const flagged = scene.lines.filter(line => line.still === true).map(line => line.instant);
  const missing = flagged.filter(k => !files.includes(k)), extra = files.filter(k => !flagged.includes(k));
  if (missing.length || extra.length) return { problem: `still:true without file at ${firstOf(missing) || '-'}; file without still:true at ${firstOf(extra) || '-'}` };
  const bad = [];
  for (const k of files) {
    if (k >= scene.instants) { bad.push(`still ${k}: beyond the scene`); continue; }
    const { width, height } = scene.corpus[k];
    let decoded;
    try { decoded = decodeStill(scene.stillPath(k)); } catch (error) { bad.push(`still ${k}: ${error.message}`); continue; }
    const expected = scene.picture(k);
    if (decoded.length !== width * height * 3 || expected.length !== width * height * 3) bad.push(`still ${k}: ${decoded.length} bytes, picture ${width}x${height} has ${expected.length}`);
    else if (!decoded.equals(expected)) bad.push(`still ${k}: pixels differ`);
  }
  if (bad.length) return { problem: firstOf(bad) };
  stillsCompared += files.length;
  return { note: `${files.length} stills exact` };
}

/** "a still is exact", plus: at least one still compared over all scenes. */
function stillsExact() {
  stillsCompared = 0;
  const result = judge(stillExact);
  if (result.pass && stillsCompared === 0) return { ...result, pass: false, measured: `0 stills compared over all scenes; ${result.measured}` };
  return result;
}

function stillOnSettle(scene) {
  const bad = [];
  if (scene.name === SETTLE_SCENE) {
    const settles = scene.corpus.filter((row, k) => scene.changed(k) && scene.unchangedAfter(k) >= SETTLE_INSTANTS - 1).length;
    if (settles === 0) bad.push(`corpus has 0 settle points (needs at least 1)`);
  }
  const wrong = [];
  scene.corpus.forEach((row, k) => {
    const expected = scene.changed(k) && scene.unchangedAfter(k) >= SETTLE_INSTANTS - 1;
    if ((scene.lines[k]?.still === true) !== expected) wrong.push(`${k}${expected ? ' missing' : ' extra'}`);
  });
  if (wrong.length) bad.push(`still:true wrong at instants ${firstOf(wrong)}`);
  if (bad.length) return { problem: bad.join('; ') };
  const stills = scene.lines.filter(line => line.still === true).map(line => line.instant);
  return { note: `stills at ${stills.length ? firstOf(stills, 5) : 'none (corpus has none)'}` };
}

function unchangedCostsNothing(scene) {
  const wrong = [];
  scene.lines.forEach((line, k) => {
    const kind = kindOf(line);
    if (k === 0) { if (kind !== 'enc') wrong.push(`0 is ${kind}`); return; }
    const unchanged = !scene.changed(k);
    if ((kind === 'same') !== unchanged) wrong.push(`${k} is ${kind}, corpus ${unchanged ? 'unchanged' : 'changed'}`);
  });
  if (wrong.length) return { problem: firstOf(wrong) };
  return { note: `${scene.lines.filter(line => kindOf(line) === 'same').length} same lines, all unchanged in the corpus` };
}

function readBack(scene) {
  const enc = scene.encLines();
  if (!enc.length) return { problem: 'no enc lines' };
  // first, middle, last, and the worst place: the largest index-in-part (last picture before a key picture)
  const deepest = enc.reduce((best, line, i) => (line.enc[1] > enc[best].enc[1] ? i : best), 0);
  const picks = [...new Set([0, Math.floor(enc.length / 2), enc.length - 1, deepest])].map(i => enc[i]);
  const bad = [];
  let slowest = { ms: -1 };
  for (const line of picks) {
    const [part, index] = line.enc;
    const started = process.hrtime.bigint();
    const run = spawnSync('ffmpeg', ['-v', 'error', '-i', scene.partPath(part), '-vf', `select=eq(n\\,${index})`,
      '-frames:v', '1', '-f', 'null', '-'], { encoding: 'utf8' });
    const ms = Number(process.hrtime.bigint() - started) / 1e6;
    if (run.error || run.status !== 0) { bad.push(`instant ${line.instant}: ${run.error?.message || run.stderr.trim() || `ffmpeg exit ${run.status}`}`); continue; }
    if (ms >= READ_BACK_MS) bad.push(`instant ${line.instant}: ${ms.toFixed(0)} ms`);
    if (ms > slowest.ms) slowest = { ms, instant: line.instant };
  }
  if (bad.length) return { problem: firstOf(bad) };
  return { note: `slowest ${slowest.ms.toFixed(0)} ms (instant ${slowest.instant})` };
}

/** Reported, not judged: one row per scene from probe.json. */
function printMeasures() {
  console.log('scene | video MB/min | stills MB/min | cpu s per recorded s | peak RAM MB | first_encoded_ms | lost');
  for (const scene of scenes) {
    const p = scene.probe;
    if (!p) { console.log(`${scene.name} | - | - | - | - | - | -`); continue; }
    const seconds = p.instants / (1000 / INSTANT_MS);
    const perMinute = bytes => (bytes / MB / (seconds / 60)).toFixed(2);
    console.log(`${scene.name} | ${perMinute(p.bytes_video)} | ${perMinute(p.bytes_stills)} | ${(p.cpu_ns / 1e9 / seconds).toFixed(3)} | ${(p.ram_peak / MB).toFixed(1)} | ${p.first_encoded_ms} | ${p.lost}`);
  }
}

const scenes = SCENES.map(name => new Scene(name));
let stillsCompared = 0;
fs.mkdirSync(OUT, { recursive: true });
for (const scene of scenes) scene.record();
const table = {
  'every instant is accounted for': () => judge(everyInstant),
  'nothing is lost silently': () => judge(nothingLostSilently),
  'the video decodes': () => judge(videoDecodes),
  'a still is exact': stillsExact,
  'a still appears when the window settles': () => judge(stillOnSettle),
  'unchanged instants cost nothing': () => judge(unchangedCostsNothing),
  'read-back': () => judge(readBack),
};
fs.writeFileSync(path.join(OUT, 'checks.json'), `${JSON.stringify(scenes.map(s => ({ name: s.name, error: s.error, probe: s.probe })), null, 2)}\n`);
process.exitCode = runFeature('av1-probe', path.join(BASE, 'spec/av1-probe.feature'), table);
printMeasures();
