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
// `scenes=a,b,c` runs other corpus scenes (for example the small and odd-sized ones) through the same checks.
const SCENES = options.scenes ? options.scenes.split(',') : ['typing-1920x1080', 'scroll-1920x1080', 'ui-1920x1080', 'mix-1920x1080', 'resize-640x360-to-506x333', 'lost-then-same-640x360'];
// A clip the product recorded on a real page (1904x1012, 40 s, parts of 200 pictures); the fetch checks read it.
const FETCH_CLIP = path.join(ROOT, 'recorded-real-page');
const FETCH_GOLDEN = path.join(FETCH_CLIP, 'fetch-reference.json');
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

  /** True when instant k shows a different picture than instant k-1; instant 0 and the instant after a lost one count as a change. */
  changed(k) { return k === 0 || this.lostInCorpus(k - 1) || this.pictureHash(k) !== this.pictureHash(k - 1); }

  /** True when the corpus says this instant's capture failed: the recorder is given no picture for it. */
  lostInCorpus(k) { return this.corpus[k].lost === true; }

  /** How many instants directly after k show the same picture as k. */
  unchangedAfter(k) {
    let n = 0;
    while (k + 1 + n < this.instants && !this.lostInCorpus(k + 1 + n) && this.pictureHash(k + 1 + n) === this.pictureHash(k)) n += 1;
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
  // A settle point is marked once: `still:true` (a file of its own) when its picture was never stored
  // before, else `still_of: j` naming the earlier still with the same pixels. Nothing else is marked.
  const wrong = [], stored = new Map(); // pixel hash -> instant of the still file that holds it
  scene.corpus.forEach((row, k) => {
    const line = scene.lines[k] ?? {};
    const expected = !scene.lostInCorpus(k) && scene.changed(k) && scene.unchangedAfter(k) >= SETTLE_INSTANTS - 1;
    const marked = line.still === true || line.still_of != null;
    if (line.still === true && line.still_of != null) wrong.push(`${k} has both still and still_of`);
    else if (marked !== expected) wrong.push(`${k}${expected ? ' missing' : ' extra'}`);
    else if (expected) {
      const hash = scene.pictureHash(k), earlier = stored.get(hash);
      if (earlier === undefined) { if (line.still !== true) wrong.push(`${k} names still_of ${line.still_of}, but no earlier still holds its picture`); else stored.set(hash, k); }
      else if (line.still_of !== earlier) wrong.push(`${k} should be still_of ${earlier} (same pixels), is ${line.still === true ? 'a second still file' : `still_of ${line.still_of}`}`);
    }
  });
  if (wrong.length) bad.push(`settle marks wrong at instants ${firstOf(wrong)}`);
  if (bad.length) return { problem: bad.join('; ') };
  const stills = scene.lines.filter(line => line.still === true).map(line => line.instant);
  const reused = scene.lines.filter(line => line.still_of != null).length;
  return { note: `stills at ${stills.length ? firstOf(stills, 5) : 'none (corpus has none)'}${reused ? `, ${reused} settle points reuse a stored still` : ''}` };
}

function unchangedCostsNothing(scene) {
  const wrong = [];
  scene.lines.forEach((line, k) => {
    const kind = kindOf(line);
    if (scene.lostInCorpus(k)) { if (kind !== 'lost') wrong.push(`${k} is ${kind}, the corpus has no picture for it`); return; }
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

/**
 * A window that changes size keeps being recorded: no instant is lost, the first instant at a new
 * size starts a new part, and the pictures come back at the size they were recorded at.
 */
function resizeKeepsRecording(scene) {
  const size = k => `${scene.corpus[k].width}x${scene.corpus[k].height}`;
  const resizes = scene.corpus.map((row, k) => k).filter(k => k > 0 && size(k) !== size(k - 1));
  if (!resizes.length) return { note: 'no size change in this scene' };
  const lost = scene.lines.filter(line => kindOf(line) === 'lost');
  if (lost.length) return { problem: `${lost.length} instants lost (${firstOf([...new Set(lost.map(line => line.lost))])}), first at ${lost[0].instant}` };
  const bad = [];
  for (const k of resizes) {
    const line = scene.lines[k] ?? {};
    if (kindOf(line) !== 'enc' || line.enc[1] !== 0) bad.push(`instant ${k} (first at ${size(k)}) is ${JSON.stringify(line.enc ?? kindOf(line))}, not the first picture of a new part`);
  }
  // the last encoded instant of every size segment comes back at that segment's size
  for (const end of [...resizes.map(k => k - 1), scene.instants - 1]) {
    const line = scene.lines.slice(0, end + 1).reverse().find(l => kindOf(l) === 'enc');
    const got = fetchPicture(scene.out, line.instant);
    if (got.error) bad.push(`fetch of instant ${line.instant}: ${got.error}`);
    else if (`${got.width}x${got.height}` !== size(line.instant)) bad.push(`instant ${line.instant} comes back ${got.width}x${got.height}, recorded ${size(line.instant)}`);
  }
  if (bad.length) return { problem: firstOf(bad) };
  return { note: `${resizes.length} size changes at instants ${resizes.join(', ')}: 0 lost, each starts a new part, pictures come back at their own size` };
}

/** After a lost instant the next picture is recorded again, never written as 'same' (a 'same' line would resolve back to the loss). */
function recordedAgainAfterLoss(scene) {
  const after = scene.lines.filter((line, k) => k > 0 && kindOf(scene.lines[k - 1]) === 'lost' && kindOf(line) !== 'lost');
  if (!after.length) return { note: 'no lost instant in this scene' };
  const bad = [];
  for (const line of after) {
    if (kindOf(line) !== 'enc') { bad.push(`instant ${line.instant} follows a lost instant and is ${kindOf(line)}`); continue; }
    const got = fetchPicture(scene.out, line.instant);
    if (got.error) bad.push(`fetch of instant ${line.instant}: ${got.error}`);
    else if (got.kind === 'lost') bad.push(`instant ${line.instant} comes back as lost`);
  }
  // an unchanged instant later in the same run must come back as a picture too
  const later = scene.lines.find((line, k) => kindOf(line) === 'same' && scene.lines.slice(0, k).some(l => kindOf(l) === 'lost'));
  if (later) {
    const got = fetchPicture(scene.out, later.instant);
    if (got.error || got.kind === 'lost') bad.push(`instant ${later.instant} (same, after a loss) comes back as ${got.error ?? 'lost'}`);
  }
  if (bad.length) return { problem: firstOf(bad) };
  return { note: `${after.length} instants after a loss are recorded again and come back as pictures` };
}

/** A copy of a killed recording, changed by `damage(dir)`, then recovered by the product: { dir, result, before, lines } or { error }. */
function recovered(fixture, name, damage) {
  const source = path.join(ROOT, fixture);
  if (!fs.existsSync(path.join(source, 'clip.json'))) return { error: `no killed recording at ${source}` };
  const dir = path.join(OUT, name);
  if (fs.existsSync(dir)) return { error: `output folder exists: ${dir}` };
  fs.cpSync(source, dir, { recursive: true });
  const before = readJsonl(path.join(dir, 'frames.jsonl'));
  damage(dir);
  const run = spawnSync(PROBE, ['exp-av1-recover', `folder=${dir}`], { encoding: 'utf8', maxBuffer: BIG_OUTPUT });
  if (run.error || run.status !== 0) return { error: run.error?.message || run.stderr.trim() || `exit ${run.status}` };
  let result;
  try { result = JSON.parse(run.stdout.trim().split(/\r?\n/).pop()); } catch (error) { return { error: `no JSON line: ${error.message}` }; }
  if (!result.ok) return { error: `recovery failed: ${result.error}` };
  return { dir, result, before, lines: readJsonl(path.join(dir, 'frames.jsonl')) };
}
const lastPart = dir => path.join(dir, 'video', fs.readdirSync(path.join(dir, 'video')).filter(f => /^part-\d+\.ivf$/.test(f)).sort().at(-1));
const verdict = (problems, note) => ({ pass: problems.length === 0, value: '-', measured: problems.length ? problems.join('; ') : note });

/** The recorder died while writing the first bytes of a new part: the earlier parts must stay readable. */
function tornLastPart() {
  const TORN_BYTES = 10; // less than the 32-byte IVF header
  let torn;
  const got = recovered('killed-real-page', 'crash-torn-part', dir => {
    torn = Number(path.basename(lastPart(dir)).match(/\d+/)[0]);
    fs.truncateSync(lastPart(dir), TORN_BYTES);
  });
  if (got.error) return verdict([got.error]);
  const problems = [];
  const earlier = got.before.filter(line => kindOf(line) === 'enc' && line.enc[0] < torn);
  const stillEnc = earlier.filter(line => kindOf(got.lines[line.instant] ?? {}) === 'enc').length;
  if (stillEnc !== earlier.length) problems.push(`${earlier.length - stillEnc} of ${earlier.length} pictures of the earlier parts are no longer in the index`);
  const fromTorn = got.before.filter(line => kindOf(line) === 'enc' && line.enc[0] === torn);
  const unnamed = fromTorn.filter(line => { const now = got.lines[line.instant] ?? {}; return kindOf(now) !== 'lost' || !String(now.lost).trim(); }).length;
  if (unnamed) problems.push(`${unnamed} of ${fromTorn.length} pictures of the torn part are not named lost`);
  const sample = earlier.at(-1);
  const fetched = fetchPicture(got.dir, sample.instant);
  if (fetched.error || fetched.kind !== 'video') problems.push(`instant ${sample.instant} of an earlier part comes back as ${fetched.error ?? fetched.kind}`);
  return verdict(problems, `part ${torn} torn to ${TORN_BYTES} bytes: recovery succeeded, ${earlier.length} earlier pictures kept, ${fromTorn.length} named lost, instant ${sample.instant} comes back as video`);
}

/** The still reached disk but the video picture of its instant did not: the still must survive and be the answer. */
function stillSurvivesRecovery() {
  const IVF_HEADER_BYTES = 32; // the part keeps its header and loses every picture
  const got = recovered('killed-text-page', 'crash-still-without-video', dir => fs.truncateSync(lastPart(dir), IVF_HEADER_BYTES));
  if (got.error) return verdict([got.error]);
  const still = got.before.find(line => line.still === true);
  if (!still) return verdict(['the killed recording has no still']);
  const problems = [];
  if (!got.result.stills_on_disk.includes(still.instant)) problems.push(`still of instant ${still.instant} was deleted`);
  const next = got.before[still.instant + 1];
  for (const instant of [still.instant, ...(next && kindOf(next) === 'same' ? [next.instant] : [])]) {
    const fetched = fetchPicture(got.dir, instant);
    if (fetched.error || fetched.kind !== 'exact still') problems.push(`instant ${instant} comes back as ${fetched.error ?? fetched.kind}, not the exact still`);
  }
  return verdict(problems, `video pictures gone, still of instant ${still.instant} kept and returned as exact still`);
}

/**
 * The still file of an instant is gone and its line is lost: no lost line may name that still.
 * `alreadyLost` false: the line becomes lost in this recovery (its part is cut to the header).
 * `alreadyLost` true: the index already holds the line as lost with its still mark (an earlier recovery wrote it).
 */
function lostLineNamesNoMissingStill(name, alreadyLost) {
  const IVF_HEADER_BYTES = 32; // the part keeps its header and loses every picture
  let gone;
  const got = recovered('killed-text-page', name, dir => {
    const index = path.join(dir, 'frames.jsonl');
    const lines = readJsonl(index);
    const still = lines.find(line => line.still === true);
    if (!still) return;
    gone = still.instant;
    if (alreadyLost) {
      lines[gone] = { instant: still.instant, t_ms: still.t_ms, lost: 'service_restart', still: true };
      fs.writeFileSync(index, lines.map(line => `${JSON.stringify(line)}\n`).join(''));
    } else fs.truncateSync(lastPart(dir), IVF_HEADER_BYTES);
    fs.rmSync(path.join(dir, 'stills', `still-${pad(gone, 8)}.webp`));
  });
  if (got.error) return verdict([got.error]);
  if (gone === undefined) return verdict(['the killed recording has no still']);
  const problems = [];
  if (kindOf(got.lines[gone] ?? {}) !== 'lost') problems.push(`instant ${gone} is not named lost`);
  const naming = got.lines.filter(line => kindOf(line) === 'lost' && ((line.still === true && line.instant === gone) || line.still_of === gone));
  if (naming.length) problems.push(`${naming.length} lost lines name the missing still of instant ${gone} (first: instant ${naming[0].instant})`);
  const next = got.before[gone + 1];
  if (next && kindOf(next) === 'same' && kindOf(got.lines[next.instant] ?? {}) !== 'lost') problems.push(`instant ${next.instant} repeats the lost instant ${gone} and is not named lost`);
  return verdict(problems, `still of instant ${gone} gone and its line lost: no line names the still`);
}

/** A planned length off the 50 ms grid: every planned instant has an index line after recovery. */
function recoveryNamesEveryInstant() {
  const OFF_GRID_MS = 25;
  let planned;
  const got = recovered('killed-text-page', 'crash-off-grid', dir => {
    const file = path.join(dir, 'clip.json'), clip = JSON.parse(fs.readFileSync(file, 'utf8'));
    clip.end_ms = readJsonl(path.join(dir, 'frames.jsonl')).length * INSTANT_MS + OFF_GRID_MS;
    planned = Math.ceil(clip.end_ms / INSTANT_MS);
    fs.writeFileSync(file, JSON.stringify(clip));
  });
  if (got.error) return verdict([got.error]);
  const problems = [];
  if (got.lines.length !== planned) problems.push(`${got.lines.length} index lines after recovery, ${planned} instants were planned (end_ms ${OFF_GRID_MS} ms off the grid)`);
  else if (kindOf(got.lines.at(-1)) !== 'lost') problems.push(`the last planned instant is ${kindOf(got.lines.at(-1))}, not named lost`);
  return verdict(problems, `${planned} planned instants, ${got.lines.length} index lines, the last one named lost`);
}

/**
 * A failed clip whose index was never made to agree with the disk (the reconcile at failure itself
 * failed): recovery on start names every line whose picture is not on disk lost (encoder_failed),
 * adds no instant, lists the lost lines in clip.json and leaves the clip failed. A second recovery
 * of the now agreeing clip changes neither frames.jsonl nor clip.json.
 */
function failedClipReconciled() {
  const IVF_HEADER_BYTES = 32; // the part keeps its header and loses every picture
  let torn;
  const got = recovered('killed-text-page', 'failed-unreconciled', dir => {
    torn = Number(path.basename(lastPart(dir)).match(/\d+/)[0]);
    fs.truncateSync(lastPart(dir), IVF_HEADER_BYTES);
    const file = path.join(dir, 'clip.json'), clip = JSON.parse(fs.readFileSync(file, 'utf8'));
    clip.state = 'failed';
    clip.failure = 'bench: the encoder failed and the reconcile at failure failed too';
    fs.writeFileSync(file, JSON.stringify(clip));
  });
  if (got.error) return verdict([got.error]);
  const missing = got.before.filter(line => kindOf(line) === 'enc' && line.enc[0] === torn);
  if (!missing.length) return verdict([`part ${torn} has no enc line`]);
  const problems = [];
  const unnamed = missing.filter(line => (got.lines[line.instant] ?? {}).lost !== 'encoder_failed');
  if (unnamed.length) problems.push(`${unnamed.length} of ${missing.length} lines whose picture is not on disk are not named lost encoder_failed (first: instant ${unnamed[0].instant} is ${kindOf(got.lines[unnamed[0].instant] ?? {})})`);
  if (got.lines.length !== got.before.length) problems.push(`${got.lines.length} index lines after recovery, ${got.before.length} before`);
  const clipFile = path.join(got.dir, 'clip.json'), indexFile = path.join(got.dir, 'frames.jsonl');
  const clip = JSON.parse(fs.readFileSync(clipFile, 'utf8'));
  if (clip.state !== 'failed') problems.push(`the clip is ${clip.state}, not failed`);
  const listed = new Set((clip.lost ?? []).map(lost => lost.instant));
  const unlisted = got.lines.filter(line => kindOf(line) === 'lost' && !listed.has(line.instant));
  if (unlisted.length) problems.push(`${unlisted.length} lost lines are not in the lost list of clip.json (first: instant ${unlisted[0].instant})`);
  const [clipBefore, indexBefore] = [fs.readFileSync(clipFile, 'utf8'), fs.readFileSync(indexFile, 'utf8')];
  const again = spawnSync(PROBE, ['exp-av1-recover', `folder=${got.dir}`], { encoding: 'utf8', maxBuffer: BIG_OUTPUT });
  let second = null;
  try { second = JSON.parse(again.stdout.trim().split(/\r?\n/).pop()); } catch { /* reported below as a failed second recovery */ }
  if (again.error || again.status !== 0 || !second?.ok) problems.push(`second recovery: ${again.error?.message || again.stderr.trim() || second?.error || `exit ${again.status}`}`);
  else {
    if (fs.readFileSync(indexFile, 'utf8') !== indexBefore) problems.push('a second recovery changed frames.jsonl');
    if (fs.readFileSync(clipFile, 'utf8') !== clipBefore) problems.push('a second recovery changed clip.json');
  }
  return verdict(problems, `failed clip, part ${torn} cut to its header: ${missing.length} lines named lost encoder_failed and listed, still failed; a second recovery changed nothing`);
}

/**
 * A failed clip that cannot be reconciled (clip.json says fps 0, and its index has a gap at instant 0,
 * so lost lines would have to be made): recovery refuses it with an error and does not crash.
 */
function failedClipZeroFpsRefused() {
  const got = recovered('killed-text-page', 'failed-zero-fps', dir => {
    const file = path.join(dir, 'clip.json'), clip = JSON.parse(fs.readFileSync(file, 'utf8'));
    clip.state = 'failed';
    clip.failure = 'bench: failed with fps 0';
    clip.fps = 0;
    fs.writeFileSync(file, JSON.stringify(clip));
    const index = path.join(dir, 'frames.jsonl');
    fs.writeFileSync(index, readJsonl(index).slice(1, 2).map(line => `${JSON.stringify(line)}\n`).join(''));
  });
  if (!got.error) return verdict(['recovery accepted a failed clip with fps 0']);
  const refused = /^recovery failed: .*zero fps/.test(got.error);
  return verdict(refused ? [] : [`recovery did not refuse cleanly: ${got.error.replace(/\s+/g, ' ').slice(0, 200)}`], 'failed clip with fps 0 and a gap at instant 0: recovery refuses it (zero fps) and does not crash');
}

/**
 * A killed recording whose clip.json plans more than the longest recording (10 minutes): recovery
 * refuses it with an error and does not make the lost lines up to that end.
 */
function overlongClipRefused() {
  const LONGEST_MS = 600_000; // the recorder's longest clip
  const got = recovered('killed-text-page', 'crash-overlong', dir => {
    const file = path.join(dir, 'clip.json'), clip = JSON.parse(fs.readFileSync(file, 'utf8'));
    clip.end_ms = LONGEST_MS + INSTANT_MS;
    fs.writeFileSync(file, JSON.stringify(clip));
  });
  if (!got.error) return verdict([`recovery accepted a clip planned ${LONGEST_MS + INSTANT_MS} ms long and wrote ${got.lines.length} index lines`]);
  const refused = /^recovery failed: .*longest recording/.test(got.error);
  return verdict(refused ? [] : [`recovery did not refuse cleanly: ${got.error.replace(/\s+/g, ' ').slice(0, 200)}`], `clip planned ${LONGEST_MS + INSTANT_MS} ms long: recovery refuses it (beyond the longest recording)`);
}

/** One picture fetched through the product's decoder (`clip-probe exp-av1-fetch`): its JSON line, or { error }. */
function fetchPicture(folder, instant) {
  const run = spawnSync(PROBE, ['exp-av1-fetch', `folder=${folder}`, `instant=${instant}`], { encoding: 'utf8', maxBuffer: BIG_OUTPUT });
  if (run.error || run.status !== 0) return { error: run.error?.message || run.stderr.trim() || `exit ${run.status}` };
  try { return JSON.parse(run.stdout.trim().split(/\r?\n/).pop()); } catch (error) { return { error: `no JSON line: ${error.message}` }; }
}

/** The recorded clip of the fetch checks: its index, or the reason it cannot be used. */
function fetchClip() {
  const index = path.join(FETCH_CLIP, 'frames.jsonl');
  if (!fs.existsSync(index)) return { error: `no recorded clip at ${FETCH_CLIP}` };
  return { enc: readJsonl(index).filter(line => kindOf(line) === 'enc') };
}

/** The worst place to fetch: the picture with the largest index-in-part (everything before it in the part is decoded). */
function latePictureInTime() {
  const clip = fetchClip();
  if (clip.error) return { pass: false, value: '-', measured: clip.error };
  const line = clip.enc.reduce((best, candidate) => (candidate.enc[1] > best.enc[1] ? candidate : best));
  const got = fetchPicture(FETCH_CLIP, line.instant);
  if (got.error) return { pass: false, value: '-', measured: got.error };
  const measured = `instant ${line.instant} [${line.enc}]: ${got.total_ms} ms in all (read ${got.read_part_ms}, decode ${got.decode_ms}, convert ${got.convert_ms}), ${got.pictures_decoded} pictures decoded, kind ${got.kind}`;
  const pass = got.kind === 'video' && got.pictures_decoded === line.enc[1] + 1 && got.total_ms < READ_BACK_MS;
  return { pass, value: `${got.total_ms} ms`, measured };
}

/**
 * A change of decoder must not change a picture: the fetched pixels equal the reference hashes
 * (first, deepest and last encoded instant). `golden=write` stores the hashes of the current
 * build as the reference and leaves the check pending for that run.
 */
function fetchedPictureIsRecorded() {
  const clip = fetchClip();
  if (clip.error) return { pass: false, value: '-', measured: clip.error };
  const deepest = clip.enc.reduce((best, candidate) => (candidate.enc[1] > best.enc[1] ? candidate : best));
  const instants = [...new Set([clip.enc[0].instant, deepest.instant, clip.enc.at(-1).instant])];
  const hashes = {};
  for (const instant of instants) {
    const got = fetchPicture(FETCH_CLIP, instant);
    if (got.error) return { pass: false, value: '-', measured: `instant ${instant}: ${got.error}` };
    hashes[instant] = got.rgba_sha256;
  }
  if (options.golden === 'write') {
    fs.writeFileSync(FETCH_GOLDEN, `${JSON.stringify(hashes, null, 2)}\n`);
    return { pass: 'PENDING', value: '-', measured: `reference written for instants ${instants.join(', ')}` };
  }
  if (!fs.existsSync(FETCH_GOLDEN)) return { pass: false, value: '-', measured: `no reference at ${FETCH_GOLDEN} (run once with golden=write)` };
  const golden = JSON.parse(fs.readFileSync(FETCH_GOLDEN, 'utf8'));
  const differ = instants.filter(instant => golden[instant] !== hashes[instant]);
  return { pass: differ.length === 0, value: '-', measured: differ.length ? `pixels differ from the reference at instants ${differ.join(', ')}` : `${instants.length} pictures equal the reference (instants ${instants.join(', ')})` };
}

/** Reported, not judged: one row per scene from probe.json. */
function printMeasures() {
  console.log('scene | video MB/min | stills MB/min | cpu s per recorded s | peak RAM MB | first_encoded_ms | lost');
  for (const scene of scenes) {
    const p = scene.probe;
    if (!p) { console.log(`${scene.name} | - | - | - | - | - | -`); continue; }
    const seconds = p.instants / (1000 / INSTANT_MS);
    const perMinute = bytes => (bytes / MB / (seconds / 60)).toFixed(2);
    // sizes are read from the files on disk, not from the probe's own count
    const onDisk = dir => (fs.existsSync(path.join(scene.out, dir)) ? fs.readdirSync(path.join(scene.out, dir)).reduce((sum, f) => sum + fs.statSync(path.join(scene.out, dir, f)).size, 0) : 0);
    console.log(`${scene.name} | ${perMinute(onDisk('video'))} | ${perMinute(onDisk('stills'))} | ${(p.cpu_ns / 1e9 / seconds).toFixed(3)} | ${(p.ram_peak / MB).toFixed(1)} | ${p.first_encoded_ms} | ${p.lost}`);
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
  'a resized window keeps recording': () => judge(resizeKeepsRecording),
  'a picture after a lost instant is recorded again': () => judge(recordedAgainAfterLoss),
  'a torn last part does not stop recovery': tornLastPart,
  'recovery keeps a complete still': stillSurvivesRecovery,
  'a lost line names no missing still': () => lostLineNamesNoMissingStill('crash-still-and-video-gone', false),
  'a line already lost names no missing still': () => lostLineNamesNoMissingStill('crash-lost-line-stale-still', true),
  'recovery names every planned instant': recoveryNamesEveryInstant,
  'recovery makes a failed clip agree with its pictures': failedClipReconciled,
  'recovery refuses a failed clip with zero fps': failedClipZeroFpsRefused,
  'recovery refuses a clip longer than the longest recording': overlongClipRefused,
  'a late picture comes back in time': latePictureInTime,
  'the fetched picture is the recorded one': fetchedPictureIsRecorded,
};
fs.writeFileSync(path.join(OUT, 'checks.json'), `${JSON.stringify(scenes.map(s => ({ name: s.name, error: s.error, probe: s.probe })), null, 2)}\n`);
process.exitCode = runFeature('av1-probe', path.join(BASE, 'spec/av1-probe.feature'), table);
printMeasures();
