#!/usr/bin/env node
// Experiments step 1 (docs/clip-recorder-experiments-plan.md, F6, F7 and
// "Procedure" step 1): renders the fixed frame corpus with headless Chrome.
//
//   node scripts/clip-equation/corpus/render.mjs [--suffix S] [name ...]
//
// Names look like `typing-1280x720`; without names every scenario renders at
// every size of corpus.json. For each one the script starts its own Chrome
// (`--headless=new`, a fresh temporary profile, no window), drives it over the
// DevTools protocol with Node's built-in WebSocket and stops it again by PID.
//
// Time: virtual time starts at virtual_start_unix_ms and advances only when
// this script grants a budget, so timers, requestAnimationFrame, Date.now()
// and performance.now() all follow it. The page gets ?start = s_0 - page_lead;
// instant j is s_j = s_0 + j * instant_ms; at s_j the page shows its step j.
// One lossless PNG screenshot per instant at the window size, device scale 1.
// A frame equal in every pixel to the previous one is a repeat.
//
// Output per capture, in <root>\<scenario>-<w>x<h>[suffix]\:
//   frame-NNNN.zst  zstd level -3 of the frame's B,G,R bytes, rows top to
//                   bottom, w * h * 3 bytes; one file per new frame
//   frames.jsonl    one line per instant: instant, frame, new, lost, width,
//                   height, zstd_bytes, sha256 (first 16 hex of SHA-256 of B,G,R)
//   meta.json       scenario, window, sigma, recorded (= sigma * window), times
//   row.md          this capture's row of results/corpus.md
// Frames stay at the window size; sigma is recorded and the encode step scales.

import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import zlib from 'node:zlib';
import crypto from 'node:crypto';
import { spawn, spawnSync } from 'node:child_process';
import { fileURLToPath, pathToFileURL } from 'node:url';

const CORPUS_DIR = path.dirname(fileURLToPath(import.meta.url));
const BASE_DIR = path.resolve(CORPUS_DIR, '..');
const CONFIG = JSON.parse(fs.readFileSync(path.join(CORPUS_DIR, 'corpus.json'), 'utf8'));
const ROOT = process.env.CLIP_CORPUS || CONFIG.root;
const STEP_TIMEOUT_MS = 60000;
/**
 * Measured 2026-11-22 on Chrome 158: under virtual time, Date.now(),
 * performance.now() and performance.timeOrigin follow it exactly, but the
 * timestamp handed to requestAnimationFrame callbacks comes from the
 * compositor's real frame clock (it advanced about 17 ms per 50 ms instant).
 * The pages compute their step from that timestamp, so this script, added to
 * every page before its own scripts run, hands them performance.now() instead.
 */
const VIRTUAL_RAF = '(() => { const raf = window.requestAnimationFrame.bind(window); window.requestAnimationFrame = (callback) => raf(() => callback(performance.now())); })();';

/** Decodes an 8-bit, non-interlaced RGB or RGBA PNG to B,G,R bytes. */
function pngToBgr(png) {
  let at = 8;
  let width = 0;
  let height = 0;
  let colourType = -1;
  const idat = [];
  while (at < png.length) {
    const length = png.readUInt32BE(at);
    const type = png.toString('latin1', at + 4, at + 8);
    const data = png.subarray(at + 8, at + 8 + length);
    if (type === 'IHDR') {
      width = data.readUInt32BE(0);
      height = data.readUInt32BE(4);
      colourType = data[9];
      if (data[8] !== 8 || data[12] !== 0 || (colourType !== 2 && colourType !== 6)) {
        throw new Error(`unsupported PNG: depth ${data[8]}, colour type ${colourType}, interlace ${data[12]}`);
      }
    } else if (type === 'IDAT') idat.push(data);
    else if (type === 'IEND') break;
    at += 12 + length;
  }
  const bpp = colourType === 6 ? 4 : 3;
  const stride = width * bpp;
  const raw = zlib.inflateSync(Buffer.concat(idat));
  const bgr = Buffer.allocUnsafe(width * height * 3);
  let previous = new Uint8Array(stride);
  let current = new Uint8Array(stride);
  for (let y = 0; y < height; y++) {
    const filter = raw[y * (stride + 1)];
    const row = y * (stride + 1) + 1;
    for (let x = 0; x < stride; x++) {
      const a = x >= bpp ? current[x - bpp] : 0;
      const b = previous[x];
      let value = raw[row + x];
      if (filter === 1) value += a;
      else if (filter === 2) value += b;
      else if (filter === 3) value += (a + b) >> 1;
      else if (filter === 4) {
        const c = x >= bpp ? previous[x - bpp] : 0;
        const p = a + b - c;
        const pa = Math.abs(p - a);
        const pb = Math.abs(p - b);
        const pc = Math.abs(p - c);
        value += pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
      } else if (filter !== 0) throw new Error(`PNG filter ${filter}`);
      current[x] = value & 255;
    }
    let out = y * width * 3;
    for (let x = 0; x < stride; x += bpp) {
      bgr[out++] = current[x + 2];
      bgr[out++] = current[x + 1];
      bgr[out++] = current[x];
    }
    [previous, current] = [current, previous];
  }
  return { width, height, bgr };
}

/** A DevTools protocol connection to one page target. */
class Cdp {
  constructor(socket) {
    this.socket = socket;
    this.nextId = 1;
    this.pending = new Map();
    this.waiters = [];
    socket.onmessage = (event) => this.receive(JSON.parse(event.data));
  }

  static async connect(url) {
    const socket = new WebSocket(url);
    await new Promise((resolve, reject) => {
      socket.onopen = resolve;
      socket.onerror = () => reject(new Error(`WebSocket ${url} failed`));
    });
    return new Cdp(socket);
  }

  receive(message) {
    if (message.id) {
      const call = this.pending.get(message.id);
      this.pending.delete(message.id);
      if (!call) return;
      clearTimeout(call.timer);
      if (message.error) call.reject(new Error(`${call.method}: ${message.error.message}`));
      else call.resolve(message.result);
      return;
    }
    this.waiters = this.waiters.filter((waiter) => {
      if (waiter.method !== message.method) return true;
      clearTimeout(waiter.timer);
      waiter.resolve(message.params);
      return false;
    });
  }

  send(method, params = {}) {
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`${method}: no answer in ${STEP_TIMEOUT_MS} ms`));
      }, STEP_TIMEOUT_MS);
      this.pending.set(id, { method, resolve, reject, timer });
      this.socket.send(JSON.stringify({ id, method, params }));
    });
  }

  /** A promise for the next event `method`; register it before the call that causes it. */
  once(method) {
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error(`${method}: no event in ${STEP_TIMEOUT_MS} ms`)), STEP_TIMEOUT_MS);
      this.waiters.push({ method, resolve, timer });
    });
  }

  /** Lets virtual time run `ms` further, then pauses it again. */
  async advance(ms) {
    const expired = this.once('Emulation.virtualTimeBudgetExpired');
    await this.send('Emulation.setVirtualTimePolicy', { policy: 'pauseIfNetworkFetchesPending', budget: ms });
    await expired;
  }

  async evaluate(expression) {
    const answer = await this.send('Runtime.evaluate', { expression, returnByValue: true });
    return answer.result.value;
  }

  close() {
    this.socket.close();
  }
}

/** This script's own headless Chrome: started, used for one capture, stopped by PID. */
class HeadlessChrome {
  constructor(width, height) {
    this.profile = fs.mkdtempSync(path.join(os.tmpdir(), 'clip-render-'));
    this.process = spawn(CONFIG.chrome, [
      '--headless=new',
      `--user-data-dir=${this.profile}`,
      '--remote-debugging-port=0',
      `--window-size=${width},${height}`,
      '--force-device-scale-factor=1',
      '--hide-scrollbars',
      '--disable-gpu',
      '--mute-audio',
      '--no-first-run',
      '--no-default-browser-check',
      '--disable-extensions',
      '--disable-background-networking',
      '--disable-component-update',
      '--disable-sync',
      '--disable-background-timer-throttling',
      '--disable-renderer-backgrounding',
      '--allow-file-access-from-files',
      'about:blank',
    ], { stdio: 'ignore', windowsHide: true });
    try { os.setPriority(this.process.pid, os.constants.priority.PRIORITY_BELOW_NORMAL); } catch { /* already gone: reported by connect */ }
  }

  async connect() {
    const portFile = path.join(this.profile, 'DevToolsActivePort');
    const deadline = Date.now() + STEP_TIMEOUT_MS;
    while (!fs.existsSync(portFile) || !fs.readFileSync(portFile, 'utf8').includes('\n')) {
      if (Date.now() > deadline || this.process.exitCode !== null) throw new Error('Chrome did not open its DevTools port');
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    const port = fs.readFileSync(portFile, 'utf8').split('\n')[0].trim();
    const version = await (await fetch(`http://127.0.0.1:${port}/json/version`)).json();
    const targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
    const page = targets.find((target) => target.type === 'page');
    if (!page) throw new Error('no page target');
    this.version = version.Browser;
    return Cdp.connect(page.webSocketDebuggerUrl);
  }

  stop() {
    spawnSync('taskkill', ['/PID', String(this.process.pid), '/T', '/F'], { stdio: 'ignore', windowsHide: true });
    fs.rmSync(this.profile, { recursive: true, force: true, maxRetries: 20, retryDelay: 250 });
  }
}

/** One capture: a scenario page at one F6 window size. */
class Capture {
  constructor(scenario, size, suffix) {
    this.scenario = scenario;
    this.page = CONFIG.scenarios[scenario];
    this.size = size;
    this.name = `${scenario}-${size.width}x${size.height}${suffix}`;
    this.folder = path.join(ROOT, this.name);
    this.instants = CONFIG.t_s * CONFIG.f_fps;
  }

  prepareFolder() {
    fs.mkdirSync(this.folder, { recursive: true });
    for (const name of fs.readdirSync(this.folder)) {
      if (/^frame-\d{4}\.zst$/.test(name) || ['frames.jsonl', 'meta.json', 'row.md'].includes(name)) {
        fs.rmSync(path.join(this.folder, name));
      }
    }
  }

  async run() {
    const began = Date.now();
    this.prepareFolder();
    const { width, height } = this.size;
    const s0 = CONFIG.virtual_start_unix_ms + CONFIG.load_ms;
    const pageStart = s0 - CONFIG.page_lead_ms;
    const url = `${new URL(this.page, `file:///${BASE_DIR.replaceAll('\\', '/')}/`).href}?start=${pageStart}`;
    const chrome = new HeadlessChrome(width, height);
    const lines = [];
    const stored = [];
    const checks = { virtual_now: [], page_k_mismatch: 0 };
    try {
      const cdp = await chrome.connect();
      await cdp.send('Page.enable');
      await cdp.send('Page.addScriptToEvaluateOnNewDocument', { source: VIRTUAL_RAF });
      await cdp.send('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: 1, mobile: false });
      await cdp.send('Emulation.setVirtualTimePolicy', { policy: 'pause', initialVirtualTime: CONFIG.virtual_start_unix_ms / 1000 });
      const loaded = cdp.once('Page.loadEventFired');
      await cdp.send('Page.navigate', { url });
      await cdp.advance(CONFIG.load_ms);
      await loaded;
      let previous = null;
      let previousPng = null;
      for (let j = 0; j < this.instants; j++) {
        if (j > 0) await cdp.advance(CONFIG.instant_ms);
        const shot = await cdp.send('Page.captureScreenshot', { format: 'png', optimizeForSpeed: true, captureBeyondViewport: false });
        // The same PNG bytes are the same pixels: a repeat needs no decode.
        const frame = shot.data === previousPng ? { width, height, bgr: previous } : pngToBgr(Buffer.from(shot.data, 'base64'));
        previousPng = shot.data;
        if (frame.width !== width || frame.height !== height) throw new Error(`screenshot ${frame.width}x${frame.height} at instant ${j}`);
        if (j === 0 || j === this.instants - 1) checks.virtual_now.push({ instant: j, expected: s0 + j * CONFIG.instant_ms, date_now: await cdp.evaluate('Date.now()') });
        if (this.scenario === 'flash' && String(await cdp.evaluate("document.getElementById('counter').textContent")) !== String(j)) checks.page_k_mismatch += 1;
        const isNew = previous === null || !frame.bgr.equals(previous);
        if (isNew) {
          const packed = zlib.zstdCompressSync(frame.bgr, { params: { [zlib.constants.ZSTD_c_compressionLevel]: CONFIG.zstd_level } });
          fs.writeFileSync(path.join(this.folder, `frame-${String(stored.length).padStart(4, '0')}.zst`), packed);
          stored.push({ bytes: packed.length, sha256: crypto.createHash('sha256').update(frame.bgr).digest('hex').slice(0, 16) });
          previous = frame.bgr;
        }
        const last = stored.length - 1;
        lines.push(JSON.stringify({ instant: j, frame: last, new: isNew, lost: false, width, height, zstd_bytes: stored[last].bytes, sha256: stored[last].sha256 }));
      }
      cdp.close();
    } finally {
      chrome.stop();
    }
    fs.writeFileSync(path.join(this.folder, 'frames.jsonl'), `${lines.join('\n')}\n`);
    const seconds = (Date.now() - began) / 1000;
    return this.writeIndex(stored, seconds, chrome.version, checks, url);
  }

  writeIndex(stored, seconds, version, checks, url) {
    const { width, height, sigma } = this.size;
    const nNew = stored.length;
    const nRep = this.instants - nNew;
    const dWait = stored.reduce((sum, s) => sum + s.bytes, 0);
    const meta = {
      scenario: this.scenario, page: this.page, window: [width, height], sigma, recorded: [sigma * width, sigma * height],
      instants: this.instants, instant_ms: CONFIG.instant_ms, virtual_start_unix_ms: CONFIG.virtual_start_unix_ms,
      s0_unix_ms: CONFIG.virtual_start_unix_ms + CONFIG.load_ms, url, zstd_level: CONFIG.zstd_level,
      renderer: 'scripts/clip-equation/corpus/render.mjs', chrome: version, render_seconds: seconds, checks,
    };
    fs.writeFileSync(path.join(this.folder, 'meta.json'), `${JSON.stringify(meta, null, 2)}\n`);
    const timeOk = checks.virtual_now.every((c) => c.expected === c.date_now) && checks.page_k_mismatch === 0;
    const row = `| ${this.scenario} | ${width}x${height} | ${sigma} | ${sigma * width}x${sigma * height} | ${this.instants} | ${nNew} | ${nRep} | ${(nNew / CONFIG.t_s).toFixed(2)} | ${Math.round(dWait / Math.max(1, nNew))} | ${dWait} | 0 | ${timeOk ? 'yes' : 'NO'} | ${seconds.toFixed(1)} |`;
    fs.writeFileSync(path.join(this.folder, 'row.md'), `${this.name}\t${row}\n`);
    return `${this.name} ${row}`;
  }
}

/** results/corpus.md from every capture folder's row.md. */
function writeSummary() {
  const rows = fs.readdirSync(ROOT)
    .map((name) => path.join(ROOT, name, 'row.md'))
    .filter((file) => fs.existsSync(file))
    .map((file) => fs.readFileSync(file, 'utf8').trimEnd())
    .sort();
  let total = 0;
  for (const name of fs.readdirSync(ROOT)) {
    const dir = path.join(ROOT, name);
    if (!fs.statSync(dir).isDirectory()) continue;
    for (const file of fs.readdirSync(dir)) total += fs.statSync(path.join(dir, file)).size;
  }
  const text = [
    '# Clip experiments, step 1: corpus',
    '',
    'Written by `node scripts/clip-equation/corpus/render.mjs` from the capture folders in `C:\\Users\\Adam\\clip-corpus\\` (not in the repository). Plan: docs/clip-recorder-experiments-plan.md, F6, F7, Procedure step 1. Checked by `node scripts/clip-equation/spec/run-spec.mjs step1`.',
    '',
    'Each capture: headless Chrome (`--headless=new`, own temporary profile, no window), virtual time advanced 50 ms per instant over the DevTools protocol, one lossless PNG screenshot per instant at the window size, device scale 1; t = 20 s at f = 20 instants (N_inst = 400). A frame equal in every pixel to the previous one is a repeat. New frames are stored at the window size as zstd level −3 of B,G,R, one file per new frame; `frames.jsonl` has one line per instant (its `sha256` is the first 16 hex digits of SHA-256 of the B,G,R bytes); `meta.json` records σ and the encoded size σ · window. n_new = new frames per second; bytes/frame = mean zstd −3 bytes of the stored frames; D_wait = their sum; lost = instants without a frame (always 0: every instant has a screenshot); time ok = `Date.now()` in the page equals s_j at the first and last instant, and on the flash page its counter shows j at every instant; render s = wall time of the capture.',
    '',
    `Total corpus size on disk: ${total} bytes (${(total / 1048576).toFixed(1)} MiB).`,
    '',
    '| folder | scenario | window | σ | encoded | instants | N_new | N_rep | n_new (1/s) | zstd −3 bytes/frame | D_wait bytes | lost | time ok | render s |',
    '|---|---|---|---|---|---|---|---|---|---|---|---|---|---|',
    ...rows.map((row) => {
      const [name, cells] = row.split('\t');
      return `| ${name} ${cells}`;
    }),
    '',
  ].join('\n');
  const file = path.join(BASE_DIR, 'results', 'corpus.md');
  fs.writeFileSync(file, text);
  console.log(`render summary=${file} total_bytes=${total}`);
}

async function main() {
  os.setPriority(os.constants.priority.PRIORITY_BELOW_NORMAL);
  const args = process.argv.slice(2);
  let suffix = '';
  const at = args.indexOf('--suffix');
  if (at >= 0) {
    suffix = args[at + 1] ?? '';
    args.splice(at, 2);
  }
  let failed = 0;
  for (const scenario of Object.keys(CONFIG.scenarios)) {
    for (const size of CONFIG.sizes) {
      const capture = new Capture(scenario, size, suffix);
      if (args.length && !args.includes(`${scenario}-${size.width}x${size.height}`)) continue;
      try {
        console.log(`render ${await capture.run()}`);
      } catch (error) {
        failed += 1;
        console.log(`render ${capture.name} error=${error.message}`);
      }
    }
  }
  writeSummary();
  process.exit(failed ? 1 : 0);
}

export { Cdp, HeadlessChrome, CONFIG };

if (import.meta.url === pathToFileURL(process.argv[1]).href) main();
