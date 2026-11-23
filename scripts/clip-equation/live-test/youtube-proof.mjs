// YouTube live proof of the dynamic clip recorder (docs/clip-recorder-architecture-dynamic.md
// section 8; docs/clip-recorder-plan-4.2.md section 12):
//
//   node youtube-proof.mjs <synapse-mcp exe> <scratch dir> [--url <watch url> | <watch url>]
//
// One run: a Chrome (or Edge) --app window opens a YouTube watch page (a video next to text), sized
// to 1920 x 1080 or the largest that fits the primary screen. A consent page is rejected and the
// muted video starts. The build under test runs as its own stdio MCP instance (--db and
// SYNAPSE_SHELL_JOB_ROOT in the scratch dir) and records the window for 60 s. During the recording
// the page is scrolled once and a search is typed, both over the DevTools protocol (never real
// input, never activation: mcp-stdio.mjs header); the video is then paused and a DevTools
// screenshot is taken. The window is closed in a finally block; one that cannot be closed makes
// the run exit 1 (owner rule: a test window is open only while it is recorded). The clip is then
// judged against the 22 scenarios of section 8.6, plus three figures reported beside them (frame
// timing, SSIM of the moving area, the exact layer against the screen). A PASS/FAIL table is
// printed and the run report is written next to the clip folder and as <scratch>/youtube-run.json.
// Exit 0 only when all 22 scenarios pass.
//
// A field that the builder has not written yet (schema 3: frames.jsonl x / mv / mv_rect, clip.json
// moving / exact / conditions) makes its scenarios "N/A" (not available), never a crash. N/A is
// not a pass.
//
// Deviations from section 8.4, on purpose:
// - The window is shown (behind all others) before the page is made ready: Chrome suspends a muted
//   video in a minimized window, so the play check would never succeed.
// - The window is closed as soon as the recording has stopped, before the clip is ready (owner
//   rule above). The ready time is still measured.
// - The judging is done here, because spec/dyn-youtube.mjs does not exist yet.
// - Cdp, TestWindow, until and alive are copies of d5-proof.mjs with the target predicate, the
//   browser and the window size as parameters. Build step 8 moves them into test-window.mjs, to be
//   shared by both drivers.
//
// Needs Node 22.15 or later (global WebSocket, zlib zstd), Chrome or Edge, and a network
// connection. No npm packages.
import { execFileSync, spawn } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import zlib from 'node:zlib';
import { McpServer, User32, clipStatus, sleep } from './mcp-stdio.mjs';

const PROGRAM_FILES = process.env.ProgramFiles ?? '';
const PROGRAM_FILES_X86 = process.env['ProgramFiles(x86)'] ?? '';
const LOCAL_APP_DATA = process.env.LOCALAPPDATA ?? '';

// Tunables of the run (section 8.3); times in ms, clip times counted from s_0.
const CONFIG = {
  // Big Buck Bunny (Blender Foundation). The id is not trusted: the run checks that a video plays.
  url: 'https://www.youtube.com/watch?v=aqz-KE-bpKQ',
  browserCandidates: [
    process.env.CHROME_PATH,
    path.join(PROGRAM_FILES, 'Google', 'Chrome', 'Application', 'chrome.exe'),
    path.join(PROGRAM_FILES_X86, 'Google', 'Chrome', 'Application', 'chrome.exe'),
    path.join(LOCAL_APP_DATA, 'Google', 'Chrome', 'Application', 'chrome.exe'),
    path.join(PROGRAM_FILES_X86, 'Microsoft', 'Edge', 'Application', 'msedge.exe'),
    path.join(PROGRAM_FILES, 'Microsoft', 'Edge', 'Application', 'msedge.exe'),
  ],
  // d5-proof.mjs flags (a covered window keeps rendering) plus section 8.3: the video starts
  // alone, makes no sound, and colours match the DevTools screenshot.
  browserFlags: ['--disable-backgrounding-occluded-windows', '--disable-renderer-backgrounding',
    '--disable-background-timer-throttling', '--no-first-run', '--no-default-browser-check',
    '--autoplay-policy=no-user-gesture-required', '--mute-audio', '--force-color-profile=srgb'],
  // Wanted window; shrunk to the primary screen when it does not fit.
  window: { x: 0, y: 0, width: 1920, height: 1080 },
  durationMs: 60000,
  playShotAtMs: 6000,     // DevTools screenshot while the video plays (SSIM of the moving area)
  scrollAtMs: 12000,
  scrollDeltaY: 400,
  scrollSettleMs: 2000,   // the video box is read again this long after the scroll
  typeAtMs: 30000,
  typeText: 'synapse clip',
  typeGapMs: 250,
  pauseAtMs: 48000,
  shotAtMs: 54000,        // the independent screenshot
  sampleMs: 1000,         // RAM and CPU sampling
  playWaitMs: 30000,      // how long to wait for a playing video
  playProbeMs: 500,       // currentTime must rise between two reads this far apart
  consentSettleMs: 1500,  // after the consent reject click
  paintMs: 500,
  waitMs: 15000,          // browser, DevTools, window lookups, end of recording
  statusLeadMs: 500,      // record_status polling starts this long before the planned end
  statusPollMs: 100,
  readyMs: 300000,        // clip until ready; also the deadline of one viewer call
  // clip_frame images (section 8.4 step 14): with the video, after the scroll, after typing, end.
  framesAtMs: { video: 5000, after_scroll: 20000, after_typing: 40000, end: 57000 },
  diffMs: [29000, 34000],
  selectors: {
    video: ['video.html5-main-video', 'video'],
    search: ['input[name="search_query"]', '#search input'],
    above: ['#above-the-fold', '#info'],
    secondary: ['#secondary'],
  },
  consent: {
    host: 'consent.youtube.com',
    dialog: 'ytd-consent-bump-v2-lightbox',
    reject: 'reject|ablehnen|refuser|rechazar|rifiuta|weiger|odrzu|recusar|דחי|דחה',
  },
};

// Known values of the pass criteria (section 8.6 and section 3.1).
const LIMITS = {
  fps: 20,
  mb: 1024 * 1024,
  minWidth: 1900,
  minHeight: 1000,
  ramMb: 500,
  cpuPercent: 10,          // CPU_LIMIT_PERCENT
  qualityLevel: 90,
  firstMs: 1000,           // scenarios 7 and 9 start here
  movingShare: 0.95,
  iou: 0.8,
  alignPx: 16,             // MOVING_ALIGN_PX
  blockPx: 8,              // BLOCK
  animatingNeed: 5,        // A
  holdInstants: 10,        // H
  pauseSlackInstants: 5,
  scrollRecordMs: 2000,
  scrollBoxMs: 3000,
  keyRecordMs: 300,
  typeMarginMs: 1000,
  scrollChangeLeadMs: 50,
  lastBytesMs: 2000,
  readyMs: 1000,
  bMax: { movingMbPerMin: 535, exactMbPerMin: 9.4, keyPictures: 4, bytesPerPixel: 3, metaMb: 0.3,
    fullArea: 1920 * 1080, linesPerMin: 1200 },
  minCropSide: 200,        // clip_frame refuses a crop whose short side is smaller (m1.rs)
  ssimMin: 0.95,           // plan 4.2 W8
};

const SW_SHOWMINNOACTIVE = 7;
const FRAMES = 'new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(() => r(true))))';
const PNG_SIGNATURE = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]);

const firstOf = (list) => `${JSON.stringify(list)}.map((s) => document.querySelector(s)).find(Boolean)`;
const READ_BOXES = `(() => {
  const sel = ${JSON.stringify(CONFIG.selectors)};
  const box = (list) => {
    for (const s of list) {
      const e = document.querySelector(s);
      if (!e) continue;
      const r = e.getBoundingClientRect();
      if (r.width > 0 && r.height > 0) return { x: r.x, y: r.y, width: r.width, height: r.height };
    }
    return null;
  };
  return { dpr: devicePixelRatio, inner: [innerWidth, innerHeight], scroll_y: scrollY, host: location.host,
    video: box(sel.video), search: box(sel.search), above: box(sel.above), secondary: box(sel.secondary) };
})()`;
const VIDEO_STATE = `(() => { const v = ${firstOf(CONFIG.selectors.video)}; return v ? { ready: v.readyState, paused: v.paused, t: v.currentTime } : null; })()`;
const VIDEO_PLAY = `(() => { const v = ${firstOf(CONFIG.selectors.video)}; if (v) { v.muted = true; v.play().catch(() => {}); } return Boolean(v); })()`;
const VIDEO_PAUSE = `(() => { const v = ${firstOf(CONFIG.selectors.video)}; if (v) v.pause(); return v ? v.paused : null; })()`;
const SEARCH_FOCUS = `(() => { const e = ${firstOf(CONFIG.selectors.search)}; if (!e) return false; e.focus(); return document.activeElement === e; })()`;
const SEARCH_VALUE = `(() => { const e = ${firstOf(CONFIG.selectors.search)}; return e ? e.value : null; })()`;
const BLUR = '(() => { const e = document.activeElement; if (e && e.blur) e.blur(); return true; })()';
const CONSENT = `(() => {
  const dialog = location.host === ${JSON.stringify(CONFIG.consent.host)} || Boolean(document.querySelector(${JSON.stringify(CONFIG.consent.dialog)}));
  if (!dialog) return 'none';
  const re = new RegExp(${JSON.stringify(CONFIG.consent.reject)}, 'i');
  const b = [...document.querySelectorAll('button, tp-yt-paper-button, [role=button]')]
    .find((e) => re.test((e.getAttribute('aria-label') || '') + ' ' + (e.innerText || '')));
  if (!b) return 'no_button';
  b.click();
  return 'clicked';
})()`;

/** Polls `probe` until it gives a truthy value (an error counts as not yet); throws after `ms`. */
async function until(probe, what, ms = CONFIG.waitMs, every = 250) {
  const end = Date.now() + ms;
  let last = null;
  while (Date.now() < end) {
    try {
      const value = await probe();
      if (value) return value;
    } catch (error) { last = error; }
    await sleep(every);
  }
  throw new Error(`timed out after ${ms} ms waiting for ${what}${last ? `: ${last.message}` : ''}`);
}

/** `true` while process `pid` exists. */
function alive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return error.code === 'EPERM';
  }
}

const slash = (p) => p.replace(/\\/g, '/');
const readJson = (file) => (fs.existsSync(file) ? JSON.parse(fs.readFileSync(file, 'utf8')) : null);
const writeJson = (file, value) => fs.writeFileSync(file, JSON.stringify(value, null, 1));
const tryJson = (text) => { try { return JSON.parse(text); } catch { return null; } };
const fixed = (n, digits = 2) => (Number.isFinite(n) ? n.toFixed(digits) : 'n/a');
const powershell = (command) => execFileSync('powershell', ['-NoProfile', '-Command', command], { encoding: 'utf8', windowsHide: true }).trim();

/** Rectangles as [x, y, w, h] in picture pixels; null is "no rectangle". */
const Rect = {
  area: (r) => (r ? Math.max(0, r[2]) * Math.max(0, r[3]) : 0),
  intersect(a, b) {
    if (!a || !b) return null;
    const x0 = Math.max(a[0], b[0]);
    const y0 = Math.max(a[1], b[1]);
    const x1 = Math.min(a[0] + a[2], b[0] + b[2]);
    const y1 = Math.min(a[1] + a[3], b[1] + b[3]);
    return x1 > x0 && y1 > y0 ? [x0, y0, x1 - x0, y1 - y0] : null;
  },
  iou(a, b) {
    const both = Rect.area(Rect.intersect(a, b));
    const union = Rect.area(a) + Rect.area(b) - both;
    return union > 0 ? both / union : 0;
  },
  grow: (r, px) => [r[0] - px, r[1] - px, r[2] + 2 * px, r[3] + 2 * px],
  inside: (a, b) => Boolean(a && b) && a[0] >= b[0] && a[1] >= b[1] && a[0] + a[2] <= b[0] + b[2] && a[1] + a[3] <= b[1] + b[3],
  equal: (a, b) => (!a && !b) || (Boolean(a && b) && a.length === b.length && a.every((v, i) => v === b[i])),
  fromObject: (o) => (o ? [o.x, o.y, o.w, o.h] : null),
  text: (r) => (r ? `[${r.join(', ')}]` : 'none'),
  /** True when one of the row bands [first, count] meets the rows y .. y + h. */
  rowsMeet: (bands, y, h) => bands.some(([first, count]) => first < y + h && first + count > y),
  /** True when the row bands cover every row y .. y + h. */
  rowsCover(bands, y, h) {
    for (let row = y; row < y + h; row++) {
      if (!bands.some(([first, count]) => row >= first && row < first + count)) return false;
    }
    return true;
  },
};

/** One DevTools protocol connection (Node's global WebSocket). Copy of d5-proof.mjs. */
class Cdp {
  static async connect(url) {
    const ws = new WebSocket(url);
    await new Promise((resolve, reject) => {
      ws.onopen = resolve;
      ws.onerror = () => reject(new Error(`DevTools connection failed: ${url}`));
    });
    return new Cdp(ws);
  }

  constructor(ws) {
    this.ws = ws; this.id = 0; this.wait = new Map();
    ws.onmessage = (event) => {
      const m = JSON.parse(event.data);
      const w = this.wait.get(m.id);
      if (!w) return;
      this.wait.delete(m.id);
      if (m.error) w.reject(new Error(`${m.error.message} (${m.error.code})`)); else w.resolve(m.result);
    };
    ws.onclose = () => { for (const w of this.wait.values()) w.reject(new Error('DevTools connection closed')); this.wait.clear(); };
  }

  send(method, params = {}) {
    return new Promise((resolve, reject) => {
      const id = ++this.id;
      const timer = setTimeout(() => { this.wait.delete(id); reject(new Error(`${method}: no answer in ${CONFIG.waitMs} ms`)); }, CONFIG.waitMs);
      this.wait.set(id, { resolve: (v) => { clearTimeout(timer); resolve(v); }, reject: (e) => { clearTimeout(timer); reject(e); } });
      this.ws.send(JSON.stringify({ id, method, params }));
    });
  }

  /** The JSON value of a page expression; a promise is awaited. */
  async evaluate(expression) {
    const r = await this.send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
    if (r.exceptionDetails) throw new Error(`page: ${r.exceptionDetails.exception?.description ?? r.exceptionDetails.text}`);
    return r.result.value;
  }

  close() { try { this.ws.close(); } catch { } }
}

/** A test window that could not be closed (owner rule, never a window left open). */
class CleanupError extends Error {}

/** One browser --app window with its own profile, driven over DevTools. From d5-proof.mjs. */
class TestWindow {
  /**
   * Launches the window minimized without activating it at `rect` (x, y, width, height), keeps
   * its browser PID (the process of that profile without --type=) and connects to its http(s)
   * page.
   */
  static async open({ browser, url, profileDir, rect }) {
    fs.mkdirSync(profileDir, { recursive: true });
    const args = [`--app=${url}`, `--user-data-dir=${profileDir}`, '--remote-debugging-port=0',
      `--window-position=${rect.x},${rect.y}`, `--window-size=${rect.width},${rect.height}`, ...CONFIG.browserFlags];
    const line = [browser, ...args].map((a) => `"${a}"`).join(' ').replace(/'/g, "''");
    const win = new TestWindow(browser, profileDir);
    try {
      powershell(`(New-Object -ComObject WScript.Shell).Run('${line}', ${SW_SHOWMINNOACTIVE}, $false) | Out-Null`);
      win.pid = (await until(() => win.processes().find((p) => p.browser), 'the browser process')).pid;
      await win.connect();
    } catch (error) {
      await win.close();
      throw error;
    }
    return win;
  }

  constructor(browser, profileDir) {
    this.browser = browser; this.profileDir = profileDir; this.cdp = null; this.pid = null; this.hwnd = null;
  }

  /** The processes of this profile: pid, browser (no --type=), main window (0 if none). */
  processes() {
    const name = path.basename(this.browser).replace(/'/g, "''");
    const dir = this.profileDir.replace(/'/g, "''");
    const out = powershell(`Get-CimInstance Win32_Process -Filter "Name = '${name}'" | Where-Object { $_.CommandLine -and $_.CommandLine.Contains('${dir}') } | ForEach-Object { $b = -not $_.CommandLine.Contains('--type='); $h = if ($b) { (Get-Process -Id $_.ProcessId -ErrorAction SilentlyContinue).MainWindowHandle } else { 0 }; '{0} {1} {2}' -f $_.ProcessId, [int]$b, $h }`);
    return out.split(/\r?\n/).filter((l) => l.trim()).map((l) => {
      const [pid, browser, hwnd] = l.trim().split(/\s+/).map(Number);
      return { pid, browser: browser === 1, hwnd: hwnd || 0 };
    });
  }

  async connect() {
    const portFile = path.join(this.profileDir, 'DevToolsActivePort');
    const port = await until(() => fs.existsSync(portFile) && Number(fs.readFileSync(portFile, 'utf8').split(/\r?\n/)[0]), 'DevToolsActivePort');
    const target = await until(async () => {
      const targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
      return targets.find((t) => t.type === 'page' && /^https?:/.test(t.url));
    }, 'the page target');
    this.cdp = await Cdp.connect(target.webSocketDebuggerUrl);
    // The page behaves as focused although its window never is (typing, caret, focus events).
    await this.cdp.send('Emulation.setFocusEmulationEnabled', { enabled: true });
    this.hwnd = (await until(() => this.processes().find((p) => p.pid === this.pid && p.hwnd), 'the browser window')).hwnd;
  }

  read(expression) { return this.cdp.evaluate(expression); }

  /** Shows the window without activating it, lets it paint, then puts it behind all others. */
  async placeBehind() {
    User32.restoreWithoutActivating(this.hwnd);
    await this.read(FRAMES);
    await sleep(CONFIG.paintMs);
    User32.sendToBack(this.hwnd);
  }

  /** A DevTools PNG of the page viewport, saved to `file`, with the wall times around it. */
  async screenshot(file) {
    const before = Date.now();
    const r = await this.cdp.send('Page.captureScreenshot', { format: 'png' });
    const after = Date.now();
    fs.writeFileSync(file, Buffer.from(r.data, 'base64'));
    return { file, before, after };
  }

  /**
   * Ends the launch PID's process tree and every process of the profile, then verifies that none
   * is left; throws CleanupError otherwise.
   */
  async close() {
    this.cdp?.close();
    let left = [];
    try {
      left = this.processes().map((p) => p.pid);
    } catch { /* the check below asks again */ }
    for (const pid of new Set([this.pid, ...left].filter(Boolean))) {
      try { execFileSync('taskkill', ['/PID', String(pid), '/T', '/F'], { stdio: 'ignore', windowsHide: true }); } catch { /* already gone */ }
    }
    try {
      await until(() => !(this.pid && alive(this.pid)) && this.processes().length === 0, 'the browser to exit');
    } catch (error) {
      throw new CleanupError(`test window not closed (browser pid ${this.pid ?? 'unknown'}, profile ${this.profileDir}): ${error.message}`);
    }
  }
}

/** Private bytes and processor time of one process: one read, or a sampler every sampleMs. */
class ProcessProbe {
  static read(pid) {
    const out = powershell(`$p = Get-Process -Id ${pid}; '{0} {1}' -f $p.PrivateMemorySize64, [long]$p.TotalProcessorTime.TotalMilliseconds`);
    const [privateBytes, cpuMs] = out.split(/\s+/).map(Number);
    return { wall: Date.now(), private_bytes: privateBytes, cpu_ms: cpuMs };
  }

  constructor(pid) { this.pid = pid; this.samples = []; this.child = null; this.buf = ''; }

  /** One PowerShell loop for the whole recording, so sampling does not start a process a second. */
  start() {
    const loop = `$p = Get-Process -Id ${this.pid}; while ($true) { $p.Refresh(); if ($p.HasExited) { break }; '{0} {1} {2}' -f [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds(), $p.PrivateMemorySize64, [long]$p.TotalProcessorTime.TotalMilliseconds; Start-Sleep -Milliseconds ${CONFIG.sampleMs} }`;
    this.child = spawn('powershell', ['-NoProfile', '-Command', loop], { stdio: ['ignore', 'pipe', 'ignore'], windowsHide: true });
    this.child.stdout.on('data', (d) => {
      this.buf += d;
      let i;
      while ((i = this.buf.indexOf('\n')) >= 0) {
        const [wall, privateBytes, cpuMs] = this.buf.slice(0, i).trim().split(/\s+/).map(Number);
        this.buf = this.buf.slice(i + 1);
        if (Number.isFinite(privateBytes)) this.samples.push({ wall, private_bytes: privateBytes, cpu_ms: cpuMs });
      }
    });
    return this;
  }

  stop() {
    if (this.child && this.child.exitCode === null) {
      try { execFileSync('taskkill', ['/PID', String(this.child.pid), '/T', '/F'], { stdio: 'ignore', windowsHide: true }); } catch { }
    }
  }
}

/** The separate synapse-mcp instance and the clip operations the run uses. */
class Recorder {
  constructor(exe, scratch) {
    const db = path.join(scratch, 'db');
    const jobs = path.join(scratch, 'shell-jobs');
    for (const dir of [db, jobs]) fs.mkdirSync(dir, { recursive: true });
    this.srv = new McpServer({ exe, db: path.join(db, 'synapse.db'), outDir: scratch, tag: 'mcp', env: { SYNAPSE_SHELL_JOB_ROOT: jobs } });
  }

  get pid() { return this.srv.p.pid; }

  async start() {
    const r = await this.srv.init('youtube-proof');
    if (r.error) throw new Error(`synapse-mcp did not start: ${JSON.stringify(r.error)}`);
  }

  /** Starts recording `hwnd` into `folder` (not existing yet); the record response and s_0. */
  async record(hwnd, durationMs, folder) {
    const rec = await this.srv.call({ operation: 'record', window_hwnd: hwnd, duration_ms: durationMs, folder: slash(folder) });
    const r = rec.json?.record;
    if (!r?.clip_id) throw new Error(`record refused: ${rec.texts.join(' ').slice(0, 600) || JSON.stringify(rec.error)}`);
    const s0 = await until(() => readJson(path.join(r.folder, 'clip.json'))?.clock?.wall_clock_unix_ms_at_s0 || null, 'the clock in clip.json');
    return { ...r, s0 };
  }

  async status(clipId) {
    const s = await clipStatus(this.srv, clipId);
    return { state: s.state, seconds: s.seconds, stop: s.stop, raw: s.raw, wall: Date.now() };
  }

  /** Waits for the planned end, then until the clip no longer records. */
  async waitStopped(clipId, endUnixMs) {
    await sleep(Math.max(0, endUnixMs - CONFIG.statusLeadMs - Date.now()));
    return until(async () => {
      const s = await this.status(clipId);
      return s.state !== 'recording' ? s : null;
    }, 'the recording to end', CONFIG.waitMs + CONFIG.statusLeadMs, CONFIG.statusPollMs);
  }

  /** The first status that says ready or failed, with its wall time. */
  waitReady(clipId) {
    return until(async () => {
      const s = await this.status(clipId);
      return ['ready', 'failed'].includes(s.state) ? s : null;
    }, `clip ${clipId} to be ready`, CONFIG.readyMs, CONFIG.statusPollMs);
  }

  /** One clip viewer operation on `folder`: the clip_view response or the error. */
  async view(folder, args) {
    try {
      const r = await this.srv.call({ folder: slash(folder), ...args }, CONFIG.readyMs);
      const view = r.texts.map(tryJson).find((j) => j?.clip_view)?.clip_view ?? null;
      const failed = r.error || r.isError || !view;
      return { args, ok: !failed, view, text: r.texts[0]?.slice(0, 4000) ?? '',
        error: failed ? (r.error ? JSON.stringify(r.error) : r.texts.join(' ').slice(0, 600)) : null };
    } catch (error) {
      return { args, ok: false, view: null, text: '', error: String(error?.message ?? error) };
    }
  }

  close() { return this.srv.close(); }
}

/** A decoded picture: 8-bit gray, RGB or RGBA (PNG), or BGR (the exact layer). */
class Picture {
  constructor(width, height, channels, data, bgr = false) {
    this.width = width; this.height = height; this.channels = channels; this.data = data; this.bgr = bgr;
  }

  /** [r, g, b] of pixel (x, y). */
  px(x, y) {
    const d = this.data;
    const i = (y * this.width + x) * this.channels;
    if (this.channels < 3) return [d[i], d[i], d[i]];
    return this.bgr ? [d[i + 2], d[i + 1], d[i]] : [d[i], d[i + 1], d[i + 2]];
  }

  /** True when every pixel of `r` has the colour of its first pixel. */
  flat(r) {
    const first = this.px(r[0], r[1]);
    for (let y = r[1]; y < r[1] + r[3]; y++) {
      for (let x = r[0]; x < r[0] + r[2]; x++) {
        const p = this.px(x, y);
        if (p[0] !== first[0] || p[1] !== first[1] || p[2] !== first[2]) return false;
      }
    }
    return true;
  }
}

/** Minimal PNG reader (8-bit, not interlaced) and RGB writer; no npm packages. */
class Png {
  static decode(buf) {
    if (!buf.subarray(0, 8).equals(PNG_SIGNATURE)) throw new Error('not a PNG');
    let pos = 8; let header = null; const idat = [];
    while (pos + 8 <= buf.length) {
      const length = buf.readUInt32BE(pos);
      const kind = buf.toString('latin1', pos + 4, pos + 8);
      const data = buf.subarray(pos + 8, pos + 8 + length);
      if (kind === 'IHDR') header = { width: data.readUInt32BE(0), height: data.readUInt32BE(4), depth: data[8], type: data[9], interlace: data[12] };
      else if (kind === 'IDAT') idat.push(data);
      else if (kind === 'IEND') break;
      pos += 12 + length;
    }
    const channels = { 0: 1, 2: 3, 4: 2, 6: 4 }[header?.type];
    if (!header || header.depth !== 8 || header.interlace !== 0 || !channels) throw new Error(`unsupported PNG ${JSON.stringify(header)}`);
    const { width, height } = header;
    const raw = zlib.inflateSync(Buffer.concat(idat));
    const stride = width * channels;
    const out = Buffer.alloc(stride * height);
    let prev = Buffer.alloc(stride);
    for (let y = 0; y < height; y++) {
      const filter = raw[y * (stride + 1)];
      const line = raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1));
      const cur = out.subarray(y * stride, (y + 1) * stride);
      for (let i = 0; i < stride; i++) {
        const a = i >= channels ? cur[i - channels] : 0;
        const b = prev[i];
        const c = i >= channels ? prev[i - channels] : 0;
        let v = line[i];
        if (filter === 1) v += a;
        else if (filter === 2) v += b;
        else if (filter === 3) v += (a + b) >> 1;
        else if (filter === 4) {
          const p = a + b - c; const pa = Math.abs(p - a); const pb = Math.abs(p - b); const pc = Math.abs(p - c);
          v += pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
        }
        cur[i] = v & 255;
      }
      prev = cur;
    }
    return new Picture(width, height, channels, out);
  }

  static read(file) { return Png.decode(fs.readFileSync(file)); }

  static encodeRgb(width, height, rgb) {
    const stride = width * 3;
    const raw = Buffer.alloc((stride + 1) * height);
    for (let y = 0; y < height; y++) rgb.copy(raw, y * (stride + 1) + 1, y * stride, (y + 1) * stride);
    const chunk = (kind, data) => {
      const length = Buffer.alloc(4); length.writeUInt32BE(data.length);
      const body = Buffer.concat([Buffer.from(kind, 'latin1'), data]);
      const crc = Buffer.alloc(4); crc.writeUInt32BE(Png.crc32(body));
      return Buffer.concat([length, body, crc]);
    };
    const ihdr = Buffer.alloc(13);
    ihdr.writeUInt32BE(width, 0); ihdr.writeUInt32BE(height, 4); ihdr[8] = 8; ihdr[9] = 2;
    return Buffer.concat([PNG_SIGNATURE, chunk('IHDR', ihdr), chunk('IDAT', zlib.deflateSync(raw)), chunk('IEND', Buffer.alloc(0))]);
  }

  static crc32(buf) {
    Png.table ??= Array.from({ length: 256 }, (_, n) => {
      let c = n;
      for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
      return c >>> 0;
    });
    let crc = 0xffffffff;
    for (const byte of buf) crc = Png.table[(crc ^ byte) & 255] ^ (crc >>> 8);
    return (crc ^ 0xffffffff) >>> 0;
  }
}

/** Pixel comparisons of two pictures over equal-sized areas. */
class PixelCheck {
  /** Exact comparison of `a` at (ax, ay) with `b` at (bx, by), size w x h; skips pixels in `skip`. */
  static compare(a, ax, ay, b, bx, by, w, h, skip = null) {
    let differing = 0; let max = 0; let pixels = 0;
    const diff = Buffer.alloc(w * h * 3);
    for (let y = 0; y < h; y++) {
      for (let x = 0; x < w; x++) {
        if (skip && ax + x >= skip[0] && ax + x < skip[0] + skip[2] && ay + y >= skip[1] && ay + y < skip[1] + skip[3]) continue;
        const p = a.px(ax + x, ay + y);
        const q = b.px(bx + x, by + y);
        const d = [Math.abs(p[0] - q[0]), Math.abs(p[1] - q[1]), Math.abs(p[2] - q[2])];
        const m = Math.max(...d);
        pixels++;
        if (m > 0) differing++;
        if (m > max) max = m;
        const i = (y * w + x) * 3;
        diff[i] = Math.min(255, d[0] * 8); diff[i + 1] = Math.min(255, d[1] * 8); diff[i + 2] = Math.min(255, d[2] * 8);
      }
    }
    return { pixels, differing, max_difference: max, diff, width: w, height: h };
  }

  /** Mean SSIM of the luma over 8 x 8 windows. */
  static ssim(a, ax, ay, b, bx, by, w, h, win = 8) {
    const c1 = (0.01 * 255) ** 2; const c2 = (0.03 * 255) ** 2; const n = win * win;
    const luma = (p) => 0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2];
    let sum = 0; let count = 0;
    for (let y0 = 0; y0 + win <= h; y0 += win) {
      for (let x0 = 0; x0 + win <= w; x0 += win) {
        let sa = 0; let sb = 0; let saa = 0; let sbb = 0; let sab = 0;
        for (let y = y0; y < y0 + win; y++) {
          for (let x = x0; x < x0 + win; x++) {
            const p = luma(a.px(ax + x, ay + y)); const q = luma(b.px(bx + x, by + y));
            sa += p; sb += q; saa += p * p; sbb += q * q; sab += p * q;
          }
        }
        const ma = sa / n; const mb = sb / n;
        const va = saa / n - ma * ma; const vb = sbb / n - mb * mb; const cov = sab / n - ma * mb;
        sum += ((2 * ma * mb + c1) * (2 * cov + c2)) / ((ma * ma + mb * mb + c1) * (va + vb + c2));
        count++;
      }
    }
    return count ? sum / count : null;
  }

  /** Rows at the top of a clip viewer image covered by its label box (clip_viewer.rs draw_label). */
  static labelRows(image) {
    return (Math.min(image.width, image.height) >= 600 ? 3 : 2) * 9;
  }
}

/**
 * Page CSS px to picture px as the D5 driver does it (d5-proof.mjs viewport): the captured frame
 * is the whole window, the caption above the page and equal borders left, right and below.
 */
class PageMap {
  constructor(geometry, page) {
    const [width, height] = page.inner;
    this.scale = geometry.scale ?? 1;
    this.dpr = page.dpr;
    this.side = (geometry.window_width - width * page.dpr) / 2;
    this.top = geometry.window_height - height * page.dpr - this.side;
    this.viewport = [this.side, this.top, width * page.dpr, height * page.dpr].map((v) => Math.round(v * this.scale));
  }

  /** The page box in picture px, cut to the visible viewport. */
  rect(box) {
    if (!box) return null;
    const s = this.scale;
    const r = [(this.side + box.x * this.dpr) * s, (this.top + box.y * this.dpr) * s, box.width * this.dpr * s, box.height * this.dpr * s].map(Math.round);
    return Rect.intersect(r, this.viewport);
  }

  /** A picture rect grown to the viewer's minimum crop side, kept inside the viewport. */
  crop(r) {
    if (!r) return null;
    let [x, y, w, h] = r;
    const [vx, vy, vw, vh] = this.viewport;
    if (w < LIMITS.minCropSide) { w = Math.min(LIMITS.minCropSide, vw); x = Math.min(x, vx + vw - w); }
    if (h < LIMITS.minCropSide) { h = Math.min(LIMITS.minCropSide, vh); y = Math.min(y, vy + vh - h); }
    return { x: Math.max(x, vx), y: Math.max(y, vy), w, h };
  }
}

/** Exact layer records (section 4.2): row bands, and their pixels applied to the picture E. */
class ExactRecord {
  static bands(payload) {
    const count = payload.readUInt16LE(0);
    return Array.from({ length: count }, (_, i) => [payload.readUInt16LE(2 + 4 * i), payload.readUInt16LE(4 + 4 * i)]);
  }

  static apply(payload, bands, canvas, width, height) {
    let at = 2 + 4 * bands.length;
    for (const [first, count] of bands) {
      const length = count * width * 3;
      if (first + count <= height && at + length <= payload.length) payload.copy(canvas, first * width * 3, at, at + length);
      at += length;
    }
  }

  /** A whole group file; an open group (no end mark) is decoded as far as its bytes go. */
  static inflate(buf) {
    try {
      return Promise.resolve(zlib.zstdDecompressSync(buf));
    } catch {
      return new Promise((resolve) => {
        const parts = [];
        const d = zlib.createZstdDecompress();
        const done = () => resolve(Buffer.concat(parts));
        d.on('data', (c) => parts.push(c));
        d.on('end', done);
        d.on('error', done);
        d.end(buf);
      });
    }
  }
}

/** What the run reads from the clip folder: clip.json, frames.jsonl, byte sizes, the layers. */
class ClipData {
  constructor(folder) {
    this.folder = folder;
    this.meta = readJson(path.join(folder, 'clip.json')) ?? {};
    const jsonl = path.join(folder, 'frames.jsonl');
    // A torn last line is skipped, as the product's reader does.
    this.lines = fs.existsSync(jsonl)
      ? fs.readFileSync(jsonl, 'utf8').split(/\r?\n/).map(tryJson).filter((l) => l && Number.isInteger(l.instant))
      : [];
    this.byInstant = new Map(this.lines.map((l) => [l.instant, l]));
    this.fps = this.meta.fps || LIMITS.fps;
    this.frameMs = 1000 / this.fps;
    const g = this.meta.geometry ?? {};
    this.geometry = { window_width: g.window_width ?? this.lines[0]?.width, window_height: g.window_height ?? this.lines[0]?.height, scale: g.scale ?? 1 };
    this.hasMoving = this.meta.moving != null;
    this.segments = Array.isArray(this.meta.moving?.segments) ? this.meta.moving.segments : [];
    this.hasExact = this.lines.some((l) => Array.isArray(l.x));
    this.rects = this.movingRects();
    this.bytes = this.measureBytes();
  }

  instantAt(ms) { return Math.floor(ms / this.frameMs); }
  timeOf(k) { return k * this.frameMs; }
  rectAt(k) { return this.rects.get(k) ?? null; }
  condition(name) { return this.meta.conditions?.[name] ?? this.meta.conditions?.[name.toUpperCase()] ?? null; }

  /** The moving rectangle of every instant with `mv`, from clip.json segments or `mv_rect`. */
  movingRects() {
    const bySegment = new Map(this.segments.map((s) => [s.index, s.rect]));
    const rects = new Map();
    for (const line of this.lines) {
      if (!Array.isArray(line.mv)) continue;
      const [segment] = line.mv;
      if (!bySegment.has(segment) && Array.isArray(line.mv_rect)) bySegment.set(segment, line.mv_rect);
      rects.set(line.instant, bySegment.get(segment) ?? null);
    }
    return rects;
  }

  measureBytes() {
    const sum = (dir, keep) => (fs.existsSync(dir)
      ? fs.readdirSync(dir).filter(keep).reduce((s, f) => s + fs.statSync(path.join(dir, f)).size, 0) : 0);
    const size = (file) => (fs.existsSync(file) ? fs.statSync(file).size : 0);
    const exact = sum(path.join(this.folder, 'frames'), () => true);
    const moving = sum(path.join(this.folder, 'moving'), (f) => f.endsWith('.mp4'));
    const jsonl = size(path.join(this.folder, 'frames.jsonl'));
    const clipJson = size(path.join(this.folder, 'clip.json'));
    return { exact, moving, jsonl, clip_json: clipJson, total: exact + moving + jsonl + clipJson };
  }

  /**
   * Decodes every exact record: its row bands per instant. Builds E at the last record at or
   * before instant `target` (the picture of the exact layer at that instant).
   */
  async decodeExact(target) {
    const refs = this.lines.filter((l) => Array.isArray(l.x));
    const records = new Map(); const errors = [];
    let picture = null;
    if (typeof zlib.zstdDecompressSync !== 'function') return { records, picture, errors: ['this Node has no zstd (needs 22.15 or later)'] };
    const last = refs.filter((l) => l.instant <= target).at(-1);
    for (const [group, list] of Map.groupBy(refs, (l) => l.x[0])) {
      list.sort((a, b) => a.x[1] - b.x[1]);
      const file = path.join(this.folder, 'frames', `group-${String(group).padStart(6, '0')}.zst`);
      let decoded;
      try {
        decoded = await ExactRecord.inflate(fs.readFileSync(file));
      } catch (error) {
        errors.push(`group ${group}: ${error.message}`);
        continue;
      }
      const build = last && last.x[0] === group;
      let canvas = null; let at = 0;
      for (const line of list) {
        const [, offset, bytes, raw] = line.x;
        if (at + raw > decoded.length) {
          errors.push(`group ${group}: the record of instant ${line.instant} lies past the ${decoded.length} decoded bytes`);
          break;
        }
        const payload = decoded.subarray(at, at + raw);
        at += raw;
        const bands = ExactRecord.bands(payload);
        records.set(line.instant, { bands, key: offset === 0, group, bytes, raw });
        if (build && line.instant <= target) {
          canvas ??= Buffer.alloc(line.width * line.height * 3);
          ExactRecord.apply(payload, bands, canvas, line.width, line.height);
          if (line.instant === last.instant) picture = { image: new Picture(line.width, line.height, 3, canvas, true), instant: line.instant };
        }
      }
    }
    return { records, picture, errors };
  }
}

const row = (name, status, value, need) => ({ name, status, value, need });
const verdict = (ok) => (ok ? 'PASS' : 'FAIL');
const NO_MOVING = 'clip.json has no moving block (schema 3 not written yet)';
const NO_EXACT = 'frames.jsonl has no x field (exact layer not written yet)';

/** The 22 scenarios of section 8.6, and three figures reported beside them. */
class Judge {
  /** `f`: the facts the run gathered (see YoutubeProof.facts). */
  constructor(f) {
    this.f = f; this.clip = f.clip; this.at1080 = false;
  }

  scenarios() {
    return [this.complete(), this.nothingLost(), this.resolution(), this.ram(), this.cpu(), this.encoder(),
      this.videoMoves(), this.boxIsVideo(), this.textExact(), this.scrollStored(), this.boxAfterScroll(),
      this.typing(), this.stop(), this.exactAfterStop(), this.nothingWritten(), this.sampleCount(),
      this.sizeBound(), this.pictureIsScreen(), this.viewerSees(), this.videoInClip(), this.readyTime(),
      this.windowClosed()].map((r, i) => ({ id: String(i + 1), ...r }));
  }

  extras() {
    return [this.frameTiming(), this.movingSsim(), this.exactIsScreen()].map((r, i) => ({ id: `x${i + 1}`, ...r }));
  }

  /** Instants whose time lies in [from, to). */
  instantsIn(from, to) {
    return this.clip.lines.filter((l) => this.clip.timeOf(l.instant) >= from && this.clip.timeOf(l.instant) < to);
  }

  lastMoving() {
    const moving = this.clip.lines.filter((l) => Array.isArray(l.mv));
    return moving.length ? moving.at(-1).instant : null;
  }

  complete() {
    const m = this.clip.meta;
    const lines = CONFIG.durationMs / this.clip.frameMs;
    const ok = this.f.ready?.state === 'ready' && m.state === 'ready' && m.stop_reason === 'completed'
      && m.recorded_ms === CONFIG.durationMs && this.clip.lines.length === lines;
    return row('The clip is complete', verdict(ok),
      `status ${this.f.ready?.state}, clip.json ${m.state}, stop ${m.stop_reason}, recorded ${m.recorded_ms} ms, ${this.clip.lines.length} lines`,
      `ready, completed, ${CONFIG.durationMs} ms, ${lines} lines`);
  }

  nothingLost() {
    const c = this.clip.condition('c'); const t = this.clip.condition('t');
    const lost = Array.isArray(this.clip.meta.lost) ? this.clip.meta.lost.length : null;
    const lostLines = this.clip.lines.filter((l) => l.lost).length;
    const value = `lost ${lost ?? 'absent'}, lines with lost ${lostLines}, C ${c?.status ?? 'absent'}, T ${t?.status ?? 'absent'}`;
    if (!c || !t || lost === null) return row('Nothing was lost', 'N/A', `${value} (field not written)`, 'lost empty; C and T held');
    return row('Nothing was lost', verdict(lost === 0 && lostLines === 0 && c.status === 'held' && t.status === 'held'), value, 'lost empty; C and T held');
  }

  resolution() {
    const w = this.clip.lines[0]?.width ?? this.clip.geometry.window_width;
    const h = this.clip.lines[0]?.height ?? this.clip.geometry.window_height;
    this.at1080 = w >= LIMITS.minWidth && h >= LIMITS.minHeight;
    return row('The test ran at 1080p', verdict(this.at1080), `${w} x ${h}`, `>= ${LIMITS.minWidth} x ${LIMITS.minHeight}`);
  }

  ram() {
    const need = `<= ${LIMITS.ramMb} MB`;
    const { m0, samples } = this.f.process;
    const during = samples.filter((s) => s.wall >= this.f.s0 && s.wall <= (this.f.ready?.wall ?? Infinity));
    if (!m0 || !during.length) return row('RAM', 'N/A', 'no samples', need);
    const mb = (Math.max(...during.map((s) => s.private_bytes)) - m0.private_bytes) / LIMITS.mb;
    const value = `${fixed(mb, 1)} MB over M_0 ${fixed(m0.private_bytes / LIMITS.mb, 1)} MB (${during.length} samples)`;
    if (!this.at1080) return row('RAM', 'BLOCKED', `${value}; not 1080p`, need);
    return row('RAM', verdict(mb <= LIMITS.ramMb), value, need);
  }

  cpu() {
    const need = `<= ${LIMITS.cpuPercent} %; B held`;
    const { cpu0, cpu1 } = this.f.process;
    const b = this.clip.condition('b');
    if (!cpu0 || !cpu1) return row('CPU', 'N/A', 'no processor time read', need);
    const seconds = (this.clip.meta.recorded_ms ?? CONFIG.durationMs) / 1000;
    const share = (100 * (cpu1.cpu_ms - cpu0.cpu_ms)) / (seconds * 1000 * os.cpus().length);
    const value = `${fixed(share)} % of ${os.cpus().length} logical processors; B ${b?.status ?? 'absent'}`;
    if (!this.at1080) return row('CPU', 'BLOCKED', `${value}; not 1080p`, need);
    if (!b) return row('CPU', 'N/A', `${value} (condition B not written)`, need);
    return row('CPU', verdict(share <= LIMITS.cpuPercent && b.status === 'held'), value, need);
  }

  encoder() {
    const need = `available, hardware, quality ${LIMITS.qualityLevel}, Q held`;
    if (!this.clip.hasMoving) return row('The encoder', 'N/A', NO_MOVING, need);
    const moving = this.clip.meta.moving; const q = this.clip.condition('q');
    const detail = q?.detail ?? {}; const grant = moving.encoder ?? {};
    const hardware = detail.hardware ?? grant.hardware;
    const level = detail.quality_level ?? grant.quality_level;
    const ok = moving.status === 'available' && hardware === true && level === LIMITS.qualityLevel && q?.status === 'held';
    return row('The encoder', verdict(ok), `status ${moving.status}${moving.reason ? ` (${moving.reason})` : ''}, hardware ${hardware}, quality ${level}, Q ${q?.status ?? 'absent'}`, need);
  }

  videoMoves() {
    const need = `>= ${LIMITS.movingShare * 100} % of [${LIMITS.firstMs}, scroll) with mv`;
    if (!this.clip.hasMoving) return row('The video moves', 'N/A', NO_MOVING, need);
    const scroll = this.f.events.scroll?.clip_ms;
    if (!Number.isFinite(scroll)) return row('The video moves', 'FAIL', 'the scroll was not sent', need);
    const span = this.instantsIn(LIMITS.firstMs, scroll);
    const share = span.length ? span.filter((l) => Array.isArray(l.mv)).length / span.length : 0;
    return row('The video moves', verdict(share >= LIMITS.movingShare), `${fixed(share * 100, 1)} % of ${span.length} instants`, need);
  }

  boxIsVideo() {
    const need = `IoU >= ${LIMITS.iou}; inside V + ${LIMITS.alignPx + LIMITS.blockPx} px`;
    if (!this.clip.hasMoving) return row('The box is the video', 'N/A', NO_MOVING, need);
    const k = this.clip.instantAt(CONFIG.framesAtMs.video);
    const segment = this.clip.segments.find((s) => s.first_instant <= k && k <= s.last_instant);
    const rect = segment?.rect ?? this.clip.rectAt(k);
    const V = this.f.pic.V;
    if (!V) return row('The box is the video', 'FAIL', 'the video box was not read from the page', need);
    const iou = Rect.iou(rect, V);
    const inside = Rect.inside(rect, Rect.grow(V, LIMITS.alignPx + LIMITS.blockPx));
    return row('The box is the video', verdict(iou >= LIMITS.iou && inside), `rect ${Rect.text(rect)}, V ${Rect.text(V)}, IoU ${fixed(iou, 3)}, inside ${inside}`, need);
  }

  textExact() {
    const need = '0 intersections with the right column';
    if (!this.clip.hasMoving) return row('Text beside the video is exact', 'N/A', NO_MOVING, need);
    const scroll = this.f.events.scroll?.clip_ms;
    const secondary = this.f.pic.secondary;
    if (!secondary) return row('Text beside the video is exact', 'N/A', 'the page has no right column box', need);
    if (!Number.isFinite(scroll)) return row('Text beside the video is exact', 'FAIL', 'the scroll was not sent', need);
    const hits = this.instantsIn(LIMITS.firstMs, scroll).filter((l) => Rect.intersect(this.clip.rectAt(l.instant), secondary)).length;
    return row('Text beside the video is exact', verdict(hits === 0), `${hits} intersections; right column ${Rect.text(secondary)}`, need);
  }

  scrollStored() {
    const need = `>= 1 exact record in [scroll, scroll + ${LIMITS.scrollRecordMs} ms]`;
    if (!this.clip.hasExact) return row('The scroll is stored', 'N/A', NO_EXACT, need);
    const scroll = this.f.events.scroll?.clip_ms;
    if (!Number.isFinite(scroll)) return row('The scroll is stored', 'FAIL', 'the scroll was not sent', need);
    const n = [...this.f.exact.records.keys()].filter((k) => this.clip.timeOf(k) >= scroll && this.clip.timeOf(k) <= scroll + LIMITS.scrollRecordMs).length;
    return row('The scroll is stored', verdict(n >= 1), `${n} records (scroll at ${fixed(scroll, 0)} ms)`, need);
  }

  boxAfterScroll() {
    const need = `IoU(rect, V') >= ${LIMITS.iou} at scroll + ${LIMITS.scrollBoxMs} ms`;
    if (!this.clip.hasMoving) return row('After the scroll the box is the video again', 'N/A', NO_MOVING, need);
    const scroll = this.f.events.scroll?.clip_ms;
    const V2 = this.f.pic.V2;
    if (!Number.isFinite(scroll) || !V2) return row('After the scroll the box is the video again', 'FAIL', 'scroll or V\' missing', need);
    const rect = this.clip.rectAt(this.clip.instantAt(scroll + LIMITS.scrollBoxMs));
    const iou = Rect.iou(rect, V2);
    return row('After the scroll the box is the video again', verdict(iou >= LIMITS.iou), `rect ${Rect.text(rect)}, V' ${Rect.text(V2)}, IoU ${fixed(iou, 3)}`, need);
  }

  typing() {
    const keys = this.f.events.keys ?? [];
    const need = `${CONFIG.typeText.length} of ${CONFIG.typeText.length} keys stored exactly; moving rect unchanged`;
    if (!this.clip.hasExact) return row('Typing is exact and does not move', 'N/A', NO_EXACT, need);
    if (!this.clip.hasMoving) return row('Typing is exact and does not move', 'N/A', NO_MOVING, need);
    const search = this.f.pic.search;
    if (!search || keys.length !== CONFIG.typeText.length) return row('Typing is exact and does not move', 'FAIL', `search box ${Rect.text(search)}, ${keys.length} keys sent`, need);
    const records = [...this.f.exact.records];
    const stored = keys.filter((key) => records.some(([k, r]) => {
      const t = this.clip.timeOf(k);
      return t >= key.clip_ms && t <= key.clip_ms + LIMITS.keyRecordMs && Rect.rowsMeet(r.bands, search[1], search[3]);
    })).length;
    const before = this.clip.rectAt(this.clip.instantAt(CONFIG.typeAtMs - LIMITS.typeMarginMs));
    const after = this.clip.rectAt(this.clip.instantAt(keys.at(-1).clip_ms + LIMITS.typeMarginMs));
    const same = Rect.equal(before, after);
    return row('Typing is exact and does not move', verdict(stored === keys.length && same),
      `${stored} of ${keys.length} keys; rect ${Rect.text(before)} -> ${Rect.text(after)}`, need);
  }

  stop() {
    const slack = (LIMITS.animatingNeed + LIMITS.holdInstants - 1 + LIMITS.pauseSlackInstants) * this.clip.frameMs;
    const need = `last mv <= pause + ${slack} ms`;
    if (!this.clip.hasMoving) return row('The stop', 'N/A', NO_MOVING, need);
    const pause = this.f.events.pause?.clip_ms;
    const last = this.lastMoving();
    if (!Number.isFinite(pause) || last === null) return row('The stop', 'FAIL', `pause ${pause ?? 'not sent'}, last mv ${last ?? 'none'}`, need);
    const t = this.clip.timeOf(last);
    return row('The stop', verdict(t <= pause + slack), `last mv at ${fixed(t, 0)} ms = pause + ${fixed(t - pause, 0)} ms`, need);
  }

  exactAfterStop() {
    const need = 'the instant after the last mv has a record covering the last rect rows';
    if (!this.clip.hasMoving) return row('One exact picture after the stop', 'N/A', NO_MOVING, need);
    if (!this.clip.hasExact) return row('One exact picture after the stop', 'N/A', NO_EXACT, need);
    const last = this.lastMoving();
    if (last === null) return row('One exact picture after the stop', 'FAIL', 'no moving instant', need);
    const rect = this.clip.rectAt(last);
    const record = this.f.exact.records.get(last + 1);
    const ok = Boolean(rect && record && Rect.rowsCover(record.bands, rect[1], rect[3]));
    return row('One exact picture after the stop', verdict(ok), `instant ${last + 1}: ${record ? `${record.bands.length} bands` : 'no record'}, rect ${Rect.text(rect)}`, need);
  }

  nothingWritten() {
    const need = '0 x on new:false lines outside a rect change';
    if (!this.clip.hasExact) return row('Nothing changes, nothing written', 'N/A', NO_EXACT, need);
    const violations = this.clip.lines.filter((l) => l.new === false && Array.isArray(l.x) && l.instant > 0
      && Rect.equal(this.clip.rectAt(l.instant), this.clip.rectAt(l.instant - 1))).length;
    const end = this.clip.timeOf((this.clip.lines.at(-1)?.instant ?? 0) + 1);
    const lastBytes = this.clip.lines.filter((l) => Array.isArray(l.x) && this.clip.timeOf(l.instant) >= end - LIMITS.lastBytesMs).reduce((s, l) => s + l.x[2], 0);
    return row('Nothing changes, nothing written', verdict(violations === 0), `${violations} violations; exact bytes in the last ${LIMITS.lastBytesMs / 1000} s: ${lastBytes}`, need);
  }

  sampleCount() {
    const need = 'frames = last - first + 1, complete';
    if (!this.clip.hasMoving) return row('Sample count', 'N/A', NO_MOVING, need);
    const bad = this.clip.segments.filter((s) => s.frames !== s.last_instant - s.first_instant + 1 || s.complete !== true);
    return row('Sample count', verdict(bad.length === 0), `${this.clip.segments.length} segments, ${bad.length} wrong${bad.length ? ` (${bad.map((s) => s.index).join(', ')})` : ''}`, need);
  }

  sizeBound() {
    const need = 'B <= B_max';
    if (!this.clip.hasMoving) return row('Size bound', 'N/A', NO_MOVING, need);
    const p = LIMITS.bMax; const mb = LIMITS.mb;
    const area = (s) => Rect.area(s.rect);
    const moving = p.movingMbPerMin * mb * this.clip.segments.reduce((s, x) => s + (area(x) / p.fullArea) * ((x.frames ?? 0) / p.linesPerMin), 0);
    const exact = p.exactMbPerMin * mb * (CONFIG.durationMs / 60000);
    const keys = this.clip.segments.reduce((s, x) => s + p.keyPictures * area(x) * p.bytesPerPixel, 0);
    const bMax = moving + exact + keys + p.metaMb * mb;
    const b = this.clip.bytes.total;
    return row('Size bound', verdict(b <= bMax), `B ${fixed(b / mb)} MB, B_max ${fixed(bMax / mb)} MB`, need);
  }

  pictureIsScreen() {
    const c = this.f.shotCompare;
    const need = 'max difference 0 below the label box';
    if (c.na) return row('The picture is the screen', 'N/A', c.na, need);
    if (c.error) return row('The picture is the screen', 'FAIL', c.error, need);
    return row('The picture is the screen', verdict(c.max_difference === 0),
      `${c.differing} of ${c.pixels} pixels differ, max ${c.max_difference}${c.diff_file ? ` (diff ${c.diff_file})` : ''}`, need);
  }

  viewerSees() {
    const need = 'overview ok; animating over V; change at scroll; change at the search box while typing';
    const ov = this.f.viewer.overview;
    if (!ov?.ok) return row('The viewer sees it', 'FAIL', `clip_overview failed: ${ov?.error ?? 'not called'}`, need);
    const timeline = ov.view.timeline ?? [];
    const V = this.f.pic.V; const search = this.f.pic.search;
    const scroll = this.f.events.scroll?.clip_ms; const keys = this.f.events.keys ?? [];
    const animating = timeline.filter((e) => e.kind === 'animating');
    const changes = timeline.filter((e) => e.kind === 'change');
    const overV = animating.some((e) => Rect.intersect(Rect.fromObject(e.bbox), V));
    const atScroll = Number.isFinite(scroll) && changes.some((e) => e.start_ms >= scroll - LIMITS.scrollChangeLeadMs && e.start_ms <= scroll + LIMITS.scrollRecordMs);
    const typingEnd = keys.length ? keys.at(-1).clip_ms + LIMITS.keyRecordMs : -1;
    const atTyping = keys.length > 0 && changes.some((e) => e.start_ms <= typingEnd && e.end_ms >= keys[0].clip_ms && Rect.intersect(Rect.fromObject(e.bbox), search));
    const value = `${timeline.length} entries (${animating.length} animating, ${changes.length} change); animating over V ${overV}, change at scroll ${atScroll}, change at search ${atTyping}`;
    if (!animating.length && atScroll && atTyping) return row('The viewer sees it', 'N/A', `${value}; no entry of kind animating (not emitted by this build?)`, need);
    return row('The viewer sees it', verdict(overV && atScroll && atTyping), value, need);
  }

  videoInClip() {
    const need = 'frames at 5 s and 20 s differ inside V; V not flat in either';
    const v = this.f.videoFrames;
    if (v.error) return row('The video is in the clip', 'FAIL', v.error, need);
    return row('The video is in the clip', verdict(v.differing > 0 && !v.flat_a && !v.flat_b),
      `${v.differing} pixels differ inside V; flat ${v.flat_a} / ${v.flat_b}`, need);
  }

  readyTime() {
    const need = `<= ${LIMITS.readyMs} ms`;
    if (!this.f.ready?.wall) return row('Ready time', 'FAIL', 'never ready', need);
    const ms = this.f.ready.wall - this.f.s0 - CONFIG.durationMs;
    return row('Ready time', verdict(this.f.ready.state === 'ready' && ms <= LIMITS.readyMs), `${ms} ms (${this.f.ready.state})`, need);
  }

  windowClosed() {
    return row('The window is closed', verdict(this.f.closed), this.f.closed ? 'no process of the profile left' : this.f.closeError, '0 processes');
  }

  frameTiming() {
    const need = 'instants 0..n-1, t_ms = k * 1000 / f, mv frame = instant - first_instant < frames';
    const errors = [];
    this.clip.lines.forEach((l, i) => {
      if (l.instant !== i) errors.push(`line ${i}: instant ${l.instant}`);
      if (l.t_ms !== Math.round((l.instant * 1000) / this.clip.fps)) errors.push(`instant ${l.instant}: t_ms ${l.t_ms}`);
      if (Array.isArray(l.mv)) {
        const s = this.clip.segments.find((x) => x.index === l.mv[0]);
        if (s && (l.mv[1] !== l.instant - s.first_instant || l.mv[1] >= s.frames)) errors.push(`instant ${l.instant}: mv ${JSON.stringify(l.mv)}`);
        if (!s && this.clip.hasMoving) errors.push(`instant ${l.instant}: segment ${l.mv[0]} not in clip.json`);
      }
    });
    return row('Frame timing (no mapped/missing errors)', verdict(errors.length === 0 && this.clip.lines.length > 0),
      `${errors.length} errors${errors.length ? `: ${errors.slice(0, 3).join('; ')}` : ''}`, need);
  }

  movingSsim() {
    const s = this.f.ssim;
    const need = `>= ${LIMITS.ssimMin} (plan 4.2 W8)`;
    if (s.error) return row('SSIM of the moving area', 'FAIL', s.error, need);
    return row('SSIM of the moving area', verdict(s.best >= LIMITS.ssimMin), `best ${fixed(s.best, 4)} of ${s.values.map((v) => fixed(v, 4)).join(', ')}`, need);
  }

  exactIsScreen() {
    const c = this.f.exactCompare;
    const need = 'decoded exact layer = DevTools screenshot over the viewport, outside the moving box';
    if (c.na) return row('Exact layer is the screen', 'N/A', c.na, need);
    return row('Exact layer is the screen', verdict(c.max_difference === 0), `${c.differing} of ${c.pixels} pixels differ, max ${c.max_difference} (instant ${c.instant})`, need);
  }
}

/** One YouTube proof run: record (steps 1 to 13), view (step 14), judge and report. */
class YoutubeProof {
  constructor(exe, scratch, url) {
    this.exe = exe; this.scratch = scratch; this.url = url;
    const stamp = Date.now();
    this.folder = path.join(scratch, 'clips', `youtube-${stamp}`);
    this.profile = path.join(scratch, 'chrome', `youtube-${stamp}`);
    this.run = { config: CONFIG, limits: LIMITS, url, started_at: new Date().toISOString(), folder: this.folder,
      boxes: {}, events: {}, process: { samples: [] }, viewer: {}, errors: [] };
    this.closed = true; this.closeError = null; this.rec = null; this.ready = null;
  }

  async main() {
    fs.mkdirSync(path.dirname(this.folder), { recursive: true });
    const recorder = new Recorder(this.exe, this.scratch);
    try {
      await recorder.start();
      if (!(await this.record(recorder))) {
        this.writeReport(null);
        return 1;
      }
      return await this.judge(recorder);
    } finally {
      await recorder.close();
    }
  }

  /** Steps 2 to 13. False when no video played (nothing recorded, nothing judged). */
  async record(recorder) {
    const browser = CONFIG.browserCandidates.find((p) => p && fs.existsSync(p));
    if (!browser) throw new Error('Chrome or Edge not found: set CHROME_PATH');
    const rect = YoutubeProof.windowRect();
    this.run.browser = browser; this.run.window = rect;
    console.log(`browser ${path.basename(browser)}, window ${rect.width} x ${rect.height}${rect.screen ? ` on a ${rect.screen.w} x ${rect.screen.h} screen` : ''}`);
    let win = null; let probe = null; let recorded = false;
    try {
      win = await TestWindow.open({ browser, url: this.url, profileDir: this.profile, rect });
      await win.placeBehind();
      if (!(await this.makeReady(win))) {
        console.error('no playing video: stopped before recording, no clip judged');
        this.run.errors.push('no playing video');
        return false;
      }
      User32.sendToBack(win.hwnd);
      this.run.boxes.before = await win.read(READ_BOXES);
      const m0 = ProcessProbe.read(recorder.pid);
      this.run.process.m0 = m0; this.run.process.cpu0 = m0;
      probe = new ProcessProbe(recorder.pid).start();
      this.rec = await recorder.record(win.hwnd, CONFIG.durationMs, this.folder);
      this.folder = this.rec.folder; this.run.folder = this.folder; this.run.s0 = this.rec.s0;
      console.log(`recording ${this.rec.clip_id} into ${this.folder}`);
      try {
        await this.drive(win, this.rec.s0);
      } catch (error) {
        this.run.errors.push(`page events: ${error.message}`);
        console.error(`page events failed: ${error.message}`);
      }
      const stopped = await recorder.waitStopped(this.rec.clip_id, this.rec.s0 + CONFIG.durationMs);
      this.run.stopped = stopped;
      if (stopped.state === 'ready' || stopped.state === 'failed') this.ready = stopped;
      recorded = true;
    } finally {
      if (win) {
        try {
          await win.close();
        } catch (error) {
          this.closed = false; this.closeError = error.message;
          this.run.errors.push(error.message);
          console.error(`CleanupError: ${error.message}`);
        }
      }
      if (!recorded) probe?.stop();
    }
    try {
      this.ready ??= await recorder.waitReady(this.rec.clip_id);
      this.run.process.cpu1 = ProcessProbe.read(recorder.pid);
    } catch (error) {
      this.run.errors.push(error.message);
    } finally {
      probe?.stop();
      this.run.process.samples = probe?.samples ?? [];
    }
    this.run.ready = this.ready;
    console.log(`clip ${this.ready?.state ?? 'not ready'}; window ${this.closed ? 'closed' : 'NOT closed'}`);
    return true;
  }

  /** 1920 x 1080 at (0, 0), or the largest window that fits the primary screen. */
  static windowRect() {
    const { x, y, width, height } = CONFIG.window;
    try {
      const [w, h] = powershell("Add-Type -AssemblyName System.Windows.Forms; $b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds; '{0} {1}' -f $b.Width, $b.Height").split(/\s+/).map(Number);
      if (w > 0 && h > 0) return { x, y, width: Math.min(width, w - x), height: Math.min(height, h - y), screen: { w, h } };
    } catch { /* the wanted size */ }
    return { x, y, width, height, screen: null };
  }

  /** Step 3: rejects a consent page, then waits for a playing video (play() called once). */
  async makeReady(win) {
    const end = Date.now() + CONFIG.playWaitMs;
    let played = false;
    while (Date.now() < end) {
      try {
        const consent = await win.read(CONSENT);
        if (consent === 'clicked') {
          this.run.events.consent = 'rejected';
          await sleep(CONFIG.consentSettleMs);
          continue;
        }
        const a = await win.read(VIDEO_STATE);
        if (a?.paused && !played) { played = true; await win.read(VIDEO_PLAY); }
        await sleep(CONFIG.playProbeMs);
        const b = await win.read(VIDEO_STATE);
        if (a && b && b.ready >= 3 && !b.paused && b.t > a.t) {
          this.run.events.playing = b;
          return true;
        }
      } catch {
        await sleep(CONFIG.playProbeMs); // the page is navigating
      }
    }
    return false;
  }

  /** Steps 8 to 11, on the clip clock: screenshot while playing, scroll, typing, pause, screenshot. */
  async drive(win, s0) {
    const ev = this.run.events;
    const at = (ms) => sleep(Math.max(0, s0 + ms - Date.now()));
    const shot = async (name) => {
      const s = await win.screenshot(path.join(this.scratch, name));
      return { ...s, clip_ms: (s.before + s.after) / 2 - s0 };
    };
    await at(CONFIG.playShotAtMs);
    ev.play_shot = await shot('youtube-play-shot.png');
    await at(CONFIG.scrollAtMs);
    const [w, h] = this.run.boxes.before.inner;
    const scrollWall = Date.now();
    await win.cdp.send('Input.dispatchMouseEvent', { type: 'mouseWheel', x: w / 2, y: h / 2, deltaX: 0, deltaY: CONFIG.scrollDeltaY });
    ev.scroll = { wall: scrollWall, clip_ms: scrollWall - s0 };
    await at(CONFIG.scrollAtMs + CONFIG.scrollSettleMs);
    this.run.boxes.after_scroll = await win.read(READ_BOXES);
    await at(CONFIG.typeAtMs);
    ev.focused = await win.read(SEARCH_FOCUS);
    this.run.boxes.typing = await win.read(READ_BOXES);
    ev.keys = [];
    for (const char of CONFIG.typeText) {
      const key = char === ' ' ? { key: ' ', code: 'Space' } : { key: char };
      const wall = Date.now();
      await win.cdp.send('Input.dispatchKeyEvent', { type: 'keyDown', text: char, unmodifiedText: char, ...key });
      await win.cdp.send('Input.dispatchKeyEvent', { type: 'keyUp', ...key });
      ev.keys.push({ char, wall, clip_ms: wall - s0 });
      await sleep(CONFIG.typeGapMs);
    }
    ev.typed_value = await win.read(SEARCH_VALUE);
    await win.read(BLUR); // a blinking caret would keep the page changing
    await at(CONFIG.pauseAtMs);
    const pauseWall = Date.now();
    ev.pause = { wall: pauseWall, clip_ms: pauseWall - s0, paused: await win.read(VIDEO_PAUSE) };
    await at(CONFIG.shotAtMs);
    this.run.boxes.shot = await win.read(READ_BOXES);
    ev.shot = await shot('youtube-devtools-shot.png');
  }

  /** Step 14 and the judging; returns the exit code. */
  async judge(recorder) {
    const clip = new ClipData(this.folder);
    if (!clip.lines.length) this.run.errors.push(`no frames.jsonl lines in ${this.folder}`);
    if (clip.meta.schema_version !== 3) this.run.errors.push(`clip.json schema_version ${clip.meta.schema_version} (3 expected)`);
    const f = await this.facts(recorder, clip);
    const judge = new Judge(f);
    const scenarios = judge.scenarios();
    const extras = judge.extras();
    const summary = this.summary(clip);
    this.print(scenarios, extras, summary);
    const passed = scenarios.every((r) => r.status === 'PASS');
    this.run.result = { passed, scenarios, extras, summary };
    this.writeReport(this.folder);
    return passed && this.closed ? 0 : 1;
  }

  /** Page boxes in picture px, viewer outputs and the pixel checks. */
  async facts(recorder, clip) {
    const ev = this.run.events; const boxes = this.run.boxes;
    const geometryOk = Number.isFinite(clip.geometry.window_width) && Number.isFinite(clip.geometry.window_height);
    const mapOf = (page) => (page && geometryOk ? new PageMap(clip.geometry, page) : null);
    const before = mapOf(boxes.before); const after = mapOf(boxes.after_scroll);
    const typing = mapOf(boxes.typing) ?? before; const atShot = mapOf(boxes.shot);
    const pic = {
      viewport: before?.viewport ?? null,
      V: before?.rect(boxes.before?.video) ?? null,
      secondary: before?.rect(boxes.before?.secondary) ?? null,
      V2: after?.rect(boxes.after_scroll?.video) ?? null,
      search: typing?.rect(boxes.typing?.search ?? boxes.before?.search) ?? null,
      above: atShot?.rect(boxes.shot?.above) ?? null,
    };
    this.run.picture_boxes = pic;
    const shotInstant = Number.isFinite(ev.shot?.clip_ms) ? clip.instantAt(ev.shot.clip_ms) : -1;
    const exact = clip.hasExact ? await clip.decodeExact(shotInstant) : { records: new Map(), picture: null, errors: [] };
    if (exact.errors.length) this.run.errors.push(...exact.errors.map((e) => `exact layer: ${e}`));
    this.run.exact_records = { count: exact.records.size, key: [...exact.records.values()].filter((r) => r.key).length,
      bands_mean: exact.records.size ? [...exact.records.values()].reduce((s, r) => s + r.bands.length, 0) / exact.records.size : 0 };
    const viewer = await this.view(recorder, clip, pic, atShot);
    return {
      clip, pic, exact, viewer, s0: this.run.s0, events: ev, ready: this.ready, closed: this.closed, closeError: this.closeError,
      process: { m0: this.run.process.m0, cpu0: this.run.process.cpu0, cpu1: this.run.process.cpu1, samples: this.run.process.samples },
      shotCompare: this.shotCompare(viewer.shot_crop, pic, atShot, clip),
      videoFrames: this.videoFrames(viewer.frames, pic.V),
      ssim: this.ssim(viewer.ssim, pic, before),
      exactCompare: this.exactCompare(exact, clip, atShot, shotInstant),
    };
  }

  /** Step 14: every viewer call, through the same server, after the window is closed. */
  async view(recorder, clip, pic, atShot) {
    const v = this.run.viewer; const ev = this.run.events;
    const copy = (out, name) => {
      const image = out.view?.images?.[0];
      if (!out.ok || !image?.path || !fs.existsSync(image.path)) return null;
      const file = path.join(this.scratch, name);
      fs.copyFileSync(image.path, file);
      return file;
    };
    const keep = (out) => ({ ok: out.ok, error: out.error, text: out.text, images: out.view?.images ?? [], regions: out.view?.regions ?? [] });
    const overview = await recorder.view(this.folder, { operation: 'clip_overview' });
    v.overview = { ...keep(overview), timeline: overview.view?.timeline ?? [] };
    const frames = {};
    for (const [name, t] of Object.entries(CONFIG.framesAtMs)) {
      const out = await recorder.view(this.folder, { operation: 'clip_frame', t_ms: t });
      frames[name] = { ...keep(out), t_ms: t, file: copy(out, `youtube-frame-${name}-${t}ms.png`) };
    }
    v.frames = frames;
    const [start, end] = CONFIG.diffMs;
    const diff = await recorder.view(this.folder, { operation: 'clip_diff', start_ms: start, end_ms: end });
    v.diff = { ...keep(diff), file: copy(diff, `youtube-diff-${start}-${end}ms.png`) };
    v.shot_crop = null;
    const region = atShot?.crop(pic.above);
    if (region && Number.isFinite(ev.shot?.clip_ms)) {
      const t = Math.round(ev.shot.clip_ms);
      const out = await recorder.view(this.folder, { operation: 'clip_frame', t_ms: t, region });
      v.shot_crop = { ...keep(out), t_ms: t, region, file: copy(out, `youtube-shot-crop-${t}ms.png`) };
    }
    v.ssim = [];
    const vRegion = pic.V && pic.viewport ? { x: pic.V[0], y: pic.V[1], w: pic.V[2], h: pic.V[3] } : null;
    if (vRegion && Math.min(vRegion.w, vRegion.h) >= LIMITS.minCropSide && Number.isFinite(ev.play_shot?.clip_ms)) {
      const mid = Math.round(ev.play_shot.clip_ms);
      for (const t of [mid - clip.frameMs, mid, mid + clip.frameMs].map(Math.round).filter((x) => x >= 0)) {
        const out = await recorder.view(this.folder, { operation: 'clip_frame', t_ms: t, region: vRegion });
        v.ssim.push({ ...keep(out), t_ms: t, region: vRegion, file: copy(out, `youtube-video-crop-${t}ms.png`) });
      }
    }
    return { overview: { ...v.overview, view: overview.view }, frames, diff: v.diff, shot_crop: v.shot_crop, ssim: v.ssim };
  }

  /** Scenario 18: clip_frame crop of the title block at the screenshot time = the DevTools crop. */
  shotCompare(out, pic, atShot, clip) {
    if (clip.geometry.scale !== 1) return { na: `geometry.scale ${clip.geometry.scale}: pixels are not 1:1` };
    if (!pic.above || !atShot) return { na: 'the title block box was not read at the screenshot time' };
    if (!out?.ok || !out.file) return { error: `clip_frame crop failed: ${out?.error ?? 'not called'}` };
    try {
      const crop = Png.read(out.file);
      const shot = Png.read(this.run.events.shot.file);
      const label = PixelCheck.labelRows(crop);
      const [vx, vy] = atShot.viewport;
      const sx = out.region.x - vx; const sy = out.region.y - vy;
      const w = Math.min(crop.width, shot.width - sx); const h = Math.min(crop.height, shot.height - sy) - label;
      if (sx < 0 || sy < 0 || w <= 0 || h <= 0) return { error: `crop ${JSON.stringify(out.region)} lies outside the ${shot.width} x ${shot.height} screenshot` };
      const c = PixelCheck.compare(crop, 0, label, shot, sx, sy + label, w, h);
      let diffFile = null;
      if (c.differing) {
        diffFile = path.join(this.scratch, 'youtube-shot-crop-diff.png');
        fs.writeFileSync(diffFile, Png.encodeRgb(c.width, c.height, c.diff));
      }
      return { pixels: c.pixels, differing: c.differing, max_difference: c.max_difference, diff_file: diffFile };
    } catch (error) {
      return { error: `compare failed: ${error.message}` };
    }
  }

  /** Scenario 20: the frames at 5 s and 20 s inside V. */
  videoFrames(frames, V) {
    const a = frames.video; const b = frames.after_scroll;
    if (!V) return { error: 'the video box was not read from the page' };
    if (!a?.file || !b?.file) return { error: `clip_frame failed: ${a?.error ?? b?.error ?? 'no image'}` };
    try {
      const pa = Png.read(a.file); const pb = Png.read(b.file);
      const r = Rect.intersect(V, [0, 0, Math.min(pa.width, pb.width), Math.min(pa.height, pb.height)]);
      if (!r) return { error: 'V lies outside the frames' };
      const c = PixelCheck.compare(pa, r[0], r[1], pb, r[0], r[1], r[2], r[3]);
      return { differing: c.differing, flat_a: pa.flat(r), flat_b: pb.flat(r) };
    } catch (error) {
      return { error: `compare failed: ${error.message}` };
    }
  }

  /** SSIM of V between the clip (three instants around the screenshot) and the playing screenshot. */
  ssim(crops, pic, before) {
    if (!crops.length) return { error: 'no clip_frame crop of V (V missing or under the minimum crop)' };
    const shotFile = this.run.events.play_shot?.file;
    if (!shotFile || !before) return { error: 'no screenshot while playing' };
    try {
      const shot = Png.read(shotFile);
      const values = [];
      for (const c of crops.filter((x) => x.file)) {
        const crop = Png.read(c.file);
        const label = PixelCheck.labelRows(crop);
        const sx = c.region.x - before.viewport[0]; const sy = c.region.y - before.viewport[1];
        const w = Math.min(crop.width, shot.width - sx); const h = Math.min(crop.height, shot.height - sy) - label;
        if (sx >= 0 && sy >= 0 && w > 0 && h > 0) values.push(PixelCheck.ssim(crop, 0, label, shot, sx, sy + label, w, h));
      }
      if (!values.length) return { error: `clip_frame crops failed: ${crops.find((x) => x.error)?.error ?? 'outside the screenshot'}` };
      return { values, best: Math.max(...values) };
    } catch (error) {
      return { error: `SSIM failed: ${error.message}` };
    }
  }

  /** Decoded exact layer E at the screenshot instant against the DevTools screenshot. */
  exactCompare(exact, clip, atShot, shotInstant) {
    if (!clip.hasExact) return { na: NO_EXACT };
    if (!exact.picture || !atShot) return { na: 'no exact picture or page box at the screenshot instant' };
    if (clip.geometry.scale !== 1) return { na: `geometry.scale ${clip.geometry.scale}: pixels are not 1:1` };
    try {
      const shot = Png.read(this.run.events.shot.file);
      const [vx, vy, vw, vh] = atShot.viewport;
      const e = exact.picture.image;
      const w = Math.min(vw, shot.width, e.width - vx); const h = Math.min(vh, shot.height, e.height - vy);
      if (vx < 0 || vy < 0 || w <= 0 || h <= 0) return { na: 'the viewport lies outside the picture' };
      const c = PixelCheck.compare(e, vx, vy, shot, 0, 0, w, h, clip.rectAt(shotInstant));
      return { instant: exact.picture.instant, pixels: c.pixels, differing: c.differing, max_difference: c.max_difference };
    } catch (error) {
      return { na: `compare failed: ${error.message}` };
    }
  }

  /** Size, layers, moving areas, CPU and RAM, for the owner. */
  summary(clip) {
    const mb = LIMITS.mb; const b = clip.bytes;
    const minutes = (clip.meta.recorded_ms ?? CONFIG.durationMs) / 60000;
    const media = b.exact + b.moving;
    const counts = { moving: 0, exact: 0, neither: 0 };
    for (const l of clip.lines) {
      if (Array.isArray(l.mv)) counts.moving++;
      if (Array.isArray(l.x)) counts.exact++;
      if (!Array.isArray(l.mv) && !Array.isArray(l.x)) counts.neither++;
    }
    const areas = clip.segments.map((s) => ({ index: s.index, rect: s.rect, from_ms: clip.timeOf(s.first_instant), to_ms: clip.timeOf(s.last_instant + 1),
      frames: s.frames, bytes: s.bytes, complete: s.complete }));
    const { m0, cpu0, cpu1, samples } = this.run.process;
    const peak = samples.length ? Math.max(...samples.map((s) => s.private_bytes)) : null;
    return {
      bytes: b,
      total_mb: b.total / mb,
      mb_per_min: { exact: b.exact / mb / minutes, moving: b.moving / mb / minutes, total: b.total / mb / minutes },
      share_percent: { h264: media ? (100 * b.moving) / media : null, zstd: media ? (100 * b.exact) / media : null },
      instants: counts,
      moving_areas: areas,
      ram_peak_mb: peak !== null ? peak / mb : null,
      ram_over_m0_mb: peak !== null && m0 ? (peak - m0.private_bytes) / mb : null,
      cpu_percent: cpu0 && cpu1 ? (100 * (cpu1.cpu_ms - cpu0.cpu_ms)) / (minutes * 60000 * os.cpus().length) : null,
      ready_ms: this.ready?.wall && this.run.s0 ? this.ready.wall - this.run.s0 - CONFIG.durationMs : null,
      exact_records: this.run.exact_records,
      schema_version: clip.meta.schema_version ?? null,
      way: clip.meta.way ?? null,
    };
  }

  print(scenarios, extras, s) {
    console.log(`\nclip ${this.folder}`);
    console.log(`size ${fixed(s.total_mb)} MB: zstd ${fixed(s.bytes.exact / LIMITS.mb)} MB (${fixed(s.share_percent.zstd, 1)} %), H.264 ${fixed(s.bytes.moving / LIMITS.mb)} MB (${fixed(s.share_percent.h264, 1)} %); ${fixed(s.mb_per_min.total)} MB/min`);
    console.log(`instants: ${s.instants.moving} with mv, ${s.instants.exact} with x, ${s.instants.neither} with neither`);
    for (const a of s.moving_areas) console.log(`moving area ${a.index}: ${Rect.text(a.rect)} from ${fixed(a.from_ms, 0)} to ${fixed(a.to_ms, 0)} ms, ${a.frames} frames, ${a.bytes} bytes${a.complete === false ? ', incomplete' : ''}`);
    if (!s.moving_areas.length) console.log('moving areas: none (or not available)');
    console.log(`recorder CPU ${fixed(s.cpu_percent)} %, peak RAM ${fixed(s.ram_peak_mb, 1)} MB (+${fixed(s.ram_over_m0_mb, 1)} MB), ready after ${s.ready_ms ?? 'n/a'} ms`);
    const all = [...scenarios, ...extras];
    const width = Math.max(...all.map((r) => r.name.length));
    console.log('');
    for (const r of all) console.log(`${r.id.padStart(3)}  ${r.status.padEnd(7)} ${r.name.padEnd(width)}  ${r.value}  [${r.need}]`);
    const count = (status) => scenarios.filter((r) => r.status === status).length;
    console.log(`\n${count('PASS')} of ${scenarios.length} scenarios pass; ${count('FAIL')} fail, ${count('N/A')} not available, ${count('BLOCKED')} blocked`);
    if (this.run.errors.length) console.log(`notes:\n- ${this.run.errors.join('\n- ')}`);
  }

  /** The run report next to the clip folder, and as <scratch>/youtube-run.json. */
  writeReport(folder) {
    this.run.finished_at = new Date().toISOString();
    this.run.window_closed = this.closed;
    const files = [path.join(this.scratch, 'youtube-run.json')];
    if (folder) files.push(path.join(path.dirname(folder), `${path.basename(folder)}.youtube-run.json`));
    for (const file of files) writeJson(file, this.run);
    console.log(`report ${files.join(' and ')}`);
  }
}

const [exe, scratch, ...rest] = process.argv.slice(2);
if (!exe || !scratch) {
  console.error('usage: node youtube-proof.mjs <synapse-mcp exe> <scratch dir> [--url <watch url> | <watch url>]');
  process.exit(2);
}
if (typeof WebSocket !== 'function') {
  console.error('youtube-proof needs Node 22 or later (global WebSocket)');
  process.exit(2);
}
const urlAt = rest.indexOf('--url');
const url = urlAt >= 0 ? rest[urlAt + 1] : rest.find((a) => /^https?:\/\//.test(a)) ?? CONFIG.url;
if (!url || !/^https?:\/\//.test(url)) {
  console.error(`not a watch url: ${url}`);
  process.exit(2);
}
fs.mkdirSync(scratch, { recursive: true });
try {
  process.exit(await new YoutubeProof(path.resolve(exe), path.resolve(scratch), url).main());
} catch (error) {
  console.error(`youtube-proof stopped: ${error?.stack ?? error}`);
  process.exit(1);
}
