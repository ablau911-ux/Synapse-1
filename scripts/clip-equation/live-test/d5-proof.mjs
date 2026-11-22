// Clip viewer D5 live proof (docs/clip-recorder-plan.md, "D5 as an equation" item 7, and "Proof").
// Records the six clips that ../spec/d5.mjs checks, writes its two inputs and runs the check:
//
//   node d5-proof.mjs <synapse-mcp exe> <scratch dir> [--clips name,name]
//
// --clips records only the named clips again and keeps the others from the last run (for example
// a boundary run that d5 rejects as ambiguous_onset). Needs Node 22 or later (global WebSocket)
// and Chrome (CHROME_PATH or its default install folders). No npm packages.
//
// The build runs as a separate stdio MCP instance with its own --db and SYNAPSE_SHELL_JOB_ROOT in
// the scratch dir. Each clip gets its own Chrome --app window and profile: launched minimized
// without activation, shown without activation, given two animation frames to paint, put behind
// all other windows, recorded, read over the DevTools protocol, and closed in the same step
// (owner rule: a test window is open only for the seconds of its recording). Then clip_overview
// runs and its views/overview_analysis.json gives timeline and picks.
//
// Writes into the scratch dir: viewer-output.json and schedule.json (the d5.mjs header, exactly),
// d5-runs.json (per clip: folder, clip.json, frames.jsonl, overview_analysis.json, s_0, what the
// page gave, or the error), clips/, chrome/ (profiles), db/, shell-jobs/, stderr-mcp.log.
import { execFileSync, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { McpServer, User32, clipStatus, sleep } from './mcp-stdio.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const PAGES = path.resolve(HERE, '..');
const RUN_SPEC = path.join(PAGES, 'spec', 'run-spec.mjs');

// Tunables of the runs; times in ms.
const CONFIG = {
  chromeCandidates: [
    process.env.CHROME_PATH,
    path.join(process.env.ProgramFiles ?? '', 'Google', 'Chrome', 'Application', 'chrome.exe'),
    path.join(process.env['ProgramFiles(x86)'] ?? '', 'Google', 'Chrome', 'Application', 'chrome.exe'),
    path.join(process.env.LOCALAPPDATA ?? '', 'Google', 'Chrome', 'Application', 'chrome.exe'),
  ],
  // Proof section: the covered test window keeps rendering and its timers run at full rate.
  chromeFlags: ['--disable-backgrounding-occluded-windows', '--disable-renderer-backgrounding',
    '--disable-background-timer-throttling', '--no-first-run', '--no-default-browser-check'],
  // Chrome --window-position / --window-size; ui-events-page.html needs a 1180 x 744 CSS px viewport.
  window: { x: 40, y: 40, width: 1240, height: 840 },
  leadMs: 8000,        // schedule start after the launch: time to load, place and start record
  tailMs: 1500,        // the proof clip runs on after the page's last row
  sceneMs: 20000,      // still, spinner and flash clips
  boundaryMs: 35000,   // boundary runs: the page is reloaded with a start set from the clip's s_0
  parkedMs: 3600000,   // a boundary page's start until that reload: nothing plays before it
  reloadMs: 2000,      // the reloaded boundary page should have at least this long before its start
  onsetPhaseMs: 8,     // the boundary change is made this far into a recorder instant, so that
                       // its set_ms and shown_ms (one display frame later) fall to one instant
  paintMs: 500,        // after two animation frames, before the window is covered
  waitMs: 15000,       // Chrome, DevTools, window lookups, end of recording
  readyMs: 300000,     // clip worker until the clip is ready
  pollMs: 1000,
};
// clip_viewer.rs SETTLE_MS (D5 item 2): the boundary runs end SETTLE_MS - frame_ms and SETTLE_MS on.
const SETTLE_MS = 200;
const BOUNDARY_ROW = 'boundary';
const SW_SHOWMINNOACTIVE = 7;
const UI_PAGE = 'ui-events-page.html';
const SCENES = 'd5-scenes.html';
const FRAMES = 'new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(() => r(true))))';
const READ_UI_PAGE = `({ start: window.scheduleStart, schedule: window.schedule, actual: window.actualTiming,
  always: window.always, box_fits: window.scheduleBoxFits, inner: [innerWidth, innerHeight],
  dpr: devicePixelRatio, page_pixel_ratio: window.pagePixelRatio })`;
const READ_SCENE = '({ start: window.scheduleStart, flashes: window.flashTiming })';

/**
 * The clips of the d5.mjs header. run: proof (schedule from the launch), scene (fixed length) or
 * boundary (start set from s_0). entry: the clip's fields in viewer-output.json, from c = { base
 * (timeline, fps, frame_ms), analysis, files (ClipFiles), page (what the page gave), startMs (the
 * page's start on the clip timeline) }.
 */
const CLIPS = {
  proof: {
    page: UI_PAGE, run: 'proof', read: READ_UI_PAGE,
    entry: (c) => ({ ...c.base, picks: c.analysis.picks, end_ms: c.files.endMs, keyframes_ms: c.files.keyframesMs,
      schedule_start_ms: c.startMs, viewport: viewport(c.files.geometry, c.page) }),
  },
  still: {
    page: SCENES, scene: 'still', run: 'scene', read: READ_SCENE,
    entry: (c) => ({ ...c.base, end_ms: c.files.endMs, keyframes_ms: c.files.keyframesMs }),
  },
  spinner: { page: SCENES, scene: 'spinner', run: 'scene', read: READ_SCENE, entry: (c) => c.base },
  flash: {
    page: SCENES, scene: 'flash', run: 'scene', read: READ_SCENE,
    entry: (c) => ({ ...c.base, shown_ms: flashesInClip(c) }),
  },
  boundary_below: { page: UI_PAGE, run: 'boundary', read: READ_UI_PAGE, quietMs: (frameMs) => SETTLE_MS - frameMs, entry: boundaryEntry },
  boundary_at: { page: UI_PAGE, run: 'boundary', read: READ_UI_PAGE, quietMs: () => SETTLE_MS, entry: boundaryEntry },
};

function boundaryEntry(c) {
  return { ...c.base, folder: c.files.folder, page_start: c.page.start, viewport: viewport(c.files.geometry, c.page), actual: c.page.actual };
}

/**
 * The page's CSS px on the recorded frame, x_rec = x + x_css * scale (d5.mjs header). The captured
 * frame is the whole window: the caption above the page and equal borders left, right and below,
 * so the side border is half the width the page leaves; geometry.scale is the recorder's pixel
 * duplication.
 */
function viewport(geometry, page) {
  const [width, height] = page.inner;
  const side = (geometry.window_width - width * page.dpr) / 2;
  const top = geometry.window_height - height * page.dpr - side;
  return { x: side * geometry.scale, y: top * geometry.scale, scale: page.dpr * geometry.scale };
}

/** Clip times of the flashes shown and hidden, with SETTLE_MS after, inside the clip. */
function flashesInClip(c) {
  return (c.page.flashes ?? [])
    .filter((f) => Number.isFinite(f.set_ms) && Number.isFinite(f.hidden_ms)
      && c.startMs + f.set_ms >= 0 && c.startMs + f.hidden_ms + SETTLE_MS <= c.files.endMs)
    .map((f) => c.startMs + f.shown_ms);
}

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

const slash = (p) => p.replace(/\\/g, '/');
const readJson = (file) => (fs.existsSync(file) ? JSON.parse(fs.readFileSync(file, 'utf8')) : null);
const writeJson = (file, value) => fs.writeFileSync(file, JSON.stringify(value, null, 1));

/** One DevTools protocol connection (Node's global WebSocket). */
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

/** One Chrome --app window with its own profile, driven over DevTools. */
class TestWindow {
  /** Launches the window minimized without activating it and connects to its page. */
  static async open(url, profileDir) {
    const chrome = CONFIG.chromeCandidates.find((p) => p && fs.existsSync(p));
    if (!chrome) throw new Error('Chrome not found: set CHROME_PATH');
    fs.mkdirSync(profileDir, { recursive: true });
    const { x, y, width, height } = CONFIG.window;
    const args = [`--app=${url}`, `--user-data-dir=${profileDir}`, '--remote-debugging-port=0',
      `--window-position=${x},${y}`, `--window-size=${width},${height}`, ...CONFIG.chromeFlags];
    const line = [chrome, ...args].map((a) => `"${a}"`).join(' ').replace(/'/g, "''");
    execFileSync('powershell', ['-NoProfile', '-Command', `(New-Object -ComObject WScript.Shell).Run('${line}', ${SW_SHOWMINNOACTIVE}, $false) | Out-Null`]);
    const win = new TestWindow(profileDir);
    try {
      await win.connect();
    } catch (error) {
      win.close();
      throw error;
    }
    return win;
  }

  /** The browser process of `profileDir` and its main window handle (0 until it has one). */
  static browserProcess(profileDir) {
    const out = execFileSync('powershell', ['-NoProfile', '-Command',
      `Get-CimInstance Win32_Process -Filter "Name = 'chrome.exe'" | Where-Object { $_.CommandLine -and $_.CommandLine.Contains('${profileDir.replace(/'/g, "''")}') -and -not $_.CommandLine.Contains('--type=') } | Select-Object -First 1 | ForEach-Object { '{0} {1}' -f $_.ProcessId, (Get-Process -Id $_.ProcessId).MainWindowHandle }`],
    { encoding: 'utf8' }).trim();
    const [pid, hwnd] = out.split(/\s+/).map(Number);
    return pid ? { pid, hwnd: hwnd || 0 } : null;
  }

  constructor(profileDir) {
    this.profileDir = profileDir; this.cdp = null; this.pid = null; this.hwnd = null;
  }

  async connect() {
    const portFile = path.join(this.profileDir, 'DevToolsActivePort');
    const port = await until(() => fs.existsSync(portFile) && Number(fs.readFileSync(portFile, 'utf8').split(/\r?\n/)[0]), 'DevToolsActivePort');
    const target = await until(async () => {
      const targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
      return targets.find((t) => t.type === 'page' && t.url.startsWith('file:'));
    }, 'the page target');
    this.cdp = await Cdp.connect(target.webSocketDebuggerUrl);
    const browser = await until(() => {
      const found = TestWindow.browserProcess(this.profileDir);
      if (found) this.pid = found.pid;
      return found?.hwnd ? found : null;
    }, 'the Chrome window');
    this.hwnd = browser.hwnd;
  }

  read(expression) { return this.cdp.evaluate(expression); }

  /** Waits until the page has loaded and `probe` (a page expression) is true. */
  ready(probe) {
    return until(() => this.read(`document.readyState === 'complete' && Boolean(${probe})`), `the page (${probe})`);
  }

  /** Shows the window without activating it, lets it paint, then puts it behind all others. */
  async placeBehind() {
    User32.restoreWithoutActivating(this.hwnd);
    await this.read(FRAMES);
    await sleep(CONFIG.paintMs);
    User32.sendToBack(this.hwnd);
  }

  navigate(url) { return this.cdp.send('Page.navigate', { url }); }

  close() {
    this.cdp?.close();
    const pid = this.pid ?? TestWindow.browserProcess(this.profileDir)?.pid;
    if (pid) try { execFileSync('taskkill', ['/PID', String(pid), '/T', '/F'], { stdio: 'ignore' }); } catch { }
  }
}

/** What the driver reads from a clip folder: clip.json and frames.jsonl. */
class ClipFiles {
  static s0(folder) {
    return readJson(path.join(folder, 'clip.json'))?.clock?.wall_clock_unix_ms_at_s0 || null;
  }

  constructor(folder) {
    this.folder = folder;
    this.clipJson = path.join(folder, 'clip.json');
    this.framesJsonl = path.join(folder, 'frames.jsonl');
    const meta = readJson(this.clipJson);
    const lines = fs.readFileSync(this.framesJsonl, 'utf8').split(/\r?\n/).filter((l) => l.trim()).map((l) => JSON.parse(l));
    this.fps = meta.fps;
    this.frameMs = 1000 / meta.fps;
    this.s0 = meta.clock.wall_clock_unix_ms_at_s0;
    this.geometry = meta.geometry;
    // The clip end: (last instant + 1) * frame_ms.
    this.endMs = (lines.at(-1).instant + 1) * this.frameMs;
    const keyframes = new Set((meta.segments ?? []).flatMap((s) => s.keyframe_instants ?? []));
    this.keyframesMs = [...keyframes].sort((a, b) => a - b).map((k) => k * this.frameMs);
  }
}

/** The separate synapse-mcp instance and the clip operations the driver uses. */
class Recorder {
  constructor(exe, scratch) {
    const db = path.join(scratch, 'db');
    const jobs = path.join(scratch, 'shell-jobs');
    for (const dir of [db, jobs]) fs.mkdirSync(dir, { recursive: true });
    this.srv = new McpServer({ exe, db: path.join(db, 'synapse.db'), outDir: scratch, tag: 'mcp', env: { SYNAPSE_SHELL_JOB_ROOT: jobs } });
  }

  async start() {
    const r = await this.srv.init('d5-proof');
    if (r.error) throw new Error(`synapse-mcp did not start: ${JSON.stringify(r.error)}`);
  }

  /** Starts recording `hwnd` into `folder` (not existing yet); the record response and s_0. */
  async record(hwnd, durationMs, folder) {
    const rec = await this.srv.call({ operation: 'record', window_hwnd: hwnd, duration_ms: durationMs, folder: slash(folder) });
    const r = rec.json?.record;
    if (!r?.clip_id) throw new Error(`record refused: ${rec.texts.join(' ').slice(0, 600) || JSON.stringify(rec.error)}`);
    const s0 = await until(() => ClipFiles.s0(r.folder), 'the clock in clip.json');
    return { ...r, s0 };
  }

  /** Waits for the planned end, then until the clip no longer records. */
  async waitRecorded(clipId, endUnixMs) {
    await sleep(Math.max(0, endUnixMs - Date.now()));
    await until(async () => (await clipStatus(this.srv, clipId)).state !== 'recording', 'the recording to end', CONFIG.waitMs, CONFIG.pollMs);
  }

  async waitReady(clipId) {
    const s = await until(async () => {
      const st = await clipStatus(this.srv, clipId);
      return ['ready', 'failed'].includes(st.state) ? st : null;
    }, `clip ${clipId} to be ready`, CONFIG.readyMs, CONFIG.pollMs);
    if (s.state !== 'ready') throw new Error(`clip ${clipId} failed: ${s.raw}`);
  }

  /** clip_overview of `folder`, then its views/overview_analysis.json. */
  async overview(folder) {
    const ov = await this.srv.call({ operation: 'clip_overview', folder: slash(folder) });
    if (ov.error || ov.isError) throw new Error(`clip_overview failed: ${ov.texts.join(' ').slice(0, 600) || JSON.stringify(ov.error)}`);
    const file = path.join(folder, 'views', 'overview_analysis.json');
    const analysis = readJson(file);
    if (!analysis) throw new Error(`clip_overview wrote no ${file}`);
    return { file, analysis };
  }

  close() { return this.srv.close(); }
}

/** One D5 proof round: records the clips, writes the two d5 inputs, runs the check. */
class D5Proof {
  constructor(exe, scratch, only) {
    this.scratch = scratch;
    this.names = only ?? Object.keys(CLIPS);
    this.files = {
      viewer: path.join(scratch, 'viewer-output.json'),
      schedule: path.join(scratch, 'schedule.json'),
      runs: path.join(scratch, 'd5-runs.json'),
    };
    this.viewer = (only && readJson(this.files.viewer)) || {};
    this.runs = (only && readJson(this.files.runs)) || {};
    this.recorder = new Recorder(exe, scratch);
  }

  async run() {
    try {
      await this.recorder.start();
      for (const name of this.names) {
        try {
          this.viewer[name] = await this.clip(name);
          console.log(`${name}: recorded, ${this.viewer[name].timeline.length} timeline entries (${this.runs[name].folder})`);
        } catch (error) {
          delete this.viewer[name];
          this.runs[name] = { error: String(error?.stack ?? error) };
          console.log(`${name}: FAILED ${error?.message ?? error}`);
        }
        writeJson(this.files.viewer, this.viewer);
        writeJson(this.files.runs, this.runs);
      }
    } finally {
      await this.recorder.close();
    }
    const check = spawnSync(process.execPath, [RUN_SPEC, 'd5', this.files.viewer, this.files.schedule], { stdio: 'inherit' });
    return check.status ?? 1;
  }

  pageUrl(spec, start) {
    const url = pathToFileURL(path.join(PAGES, spec.page));
    if (spec.scene) url.searchParams.set('scene', spec.scene);
    url.searchParams.set('start', String(start));
    return url.href;
  }

  /** Records one clip with its own window (closed before the clip worker finishes). */
  async clip(name) {
    const spec = CLIPS[name];
    const stamp = Date.now();
    const folder = path.join(this.scratch, 'clips', `${name}-${stamp}`);
    fs.mkdirSync(path.dirname(folder), { recursive: true });
    const start = stamp + (spec.run === 'boundary' ? CONFIG.parkedMs : CONFIG.leadMs);
    const win = await TestWindow.open(this.pageUrl(spec, start), path.join(this.scratch, 'chrome', `${name}-${stamp}`));
    let rec;
    let page;
    try {
      await win.ready(spec.page === UI_PAGE ? 'window.actualTiming' : 'window.flashTiming');
      await win.placeBehind();
      ({ rec, page } = await this.record(win, spec, start, folder));
    } finally {
      win.close();
    }
    await this.recorder.waitReady(rec.clip_id);
    const { file, analysis } = await this.recorder.overview(rec.folder);
    const files = new ClipFiles(rec.folder);
    this.runs[name] = { folder: rec.folder, clip_json: files.clipJson, frames_jsonl: files.framesJsonl, overview_analysis: file, s0: files.s0, page };
    if (name === 'proof') {
      writeJson(this.files.schedule, { start: page.start, schedule: page.schedule, actual: page.actual, always: page.always, box_fits: page.box_fits });
    }
    const base = { timeline: analysis.timeline, fps: files.fps, frame_ms: files.frameMs };
    return spec.entry({ base, analysis, files, page, startMs: page.start - files.s0 });
  }

  /** The recording itself: its length by run kind; a boundary page gets its start from s_0. */
  async record(win, spec, start, folder) {
    const schedule = spec.page === UI_PAGE ? await win.read('window.schedule') : null;
    let durationMs = CONFIG.sceneMs;
    if (spec.run === 'proof') {
      const lastMs = Math.max(...schedule.map((r) => r.until_ms ?? r.t_ms));
      durationMs = Math.ceil((start - Date.now() + lastMs + CONFIG.tailMs) / 1000) * 1000;
    } else if (spec.run === 'boundary') {
      durationMs = CONFIG.boundaryMs;
    }
    const rec = await this.recorder.record(win.hwnd, durationMs, folder);
    if (spec.run === 'proof' && start <= rec.s0) console.log(`proof: the schedule started ${rec.s0 - start} ms before s_0; raise leadMs`);
    if (spec.run === 'boundary') {
      // The stop is at durationMs; the first instant that shows the change is quietMs before it,
      // and the change is made onsetPhaseMs after the instant before that one.
      const frameMs = 1000 / rec.fps;
      const row = schedule.find((r) => r.id === BOUNDARY_ROW);
      const pageStart = rec.s0 + durationMs - spec.quietMs(frameMs) - frameMs + CONFIG.onsetPhaseMs - row.t_ms;
      if (pageStart - Date.now() < CONFIG.reloadMs) console.log(`boundary: only ${pageStart - Date.now()} ms left to reload the page; raise boundaryMs`);
      await win.navigate(this.pageUrl(spec, pageStart));
      await win.ready(`window.scheduleStart === ${pageStart}`);
    }
    await this.recorder.waitRecorded(rec.clip_id, rec.s0 + durationMs);
    return { rec, page: await win.read(spec.read) };
  }
}

const [exe, scratch, ...rest] = process.argv.slice(2);
if (!exe || !scratch) {
  console.error('usage: node d5-proof.mjs <synapse-mcp exe> <scratch dir> [--clips name,name]');
  process.exit(2);
}
if (typeof WebSocket !== 'function') {
  console.error('d5-proof needs Node 22 or later (global WebSocket)');
  process.exit(2);
}
const only = rest[0] === '--clips' && rest[1] ? rest[1].split(',') : null;
const unknown = (only ?? []).filter((n) => !CLIPS[n]);
if (unknown.length) {
  console.error(`unknown clip ${unknown.join(', ')}; clips: ${Object.keys(CLIPS).join(', ')}`);
  process.exit(2);
}
fs.mkdirSync(scratch, { recursive: true });
process.exit(await new D5Proof(path.resolve(exe), path.resolve(scratch), only).run());
