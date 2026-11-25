#!/usr/bin/env node
// Acceptance checks of docs/clip-recorder-experiments-plan.md, "Acceptance (BDD)".
//
//   node scripts/clip-equation/spec/run-spec.mjs step1
//
// Reads spec/<step>.feature. For each scenario it computes the scenario's
// equation from the inputs in corpus/corpus.json, reads the outcome from the
// corpus folders, and prints "action | equation | known | measured | result".
// The process exits with 1 when any scenario fails. Node built-ins only: the
// zstd frames are decompressed with node:zlib.

import fs from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';
import { fileURLToPath } from 'node:url';

const SPEC_DIR = path.dirname(fileURLToPath(import.meta.url));
const BASE_DIR = path.resolve(SPEC_DIR, '..');
const PASS = 'pass';
const FAIL = 'FAIL';
/** A row the plan checks in a later step: printed, never a failure here. */
const STEP2 = 'step 2';

/** One scenario of a feature file: its title, equation and known result. */
class Scenario {
  constructor(title) {
    this.title = title;
    this.equation = '';
    this.known = '';
  }

  static readAll(step) {
    const text = fs.readFileSync(path.join(SPEC_DIR, `${step}.feature`), 'utf8');
    const scenarios = [];
    for (const raw of text.split(/\r?\n/)) {
      const line = raw.trim();
      if (line.startsWith('#')) continue;
      let match;
      if ((match = /^Scenario:\s*(.+)$/.exec(line))) scenarios.push(new Scenario(match[1]));
      else if (scenarios.length && (match = /\bequation:\s*(.+)$/.exec(line))) scenarios.at(-1).equation = match[1];
      else if (scenarios.length && (match = /\bknown:\s*(.+)$/.exec(line))) scenarios.at(-1).known = match[1];
    }
    return scenarios;
  }

  /** "a=b; c=d" as a Map. */
  knownMap() {
    return new Map(this.known.split(';').map((part) => part.trim().split('=').map((s) => s.trim())));
  }
}

/** One corpus folder: frames.jsonl, meta.json and frame-NNNN.zst files. */
class CorpusFolder {
  constructor(root, name, scenario, size) {
    this.name = name;
    this.scenario = scenario;
    this.size = size;
    this.dir = path.join(root, name);
    this.exists = fs.existsSync(path.join(this.dir, 'frames.jsonl'));
    this.lastIndex = -1;
    this.lastFrame = null;
  }

  get lines() {
    if (!this.cachedLines) {
      this.cachedLines = this.exists
        ? fs.readFileSync(path.join(this.dir, 'frames.jsonl'), 'utf8').split(/\r?\n/).filter(Boolean).map((l) => JSON.parse(l))
        : [];
    }
    return this.cachedLines;
  }

  get meta() {
    const file = path.join(this.dir, 'meta.json');
    return fs.existsSync(file) ? JSON.parse(fs.readFileSync(file, 'utf8')) : null;
  }

  get files() {
    return this.exists ? fs.readdirSync(this.dir).filter((n) => /^frame-\d{4}\.zst$/.test(n)).sort() : [];
  }

  get newCount() {
    return this.lines.filter((l) => l.new === true).length;
  }

  get repeatCount() {
    return this.lines.filter((l) => l.new === false && l.lost !== true && Number.isInteger(l.frame)).length;
  }

  filePath(index) {
    return path.join(this.dir, `frame-${String(index).padStart(4, '0')}.zst`);
  }

  /** The decompressed B,G,R bytes of stored frame `index` (last one cached). */
  frame(index) {
    if (index !== this.lastIndex) {
      this.lastFrame = zlib.zstdDecompressSync(fs.readFileSync(this.filePath(index)));
      this.lastIndex = index;
    }
    return this.lastFrame;
  }
}

/** The step-1 corpus as the inputs describe it. */
class Corpus {
  constructor(config) {
    this.config = config;
    this.root = process.env.CLIP_CORPUS || config.root;
    this.folders = [];
    for (const scenario of Object.keys(config.scenarios)) {
      for (const size of config.sizes) {
        this.folders.push(new CorpusFolder(this.root, `${scenario}-${size.width}x${size.height}`, scenario, size));
      }
    }
    this.instants = config.t_s * config.f_fps;
  }

  get present() {
    return this.folders.filter((f) => f.exists);
  }

  get complete() {
    return this.present.length === this.folders.length;
  }

  folder(name) {
    return this.folders.find((f) => f.name === name) ?? new CorpusFolder(this.root, name, null, null);
  }

  /** True when the flash box of a B,G,R frame shows the flash colour. */
  flashLit(buffer, size) {
    const flash = this.config.flash;
    const left = Math.floor(size.width * flash.box_left_fraction);
    const right = Math.min(size.width, left + flash.box_width + 1);
    const bottom = Math.min(size.height, flash.box_top + flash.box_height);
    let lit = 0;
    for (let y = flash.box_top; y < bottom; y++) {
      for (let x = left; x < right; x++) {
        const at = (y * size.width + x) * 3;
        lit += buffer[at] === flash.bgr[0] && buffer[at + 1] === flash.bgr[1] && buffer[at + 2] === flash.bgr[2] ? 1 : 0;
      }
    }
    return lit >= flash.min_pixels;
  }
}

/** Short list "name=value" of the folders that miss. */
function misses(list) {
  return list.length ? `; misses: ${list.slice(0, 4).join(', ')}${list.length > 4 ? ` (+${list.length - 4})` : ''}` : '';
}

/** Step-1 checks by scenario title: each returns { value, measured, pass }. */
const STEP1 = {
  'list encoders'(scenario, corpus) {
    const file = path.join(BASE_DIR, 'results', 'encoders.json');
    const entries = fs.existsSync(file) ? JSON.parse(fs.readFileSync(file, 'utf8')) : [];
    const unique = new Set(entries.map((e) => `${e.codec}|${e.name}|${e.vendor_id}`)).size;
    return { value: '-', measured: `${unique} unique of ${entries.length} entries`, pass: STEP2 };
  },

  'capture folders'(scenario, corpus) {
    const value = Object.keys(corpus.config.scenarios).length * corpus.config.sizes.length;
    const measured = corpus.present.length;
    const missing = corpus.folders.filter((f) => !f.exists).map((f) => f.name);
    return { value, measured: `${measured} present${misses(missing)}`, pass: value === Number(scenario.known) && measured === value };
  },

  'capture instants'(scenario, corpus) {
    const value = corpus.instants;
    const bad = corpus.folders.filter((f) => f.lines.length !== value).map((f) => `${f.name}=${f.lines.length}`);
    const ok = corpus.folders.length - bad.length;
    return { value, measured: `${ok}/${corpus.folders.length} folders have ${value}${misses(bad)}`, pass: value === Number(scenario.known) && bad.length === 0 };
  },

  'lost instants'(scenario, corpus) {
    const lost = corpus.folders.map((f) => ({ name: f.name, lost: f.exists ? corpus.instants - f.newCount - f.repeatCount : null }));
    const bad = lost.filter((l) => l.lost !== 0).map((l) => `${l.name}=${l.lost ?? 'missing'}`);
    const total = lost.reduce((sum, l) => sum + (l.lost ?? 0), 0);
    return { value: 0, measured: `sum N_lost = ${total} over ${corpus.present.length} folders${misses(bad)}`, pass: Number(scenario.known) === 0 && bad.length === 0 };
  },

  'frame size'(scenario, corpus) {
    const known = scenario.knownMap();
    const equationOk = corpus.config.sizes.every((s) => Number(known.get(`${s.width}x${s.height}`)) === s.width * s.height * 3);
    let checked = 0;
    const bad = [];
    for (const folder of corpus.present) {
      const expected = folder.size.width * folder.size.height * 3;
      for (let index = 0; index < folder.files.length; index++) {
        checked += 1;
        const length = folder.frame(index).length;
        if (length !== expected) bad.push(`${folder.name}/${index}=${length}`);
      }
    }
    const value = corpus.config.sizes.map((s) => s.width * s.height * 3).join('; ');
    return { value, measured: `${checked} frames, ${bad.length} wrong${misses(bad)}`, pass: equationOk && corpus.complete && checked > 0 && bad.length === 0 };
  },

  repeats(scenario, corpus) {
    const bad = [];
    for (const folder of corpus.folders) {
      if (!folder.exists) { bad.push(`${folder.name}=missing`); continue; }
      let stored = 0;
      let wrong = folder.files.length !== folder.newCount;
      for (const line of folder.lines) {
        if (line.new === true) {
          wrong ||= line.frame !== stored;
          stored += 1;
        } else if (line.lost !== true) {
          wrong ||= !(Number.isInteger(line.frame) && line.frame < stored);
        }
      }
      if (wrong) bad.push(`${folder.name}: files ${folder.files.length}, N_new ${folder.newCount}`);
    }
    return { value: 'files = N_new', measured: `${corpus.folders.length - bad.length}/${corpus.folders.length} folders${misses(bad)}`, pass: bad.length === 0 };
  },

  'still page'(scenario, corpus) {
    const still = corpus.folders.filter((f) => f.scenario === 'still');
    const counts = still.map((f) => (f.exists ? f.newCount : 'missing'));
    const value = 1;
    return { value, measured: `N_new = ${counts.join(', ')}`, pass: value === Number(scenario.known) && counts.every((c) => c === value) };
  },

  'flash page'(scenario, corpus) {
    const value = corpus.config.t_s * corpus.config.flash.per_second;
    const counts = corpus.folders.filter((f) => f.scenario === 'flash').map((folder) => {
      if (!folder.exists) return 'missing';
      const litByFrame = new Map();
      let flashes = 0;
      let previous = false;
      for (const line of folder.lines) {
        let lit = false;
        if (Number.isInteger(line.frame)) {
          if (!litByFrame.has(line.frame)) litByFrame.set(line.frame, corpus.flashLit(folder.frame(line.frame), folder.size));
          lit = litByFrame.get(line.frame);
        }
        if (lit && !previous) flashes += 1;
        previous = lit;
      }
      return flashes;
    });
    return { value, measured: `flashes = ${counts.join(', ')}`, pass: value === Number(scenario.known) && counts.every((c) => c === value) };
  },

  repeatability(scenario, corpus) {
    const a = corpus.folder(corpus.config.repeat.of);
    const b = corpus.folder(corpus.config.repeat.folder);
    if (!a.exists || !b.exists) return { value: 0, measured: `missing ${[a, b].filter((f) => !f.exists).map((f) => f.name).join(', ')}`, pass: false };
    let differing = 0;
    const count = Math.max(a.lines.length, b.lines.length);
    for (let j = 0; j < count; j++) {
      const la = a.lines[j];
      const lb = b.lines[j];
      const same = la && lb && Number.isInteger(la.frame) && Number.isInteger(lb.frame) && a.frame(la.frame).equals(b.frame(lb.frame));
      differing += same ? 0 : 1;
    }
    return { value: 0, measured: `${differing} of ${count} instants differ`, pass: Number(scenario.known) === 0 && differing === 0 };
  },

  'waiting bytes'(scenario, corpus) {
    const bad = [];
    let total = 0;
    for (const folder of corpus.folders) {
      if (!folder.exists) { bad.push(`${folder.name}=missing`); continue; }
      const listed = folder.lines.filter((l) => l.new === true).reduce((sum, l) => sum + l.zstd_bytes, 0);
      const onDisk = folder.files.reduce((sum, n) => sum + fs.statSync(path.join(folder.dir, n)).size, 0);
      total += onDisk;
      if (listed !== onDisk) bad.push(`${folder.name}: ${listed} != ${onDisk}`);
    }
    return { value: 'sum zstd_bytes', measured: `${corpus.folders.length - bad.length}/${corpus.folders.length} folders equal; D_wait total ${total} B${misses(bad)}`, pass: bad.length === 0 };
  },
};

const STEPS = { step1: STEP1 };

function main() {
  const step = process.argv[2];
  const checks = STEPS[step];
  if (!checks) {
    console.error(`usage: node run-spec.mjs <${Object.keys(STEPS).join('|')}>`);
    process.exit(2);
  }
  const config = JSON.parse(fs.readFileSync(path.join(BASE_DIR, 'corpus', 'corpus.json'), 'utf8'));
  const corpus = new Corpus(config);
  console.log(`${step}: corpus ${corpus.root}`);
  console.log('action | equation | known | equation value | measured | result');
  let failed = 0;
  for (const scenario of Scenario.readAll(step)) {
    const check = checks[scenario.title];
    let outcome;
    try {
      outcome = check ? check(scenario, corpus) : { value: '-', measured: 'no check for this scenario', pass: false };
    } catch (error) {
      outcome = { value: '-', measured: `error: ${error.message}`, pass: false };
    }
    const result = outcome.pass === STEP2 ? STEP2 : outcome.pass === true ? PASS : FAIL;
    if (result === FAIL) failed += 1;
    console.log(`${scenario.title} | ${scenario.equation} | ${scenario.known} | ${outcome.value} | ${outcome.measured} | ${result}`);
  }
  console.log(failed ? `${step}: ${failed} failed` : `${step}: all passed`);
  process.exit(failed ? 1 : 0);
}

if (process.argv[2] === 'step2z') process.exitCode = (await import('./step2z.mjs')).run([process.argv[3] ?? path.join(BASE_DIR, 'results', 'z-runs.csv'), ...process.argv.slice(4)]);
else if (process.argv[2] === 'step2d') process.exitCode = (await import('./step2d.mjs')).main(process.argv[3], process.argv[4]);
else if (process.argv[2] === 'step4') process.exitCode = await (await import('./step4.mjs')).main(process.argv[3]);
else if (process.argv[2] === 'step5') process.exitCode = await (await import('./step5.mjs')).main(process.argv[3]);
else if (process.argv[2] === 'd5') process.exitCode = (await import('./d5.mjs')).main(process.argv[3], process.argv[4]);
else if (process.argv[2] === 'dyn-exact') await import('./dyn-exact.mjs');
else if (process.argv[2] === 'dyn-moving') await import('./dyn-moving.mjs');
else if (process.argv[2] === 'av1-probe') await import('./av1-probe.mjs');
else if (process.argv[2] === 'dyn-motion') await import('./dyn-motion.mjs');
else if (['dyn-folder', 'dyn-recovery', 'dyn-viewer', 'dyn-youtube', 'dyn-seek', 'dyn-empty-recovery'].includes(process.argv[2])) {
  const [check] = process.argv.splice(2, 1);
  await import(`./${check}.mjs`);
}
else main();
