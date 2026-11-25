// Live check of the stress-batch fixes, one check per issue (scripts/live-check/PLAN.md).
//
//   node scripts/live-check/batch-live.mjs run=<name> [only=163,199]
//
// Starts target/release/synapse-mcp.exe twice, both with their own database, shell-job root and
// working folder under .runs/live-check/<run>/: one stdio instance (checks with no session state)
// and one HTTP instance on a free loopback port outside 7700-7799, with a bearer token generated
// for this run. Target, browser and lease checks run in their own HTTP sessions there (those tools
// need an MCP session id); transport and permission checks too (default read-only grants). It
// never connects to, signals or shares files with the owner's running server. Each check sends
// its issue's failing input and prints
//   #N PASS|FAIL|SKIP <observed, at most 700 characters>
// Checks whose issue touches a window open their own window (a WinForms test window, or a Chrome
// window with its own --user-data-dir), save <N>-before.png / <N>-after.png through the stdio
// test server's screenshot tool, print those paths, and close what they opened in the same check. results.txt in the run
// folder gets every line. Exit code 0 when the run finished: a FAIL is a result, not an error.
import { spawn, execFileSync } from 'node:child_process';
import crypto from 'node:crypto';
import fs from 'node:fs';
import net from 'node:net';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { McpServer, sleep } from './mcp-stdio.mjs';

const REPO = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..');
const EXE = path.join(REPO, 'target', 'release', 'synapse-mcp.exe');
const OWNER_PORTS = [7700, 7799]; // the owner's port range; never used here
const MAX_OBSERVED = 700; // room for the decisive text plus the screenshot paths

// ---------------------------------------------------------------- arguments and run folder

const argv = Object.fromEntries(process.argv.slice(2).map((a) => {
  const i = a.indexOf('=');
  return i < 0 ? [a, ''] : [a.slice(0, i), a.slice(i + 1)];
}));
if (!argv.run || !/^[A-Za-z0-9._-]+$/.test(argv.run)) {
  console.error('usage: node scripts/live-check/batch-live.mjs run=<name> [only=163,199]');
  process.exit(2);
}
const ONLY = argv.only ? new Set(argv.only.split(',').map((s) => Number(s.trim())).filter(Boolean)) : null;
const RUN_DIR = path.join(REPO, '.runs', 'live-check', argv.run);
const STAMP = Date.now().toString(36); // one invocation; keeps databases and keys of re-runs apart
const TAG = `lc${STAMP}`;
fs.mkdirSync(RUN_DIR, { recursive: true });
const RESULTS = path.join(RUN_DIR, 'results.txt');

// The test servers inherit this environment: drop every SYNAPSE_* setting of the owner's shell
// (mode, bind, db, token, allowlists) so only what this driver sets applies.
for (const k of Object.keys(process.env)) if (k.toUpperCase().startsWith('SYNAPSE_')) delete process.env[k];

// ---------------------------------------------------------------- small helpers

const clip = (s, n = MAX_OBSERVED) => { s = String(s ?? '').replace(/\s+/g, ' ').trim(); return s.length > n ? `${s.slice(0, n - 3)}...` : s; };

/** First value of `key` anywhere inside `obj` (depth-first). */
function dig(obj, key) {
  const stack = [obj];
  while (stack.length) {
    const o = stack.pop();
    if (!o || typeof o !== 'object') continue;
    if (!Array.isArray(o) && Object.prototype.hasOwnProperty.call(o, key) && o[key] !== undefined) return o[key];
    const vals = Array.isArray(o) ? o : Object.values(o);
    for (let i = vals.length - 1; i >= 0; i -= 1) stack.push(vals[i]);
  }
  return undefined;
}

/** Every object inside `obj` for which `pred` is true. */
function digAll(obj, pred) {
  const out = []; const stack = [obj];
  while (stack.length) {
    const o = stack.pop();
    if (!o || typeof o !== 'object') continue;
    if (!Array.isArray(o) && pred(o)) out.push(o);
    for (const v of Array.isArray(o) ? o : Object.values(o)) stack.push(v);
  }
  return out;
}

/** One answer of a tools/call (stdio or HTTP) in one shape: ok, code, message, json, text, bytes. */
function outcome(m) {
  const raw = JSON.stringify(m ?? null);
  if (!m) return { ok: false, code: 'NO_ANSWER', message: 'no JSON-RPC answer', data: null, json: null, text: '', raw, bytes: raw.length };
  if (m.error) {
    const data = m.error.data ?? null;
    return { ok: false, code: (data && data.code) || String(m.error.code), rpcCode: m.error.code, message: m.error.message ?? '', data, json: data, text: JSON.stringify(m.error), raw, bytes: raw.length };
  }
  const res = m.result ?? {};
  const texts = (res.content || []).filter((c) => c.type === 'text').map((c) => c.text);
  let json = res.structuredContent ?? null;
  if (json == null) { try { json = JSON.parse(texts[0]); } catch { json = null; } }
  const isErr = res.isError === true;
  return {
    ok: !isErr,
    code: isErr ? (dig(json, 'code') ?? 'TOOL_ERROR') : null,
    message: isErr ? (dig(json, 'message') ?? texts[0] ?? '') : '',
    data: json, json, text: texts.join('\n'), raw, bytes: raw.length,
  };
}

const said = (o) => (o.ok ? `ok ${clip(o.text, 160)}` : `${o.code}: ${o.message}`);
const result = (pass, observed) => ({ status: pass ? 'PASS' : 'FAIL', observed });
const skip = (reason) => ({ status: 'SKIP', observed: reason });

function powershell(script, timeout = 30000) {
  return execFileSync('powershell', ['-NoProfile', '-NonInteractive', '-Command', script], { encoding: 'utf8', timeout }).trim();
}

function killTree(pid) {
  if (!pid) return;
  try { execFileSync('taskkill', ['/PID', String(pid), '/T', '/F'], { stdio: 'ignore' }); } catch { /* already gone */ }
}

function pidAlive(pid) {
  try { return execFileSync('tasklist', ['/FI', `PID eq ${pid}`, '/NH'], { encoding: 'utf8' }).includes(` ${pid} `); } catch { return false; }
}

async function freePort() {
  for (;;) {
    const port = await new Promise((resolve, reject) => {
      const s = net.createServer(); s.unref();
      s.on('error', reject);
      s.listen(0, '127.0.0.1', () => { const p = s.address().port; s.close(() => resolve(p)); });
    });
    if (port < OWNER_PORTS[0] || port > OWNER_PORTS[1]) return port;
  }
}

async function waitForPort(port, ms) {
  const deadline = Date.now() + ms;
  while (Date.now() < deadline) {
    const up = await new Promise((resolve) => {
      const c = net.connect(port, '127.0.0.1'); c.on('connect', () => { c.destroy(); resolve(true); }); c.on('error', () => resolve(false));
    });
    if (up) return true;
    await sleep(300);
  }
  return false;
}

/** JSON-RPC messages of one HTTP answer: SSE `data:` events, or one JSON body. */
function parseMessages(ctype, text) {
  if ((ctype || '').includes('text/event-stream')) {
    const msgs = [];
    for (const block of text.split(/\r?\n\r?\n/)) {
      const data = block.split(/\r?\n/).filter((l) => l.startsWith('data:')).map((l) => l.slice(5).trimStart()).join('\n');
      if (!data) continue;
      try { msgs.push(JSON.parse(data)); } catch { /* keep-alive or partial */ }
    }
    return msgs;
  }
  try { const j = JSON.parse(text); return Array.isArray(j) ? j : [j]; } catch { return []; }
}

// ---------------------------------------------------------------- HTTP test server

/** One MCP Streamable-HTTP session on the test server. */
class HttpSession {
  constructor(server, sid) { this.server = server; this.sid = sid; this.id = 1000; }

  headers(extra = {}) {
    const h = { 'content-type': 'application/json', accept: 'application/json, text/event-stream', 'mcp-protocol-version': '2025-06-18', authorization: `Bearer ${this.server.token}`, ...extra };
    if (this.sid) h['mcp-session-id'] = this.sid;
    return h;
  }

  /** POST a raw body (string or object); the status, content type, text and parsed messages. */
  async post(body, timeoutMs = 60000) {
    const r = await fetch(this.server.url, { method: 'POST', headers: this.headers(), body: typeof body === 'string' ? body : JSON.stringify(body), signal: AbortSignal.timeout(timeoutMs) });
    const ctype = r.headers.get('content-type') || '';
    const text = await r.text();
    return { status: r.status, ctype, text, msgs: parseMessages(ctype, text), headers: r.headers };
  }

  async request(method, params, timeoutMs = 60000) {
    const id = ++this.id;
    const r = await this.post({ jsonrpc: '2.0', id, method, params }, timeoutMs);
    return r.msgs.find((m) => m.id === id) ?? r.msgs[0] ?? null;
  }

  async call(name, args, timeoutMs = 60000) { return outcome(await this.request('tools/call', { name, arguments: args }, timeoutMs)); }

  async del() {
    const r = await fetch(this.server.url, { method: 'DELETE', headers: this.headers(), signal: AbortSignal.timeout(60000) });
    return { status: r.status, text: await r.text() };
  }
}

class HttpServer {
  async start() {
    this.port = await freePort();
    this.url = `http://127.0.0.1:${this.port}/mcp`;
    this.token = crypto.randomBytes(32).toString('hex'); // this run only; never written to a file
    this.cwd = path.join(RUN_DIR, `http-cwd-${STAMP}`);
    const appdata = path.join(RUN_DIR, `http-appdata-${STAMP}`); // no synapse\token.txt here: the env token is used
    for (const d of [this.cwd, appdata]) fs.mkdirSync(d, { recursive: true });
    // Default M3 grants (read-only), as a stock daemon: #219 needs a permission denial.
    this.p = spawn(EXE, ['--mode', 'http', '--bind', `127.0.0.1:${this.port}`, '--db', path.join(RUN_DIR, `db-http-${STAMP}`), '--log-level', 'info'], {
      cwd: this.cwd, stdio: ['ignore', 'ignore', 'pipe'],
      env: { ...process.env, SYNAPSE_BEARER_TOKEN: this.token, APPDATA: appdata, SYNAPSE_SHELL_JOB_ROOT: path.join(RUN_DIR, `shell-jobs-http-${STAMP}`) },
    });
    this.p.stderr.pipe(fs.createWriteStream(path.join(RUN_DIR, `stderr-http-${STAMP}.log`)));
    if (!(await waitForPort(this.port, 60000))) throw new Error(`HTTP test server did not listen on ${this.port}`);
  }

  /** A new initialized session. `clientName` is its clientInfo.name (the registry infers agent_kind from it). */
  async session(clientName = 'live-check') {
    const s = new HttpSession(this, null);
    const r = await s.post({ jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: clientName, version: '0' } } });
    s.sid = r.headers.get('mcp-session-id');
    if (r.status !== 200 || !s.sid) throw new Error(`initialize: HTTP ${r.status} ${clip(r.text, 200)}`);
    await s.post({ jsonrpc: '2.0', method: 'notifications/initialized', params: {} });
    return s;
  }

  stop() { killTree(this.p?.pid); }
}

// ---------------------------------------------------------------- windows the checks open

const CHROME_CANDIDATES = [
  path.join(process.env['ProgramFiles'] || 'C:\\Program Files', 'Google', 'Chrome', 'Application', 'chrome.exe'),
  path.join(process.env['ProgramFiles(x86)'] || 'C:\\Program Files (x86)', 'Google', 'Chrome', 'Application', 'chrome.exe'),
  path.join(process.env['LOCALAPPDATA'] || '', 'Google', 'Chrome', 'Application', 'chrome.exe'),
  path.join(process.env['ProgramFiles(x86)'] || 'C:\\Program Files (x86)', 'Microsoft', 'Edge', 'Application', 'msedge.exe'),
  path.join(process.env['ProgramFiles'] || 'C:\\Program Files', 'Microsoft', 'Edge', 'Application', 'msedge.exe'),
];
const CHROME = CHROME_CANDIDATES.find((p) => p && fs.existsSync(p)) ?? null;

async function mainWindow(pid, ms = 20000) {
  const deadline = Date.now() + ms;
  while (Date.now() < deadline) {
    let h = 0;
    try { h = Number(powershell(`(Get-Process -Id ${pid} -ErrorAction Stop).MainWindowHandle.ToInt64()`)); } catch { h = 0; }
    if (h > 0) return h;
    await sleep(500);
  }
  throw new Error(`process ${pid} showed no main window in ${ms} ms`);
}

/** A WinForms window (text box + button) in its own powershell process, shown without activation. */
async function openForm(title) {
  const ps = `
Add-Type -AssemblyName System.Windows.Forms
Add-Type -ReferencedAssemblies System.Windows.Forms, System.Drawing -TypeDefinition 'public class LcForm : System.Windows.Forms.Form { protected override bool ShowWithoutActivation { get { return true; } } }'
$f = New-Object LcForm; $f.Text = '${title.replace(/'/g, "''")}'; $f.StartPosition = 'Manual'; $f.Left = 60; $f.Top = 60; $f.Width = 560; $f.Height = 340
$t = New-Object System.Windows.Forms.TextBox; $t.Dock = 'Top'; $t.Text = 'Synapse live check'; $f.Controls.Add($t)
$b = New-Object System.Windows.Forms.Button; $b.Text = 'OK'; $b.Dock = 'Bottom'; $f.Controls.Add($b)
[System.Windows.Forms.Application]::Run($f)`;
  const p = spawn('powershell', ['-NoProfile', '-STA', '-EncodedCommand', Buffer.from(ps, 'utf16le').toString('base64')], { stdio: 'ignore', windowsHide: true });
  const w = { kind: 'form', pid: p.pid, hwnd: 0 };
  w.hwnd = await mainWindow(p.pid);
  return w;
}

/** A Chrome (or Edge) window with its own profile folder and a small test page. */
async function openChrome(n, title) {
  if (!CHROME) return null;
  const html = `<!doctype html><title>${title}</title><body><div><div><div><div><div><div><div><div><form><label>Search <input id="z" name="q"></label> <button type="button">Go</button></form></div></div></div></div></div></div></div></div></body>`;
  const dir = path.join(RUN_DIR, `chrome-${n}-${STAMP}`);
  const p = spawn(CHROME, [`--user-data-dir=${dir}`, '--no-first-run', '--no-default-browser-check', '--force-renderer-accessibility', '--window-position=100,100', '--window-size=900,640', '--new-window', `data:text/html,${encodeURIComponent(html)}`], { stdio: 'ignore' });
  const w = { kind: 'chrome', pid: p.pid, hwnd: 0 };
  w.hwnd = await mainWindow(p.pid, 30000);
  await sleep(2500); // page and accessibility tree
  return w;
}

// ---------------------------------------------------------------- check context

class Ctx {
  constructor(srv) { this.srv = srv; this.http = null; this.windows = []; this.pids = []; this.sessions = []; this.toolsCache = null; }

  async call(name, args, timeoutMs) { return outcome(await this.srv.tool(name, args, timeoutMs)); }

  async tools() {
    if (!this.toolsCache) { const r = await this.srv.rpc('tools/list', {}); this.toolsCache = r.result?.tools ?? []; }
    return this.toolsCache;
  }

  async tool(name) { return (await this.tools()).find((t) => t.name === name) ?? null; }

  /** The JSON schema of one property path inside a tool's input schema, $ref and anyOf resolved. */
  async prop(toolName, ...names) {
    const t = await this.tool(toolName); if (!t) return null;
    const root = t.inputSchema;
    const resolve = (s) => {
      for (let i = 0; s && i < 8; i += 1) {
        if (s.$ref) { s = root.$defs?.[s.$ref.split('/').pop()] ?? root.definitions?.[s.$ref.split('/').pop()]; continue; }
        const branches = s.anyOf || s.oneOf;
        if (branches && !s.properties && s.minimum === undefined) { const b = branches.find((x) => x.type !== 'null'); if (b) { s = b; continue; } }
        break;
      }
      return s;
    };
    let s = resolve(root);
    for (const n of names) { s = resolve(s?.properties?.[n]); if (!s) return null; }
    return s;
  }

  async httpServer() {
    if (!this.http) { this.http = new HttpServer(); await this.http.start(); }
    return this.http;
  }

  /**
   * A session on the HTTP test server, closed (DELETE) when the check ends. Target, browser and
   * lease tools need one: on the stdio server they answer "require an MCP session id".
   */
  async session(clientName) {
    const s = await (await this.httpServer()).session(clientName);
    this.sessions.push(s);
    return s;
  }

  async form(n) { const w = await openForm(`Synapse live check #${n} ${TAG}`); this.windows.push(w); return w; }

  async chrome(n) { const w = await openChrome(n, `LiveCheck-${n}-${TAG}`); if (w) this.windows.push(w); return w; }

  track(pid) { if (pid) this.pids.push(pid); }

  /** Screenshot of one window through the test server's own screenshot tool. */
  async shot(n, hwnd, label) {
    const file = path.join(RUN_DIR, `${n}-${label}.png`);
    const o = await this.call('screenshot', { operation: 'capture', path: file, window_hwnd: hwnd, overwrite: true }, 60000);
    const rel = path.relative(REPO, file);
    return o.ok ? rel : `${rel} failed (${o.code})`;
  }

  /** Closes every HTTP session, window and process this check opened. */
  async cleanup() {
    for (const s of this.sessions.splice(0)) await s.del().catch(() => {});
    for (const w of this.windows.splice(0)) killTree(w.pid);
    for (const pid of this.pids.splice(0)) killTree(pid);
  }
}

// ---------------------------------------------------------------- shared steps

const proc = (o) => Number(dig(o.json, 'pid')) || 0;

async function shellStart(ctx, args) {
  const o = await ctx.call('shell', { operation: 'start', ...args });
  const pid = proc(o); ctx.track(pid);
  return o;
}

async function templateAndTask(ctx, taskId) {
  await ctx.call('agent', { operation: 'template_put', template_put: { template_id: `${TAG}-tpl`, name: 'live check', model: 'claude', prompt: 'live check template, never spawned' } });
  return ctx.call('task', { operation: 'create', create: { task_id: taskId, title: 'live check', template_id: `${TAG}-tpl` } });
}

function cfBytes(cf, name) {
  if (!cf) return undefined;
  if (Array.isArray(cf)) { const e = cf.find((x) => x.name === name || x.cf === name || x.cf_name === name); return e ? Number(e.bytes ?? e.size_bytes ?? e.value) : undefined; }
  const v = cf[name];
  return typeof v === 'object' && v ? Number(v.bytes ?? v.size_bytes ?? v.estimate) : Number(v);
}

/** Sum of a tool_usage counter over every telemetry row of one tool. */
const usage = (json, tool, field) => digAll(json, (o) => o.tool === tool && typeof o[field] === 'number').reduce((a, o) => a + o[field], 0);

const rpcErr = (r) => r.msgs.find((m) => m.error)?.error ?? null;

/** Opens and closes HTTP sessions, `width` at a time. Returns the session ids. */
async function churnSessions(http, count, width = 10) {
  const sids = [];
  for (let i = 0; i < count; i += width) {
    const batch = await Promise.all(Array.from({ length: Math.min(width, count - i) }, async () => {
      const s = await http.session(); await s.del(); return s.sid;
    }));
    sids.push(...batch);
  }
  return sids;
}

// ---------------------------------------------------------------- the checks, one per issue

const CHECKS = {
  // storage size estimates count memtables; health labels them as estimates
  144: async (ctx) => {
    await ctx.call('shell', { command: 'cmd.exe', args: ['/c', 'exit 0'] });
    const l = await ctx.call('process', { operation: 'launch', target: 'cmd.exe', args: ['/c', 'exit 0'], windows_console_window_state: 'hidden' });
    ctx.track(proc(l));
    const h = await ctx.call('health', {});
    const cf = dig(h.json, 'cf_sizes');
    const act = cfBytes(cf, 'CF_ACTION_LOG'); const hist = cfBytes(cf, 'CF_PROCESS_HISTORY');
    const labelled = /size estimates/i.test(h.text);
    return result(h.ok && (act > 0 || hist > 0) && labelled, `health cf_sizes CF_ACTION_LOG=${act} CF_PROCESS_HISTORY=${hist}; estimate label ${labelled ? 'present' : 'missing'}${h.ok ? '' : `; ${said(h)}`}`);
  },

  // process launch honours idempotency_key
  146: async (ctx) => {
    const base = { operation: 'launch', target: 'cmd.exe', args: ['/c', 'exit 0'], windows_console_window_state: 'hidden', idempotency_key: `${TAG}-146` };
    const a = await ctx.call('process', base); const b = await ctx.call('process', base);
    const c = await ctx.call('process', { ...base, args: ['/c', 'exit 5'] });
    [a, b, c].forEach((o) => ctx.track(proc(o)));
    const pass = a.ok && b.ok && proc(a) > 0 && proc(a) === proc(b) && !c.ok && /already used for different parameters/.test(c.message);
    return result(pass, `pids ${proc(a)}, replay ${proc(b)}; changed args: ${said(c)}`);
  },

  // run idempotency fingerprint covers execution_mode and durable_timeout_ms
  148: async (ctx) => {
    const k1 = `${TAG}-148d`; const k2 = `${TAG}-148m`;
    const d = await ctx.call('shell', { command: 'cmd.exe', args: ['/c', 'echo d1'], execution_mode: 'durable', idempotency_key: k1 });
    const i = await ctx.call('shell', { command: 'cmd.exe', args: ['/c', 'echo d1'], execution_mode: 'inline', idempotency_key: k1 });
    const m = await ctx.call('shell', { command: 'cmd.exe', args: ['/c', 'echo m2'], execution_mode: 'inline', idempotency_key: k2 });
    const t = await ctx.call('shell', { command: 'cmd.exe', args: ['/c', 'echo m2'], execution_mode: 'inline', idempotency_key: k2, durable_timeout_ms: 999 });
    const re = /already used for different parameters/;
    return result(d.ok && m.ok && !i.ok && re.test(i.message) && !t.ok && re.test(t.message), `mode change: ${said(i)} | cap change: ${said(t)}`);
  },

  // tool_usage counts an act whose action.ok is false as an error (2d1a0ffa: top-level ok, isError,
  // or act's action.ok). HTTP session: target = the test window, then set_window_bounds 1x1, which
  // Windows clamps to its minimum size: delivered, readback size_satisfied false, so act answers
  // without an error and action.ok false (status verify_needed).
  162: async (ctx, n) => {
    const w = await ctx.form(n);
    const s = await ctx.session();
    const shots = [await ctx.shot(n, w.hwnd, 'before')];
    const set = await s.call('target', { operation: 'set', target: { kind: 'window', window_hwnd: w.hwnd } });
    if (!set.ok) return result(false, `target set: ${said(set)}; ${shots.join(' ')}`);
    const before = await s.call('telemetry', { operation: 'status', status: {} });
    let falses = 0; const seen = [];
    try {
      for (let i = 0; i < 2; i += 1) {
        const o = await s.call('act', { action: { verb: 'set_window_bounds', width: 1, height: 1 } });
        const a = o.json?.action;
        if (o.ok && a?.ok === false) falses += 1;
        seen.push(o.ok ? `action.ok=${a?.ok} status ${a?.status} ${a?.delivery_state} actual ${a?.result?.actual?.width}x${a?.result?.actual?.height}` : said(o));
      }
    } finally { await s.call('target', { operation: 'clear' }); }
    shots.push(await ctx.shot(n, w.hwnd, 'after'));
    const after = await s.call('telemetry', { operation: 'status', status: {} });
    if (!before.ok || !after.ok) return result(false, `telemetry status: ${said(before.ok ? after : before)}; acts: ${seen.join(' | ')}; ${shots.join(' ')}`);
    const dErr = usage(after.json, 'act', 'error_total') - usage(before.json, 'act', 'error_total');
    const dOk = usage(after.json, 'act', 'ok_total') - usage(before.json, 'act', 'ok_total');
    if (falses === 0) return result(false, `no act result with action.ok false (${seen.join(' | ')}); act ok_total +${dOk} error_total +${dErr}; ${shots.join(' ')}`);
    return result(dErr === falses && dOk === 0, `${falses} act calls answered with action.ok false (${seen.join(' | ')}); act tool_usage error_total +${dErr}, ok_total +${dOk}; ${shots.join(' ')}`);
  },

  // observe include=interactable reaches the controls find reaches (Chrome page, depth > 6)
  163: async (ctx, n) => {
    const w = await ctx.chrome(n);
    if (!w) return skip('no Chrome or Edge installed');
    const shots = [await ctx.shot(n, w.hwnd, 'before')];
    const ob = await ctx.call('observe', { include: ['interactable'], window_hwnd: w.hwnd, max_elements: 200 });
    const fd = await ctx.call('find', { role: 'edit', window_hwnd: w.hwnd });
    shots.push(await ctx.shot(n, w.hwnd, 'after'));
    const isEdit = (o) => typeof o.role === 'string' && /edit|textbox|text_field/i.test(o.role);
    const obEdits = digAll(ob.json, isEdit).length;
    const fdEdits = digAll(fd.json, isEdit).length || (fd.ok ? (dig(fd.json, 'matches')?.length ?? dig(fd.json, 'count') ?? 0) : 0);
    const total = dig(ob.json, 'total') ?? dig(ob.json, 'elements_total');
    return result(ob.ok && obEdits > 0, `observe interactable: ${ob.ok ? `${obEdits} edit(s), total ${total}` : said(ob)}; find role=edit: ${fd.ok ? `${fdEdits}` : said(fd)}; ${shots.join(' ')}`);
  },

  // shell cancel never overwrites the monitor's timed_out status
  174: async (ctx) => {
    const ip = 2 + crypto.randomInt(250);
    const jobId = `${TAG}-174`;
    const s = await shellStart(ctx, { command: 'ping.exe', args: ['-n', '30', `127.0.0.${ip}`], timeout_ms: 3000, job_id: jobId });
    if (!s.ok) return result(false, `start: ${said(s)}`);
    await sleep(2950);
    const c = await ctx.call('shell', { operation: 'cancel', job_id: jobId });
    await sleep(500);
    const st = await ctx.call('shell', { operation: 'status', job_id: jobId, tail_bytes: 0 });
    const job = dig(st.json, 'job') ?? st.json ?? {};
    const status = job.status; const dur = job.duration_ms; const term = dig(c.json, 'termination_status');
    const lie = status === 'cancelled' && (term === 'already_exited' || (dur ?? 0) > 3500) && job.exit_code == null;
    return result(st.ok && (status === 'timed_out' || (status === 'cancelled' && !lie)), `cancel at cap: status ${status}, termination_status ${term}, duration_ms ${dur}, exit_code ${job.exit_code}`);
  },

  // task claim refuses a session_id that is not a live MCP session
  180: async (ctx) => {
    const t = await templateAndTask(ctx, `${TAG}-t180`);
    const c = await ctx.call('task', { operation: 'claim', claim: { task_id: `${TAG}-t180`, session_id: 'x'.repeat(100000) } });
    return result(t.ok && !c.ok && c.code === 'TOOL_PARAMS_INVALID' && /not a live MCP session/.test(c.message) && c.bytes < 4000, `${t.ok ? '' : `create: ${said(t)}; `}claim: ${said(c)} (${c.bytes} bytes)`);
  },

  // agent stats per-agent breakdown is bounded by per_agent_limit
  181: async (ctx) => {
    const bad = await ctx.call('agent', { operation: 'stats', stats: { group_by: 'agent', per_agent_limit: 0 } });
    const good = await ctx.call('agent', { operation: 'stats', stats: { group_by: 'agent', per_agent_limit: 1 } });
    return result(!bad.ok && /AGENT_STATS_LIMIT_INVALID/.test(bad.raw) && good.ok, `per_agent_limit 0: ${said(bad)}; per_agent_limit 1: ${good.ok ? 'ok' : said(good)}`);
  },

  // hygiene scan_text returns at most 1000 matches and reports the total
  183: async (ctx) => {
    const text = Array.from({ length: 5000 }, () => 'x').join('\u200b');
    const o = await ctx.call('hygiene', { operation: 'scan_text', scan_text: { text, min_score: 0 } }, 120000);
    const matches = dig(o.json, 'matches'); const total = dig(o.json, 'matches_total'); const cut = dig(o.json, 'matches_truncated');
    return result(o.ok && Array.isArray(matches) && matches.length <= 1000 && total > 1000 && cut === true, `${o.ok ? `matches ${matches?.length}, matches_total ${total}, matches_truncated ${cut}, ${o.bytes} bytes` : said(o)}`);
  },

  // the registry keeps at most 256 closed sessions; a pruned closed session stays closed
  184: async (ctx) => {
    const http = await ctx.httpServer();
    const sids = await churnSessions(http, 300);
    const first = sids[0];
    const again = await new HttpSession(http, first).del();
    const probe = await http.session();
    const list = await probe.call('session', { operation: 'list', include_closed: true, limit: 500 });
    await probe.del();
    const count = dig(list.json, 'registry_entry_count');
    const reappeared = list.text.includes(first) || list.raw.includes(first);
    return result(list.ok && typeof count === 'number' && count <= 270 && !reappeared, `300 sessions opened+closed; DELETE of the first again: HTTP ${again.status}; registry_entry_count ${count}; first id listed again: ${reappeared}${list.ok ? '' : `; ${said(list)}`}`);
  },

  // closed sessions are bounded across repeated initialize floods
  186: async (ctx) => {
    const http = await ctx.httpServer();
    const counts = [];
    for (let round = 0; round < 2; round += 1) {
      await churnSessions(http, 200, 20);
      const probe = await http.session();
      const list = await probe.call('session', { operation: 'list', include_closed: true, limit: 500 });
      await probe.del();
      counts.push(dig(list.json, 'registry_entry_count'));
    }
    return result(counts.every((c) => typeof c === 'number' && c <= 270), `registry_entry_count after 200 and 400 initialize+DELETE: ${counts.join(' -> ')} (bound 256 closed + open)`);
  },

  // /mcp refuses every undecodable body with a JSON-RPC error, never 415
  189: async (ctx) => {
    const http = await ctx.httpServer(); const s = await http.session();
    const deep = `{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"workspace","arguments":{"operation":"put","put":{"key":"${TAG}-deep","value":${'['.repeat(200)}1${']'.repeat(200)}}}}}`;
    const cases = [
      ['truncated', '{"jsonrpc":', -32700], ['number', '42', -32600], ['batch', '[{"jsonrpc":"2.0","id":1,"method":"ping"}]', -32600],
      ['jsonrpc 1.0', '{"jsonrpc":"1.0","id":1,"method":"ping"}', -32600], ['error:7', '{"jsonrpc":"2.0","id":1,"error":7}', -32600],
      ['error.code twice', '{"jsonrpc":"2.0","id":1,"error":{"code":1,"code":2,"message":"x"}}', -32600], ['nesting 200', deep, -32700],
    ];
    const seen = []; let pass = true;
    for (const [name, body, want] of cases) {
      const r = await s.post(body); const e = rpcErr(r);
      const ok = r.status === 400 && e?.code === want;
      pass &&= ok; seen.push(`${name} ${r.status}/${e?.code ?? clip(r.text, 30)}${ok ? '' : '!'}`);
    }
    await s.del();
    return result(pass, seen.join('; '));
  },

  // empty setup repair.reason names the field
  191: async (ctx) => {
    const a = await ctx.call('setup', { operation: 'repair', repair: { reason: '' } });
    const b = await ctx.call('setup', { operation: 'repair', repair: { reason: '   ' } });
    const re = /requires a non-empty repair\.reason/;
    return result(!a.ok && !b.ok && re.test(a.message) && re.test(b.message), `'': ${said(a)} | '   ': ${clip(b.message, 80)}`);
  },

  // unknown shell job id: typed not-found that names the job, no store path
  193: async (ctx) => {
    const id = `${TAG}-bogus-job`;
    const a = await ctx.call('shell', { operation: 'status', job_id: id });
    const b = await ctx.call('shell', { operation: 'cancel', job_id: id });
    const good = (o) => !o.ok && o.code === 'TOOL_PARAMS_INVALID' && o.message.includes(`job ${id} was not found`) && !/status\.json/.test(o.message);
    return result(good(a) && good(b), `status: ${said(a)} | cancel: ${clip(b.message, 80)}`);
  },

  // a command that cannot be started names itself; a bad working_dir is not "program not found"
  194: async (ctx) => {
    const a = await ctx.call('shell', { command: 'nonexistent_cmd_xyz.exe' });
    const b = await ctx.call('shell', { command: 'cmd.exe', args: ['/c', 'exit 0'], working_dir: `C:\\no\\such\\dir-${TAG}` });
    const c = await ctx.call('process', { operation: 'launch', target: `C:\\no\\such\\dir-${TAG}\\missing.exe`, windows_console_window_state: 'hidden' });
    const pass = !a.ok && a.message.includes("could not start 'nonexistent_cmd_xyz.exe'") && dig(a.json, 'reason') === 'command_not_found'
      && !b.ok && !/program not found/.test(b.message) && !c.ok && /could not start/.test(c.message);
    return result(pass, `missing program: ${said(a)} [${dig(a.json, 'reason')}] | bad working_dir: ${clip(b.message, 70)} [${dig(b.json, 'reason')}] | launch: ${clip(c.message, 60)}`);
  },

  // version-conflict text fits delete
  195: async (ctx) => {
    const key = `${TAG}-cas`;
    const p = await ctx.call('workspace', { operation: 'put', put: { key, value: 1 } });
    const d = await ctx.call('workspace', { operation: 'delete', delete: { key } });
    return result(p.ok && !d.ok && /expected_version is required because the key exists at version/.test(d.message), `put ${p.ok ? 'ok' : said(p)}; delete without expected_version: ${said(d)}`);
  },

  // target set kind=cdp on a non-browser window: early ACTION_TARGET_INVALID, no bridge guide (HTTP session)
  199: async (ctx, n) => {
    const w = await ctx.form(n);
    const s = await ctx.session();
    const shots = [await ctx.shot(n, w.hwnd, 'before')];
    const t0 = Date.now();
    const o = await s.call('target', { operation: 'set', target: { kind: 'cdp', window_hwnd: w.hwnd, cdp_target_id: 'bogus' } });
    const ms = Date.now() - t0;
    await s.call('target', { operation: 'clear' });
    shots.push(await ctx.shot(n, w.hwnd, 'after'));
    return result(!o.ok && o.code === 'ACTION_TARGET_INVALID' && /requires a Chromium browser window/.test(o.message) && o.bytes < 2000, `${said(o)} (${o.bytes} bytes, ${ms} ms); ${shots.join(' ')}`);
  },

  // CDP tab verbs on a native-window target say the target is a window (HTTP session)
  200: async (ctx, n) => {
    const w = await ctx.form(n);
    const s = await ctx.session();
    const shots = [await ctx.shot(n, w.hwnd, 'before')];
    const set = await s.call('target', { operation: 'set', target: { kind: 'window', window_hwnd: w.hwnd } });
    let nav; let ev;
    try {
      nav = await s.call('act', { action: { verb: 'navigate', url: 'about:blank' } });
      ev = await s.call('act', { action: { verb: 'dispatch_event', event_type: 'click', selector: 'body' } });
    } finally { await s.call('target', { operation: 'clear' }); }
    shots.push(await ctx.shot(n, w.hwnd, 'after'));
    const re = /needs a browser tab target, but this session's target is the native window/;
    // a13e112e: act still answers the refusal inside its result (action.ok false, refused_before_delivery).
    const good = (o) => re.test(o.raw) && /ACTION_TARGET_INVALID/.test(o.raw);
    const show = (o) => (o.ok ? `action.ok=${o.json?.action?.ok} ${o.json?.action?.delivery_state} ${dig(o.json?.action, 'code')}: ${clip(dig(o.json?.action, 'message') ?? o.text, 140)}` : said(o));
    return result(set.ok && good(nav) && good(ev), `${set.ok ? '' : `set: ${said(set)}; `}navigate: ${show(nav)} | dispatch_event: ${show(ev)}; ${shots.join(' ')}`);
  },

  // tools/call with missing or malformed params is -32602
  206: async (ctx) => {
    const http = await ctx.httpServer(); const s = await http.session();
    const bodies = [
      { jsonrpc: '2.0', id: 5, method: 'tools/call' },
      { jsonrpc: '2.0', id: 6, method: 'tools/call', params: { name: 'health', arguments: 'str' } },
      { jsonrpc: '2.0', id: 7, method: 'tools/call', params: { name: 'health', arguments: [1] } },
    ];
    const seen = []; let pass = true;
    for (const b of bodies) {
      const r = await s.post(b); const e = rpcErr(r);
      const ok = e?.code === -32602 && e?.data?.code === 'TOOL_PARAMS_INVALID';
      pass &&= ok; seen.push(`id ${b.id}: ${r.status} ${e?.code ?? '-'} ${e?.data?.code ?? clip(r.text, 40)}`);
    }
    await s.del();
    return result(pass, seen.join('; '));
  },

  // a request with id null is refused, not swallowed as a notification
  207: async (ctx) => {
    const http = await ctx.httpServer(); const s = await http.session();
    const r = await s.post('{"jsonrpc":"2.0","id":null,"method":"tools/call","params":{"name":"health","arguments":{}}}');
    const e = rpcErr(r); await s.del();
    return result(r.status === 400 && e?.code === -32600, `HTTP ${r.status} ${e ? `${e.code} ${e.message}` : clip(r.text, 120) || '(empty body)'}`);
  },

  // target ttl_ms schema publishes the claim bounds
  209: async (ctx) => {
    const p = await ctx.prop('target', 'ttl_ms');
    return result(p?.minimum === 1000 && p?.maximum === 600000, `tools/list target.ttl_ms minimum ${p?.minimum}, maximum ${p?.maximum}`);
  },

  // task list max, concurrency_cap and update.priority follow their published bounds
  210: async (ctx) => {
    const pr = await ctx.prop('task', 'update', 'priority');
    const l = await ctx.call('task', { operation: 'list', list: { max: 0 } });
    const nx = await ctx.call('task', { operation: 'next', next: { concurrency_cap: 0 } });
    return result(pr?.minimum === 1 && pr?.maximum === 5 && !l.ok && !nx.ok, `schema update.priority ${pr?.minimum}..${pr?.maximum}; list max 0: ${l.ok ? 'accepted' : clip(l.message, 70)}; next cap 0: ${nx.ok ? 'accepted' : clip(nx.message, 70)}`);
  },

  // template_list refuses max outside 1..=1000
  211: async (ctx) => {
    const o = await ctx.call('agent', { operation: 'template_list', template_list: { max: 0 } });
    return result(!o.ok && o.code === 'TOOL_PARAMS_INVALID' && /max must be 1\.\.=1000, got 0/.test(o.message), said(o));
  },

  // workspace put documents that null counts as omitted
  212: async (ctx) => {
    const o = await ctx.call('workspace', { operation: 'put', put: { key: `${TAG}-null`, value: null } });
    const d = (await ctx.prop('workspace', 'put', 'value'))?.description ?? '';
    return result(!o.ok && /a JSON null value counts as omitted/.test(o.message) && /null/i.test(d), `${said(o)} | schema: ${clip(d, 90)}`);
  },

  // subscribe schema states buffer_size is fixed at 4096
  218: async (ctx) => {
    const p = await ctx.prop('subscribe', 'buffer_size');
    return result(p?.minimum === 4096 && p?.maximum === 4096, `tools/list subscribe.buffer_size minimum ${p?.minimum}, maximum ${p?.maximum}; ${clip(p?.description, 80)}`);
  },

  // permission denials name the public tool and operation (HTTP instance: default read-only grants)
  219: async (ctx) => {
    const http = await ctx.httpServer(); const s = await http.session();
    const o = await s.call('approval', { operation: 'list', list: {} });
    await s.del();
    return result(!o.ok && /^approval operation=list requires permission /.test(o.message) && /SAFETY_PERMISSION_DENIED/.test(o.raw), said(o));
  },

  // a relative working_dir resolves against the session cwd; drive-relative is refused (HTTP session)
  220: async (ctx) => {
    const http = await ctx.httpServer(); const s = await http.session();
    const dot = await s.call('shell', { command: 'cmd.exe', args: ['/c', 'cd'], working_dir: '.' });
    const drv = await s.call('shell', { command: 'cmd.exe', args: ['/c', 'cd'], working_dir: 'C:' });
    await s.del();
    const out = String(dig(dot.json, 'stdout') ?? '').trim();
    // Before the fix '.' was the daemon cwd (http-cwd-*); the session cwd lives under shell-sessions.
    const underSession = /shell-sessions/i.test(out);
    return result(dot.ok && underSession && !drv.ok && dig(drv.json, 'reason') === 'working_dir_drive_relative', `'.' -> ${dot.ok ? clip(out, 120) : said(dot)} | 'C:' -> ${said(drv)} [${dig(drv.json, 'reason')}]`);
  },

  // done and cancelled tasks refuse every update
  221: async (ctx) => {
    const id = `${TAG}-t221`;
    const t = await templateAndTask(ctx, id);
    const c = await ctx.call('task', { operation: 'cancel', cancel: { task_id: id, reason: 'live check' } });
    const u = await ctx.call('task', { operation: 'update', update: { task_id: id, title: 'renamed after cancel' } });
    return result(t.ok && c.ok && !u.ok && /AGENT_TASK_INVALID_TRANSITION/.test(u.raw) && /terminal\) and can no longer be updated/.test(u.message), `${t.ok ? '' : `create: ${said(t)}; `}cancel ${c.ok ? 'ok' : said(c)}; update: ${said(u)}`);
  },

  // audit profile_intelligence refuses an unknown profile_id
  222: async (ctx) => {
    const a = await ctx.call('audit', { operation: 'profile_intelligence', profile_intelligence: { profile_id: 'no-such-profile-lc' } });
    const b = await ctx.call('audit', { operation: 'profile_intelligence', profile_intelligence: { profile_id: '../../../windows' } });
    return result(!a.ok && /PROFILE_NOT_FOUND/.test(a.raw) && !b.ok, `unknown: ${said(a)} | traversal: ${b.ok ? 'accepted' : b.code}`);
  },

  // storage summary names its figures as estimates (21fdff37: the facade contract row of storage
  // summary reads "RocksDB CF size estimates (live data + memtables) + estimated row counts", no
  // longer "... + exact row counts"; the summary itself reports metrics_mode ..._estimated_row_counts)
  223: async (ctx) => {
    const o = await ctx.call('storage', { operation: 'summary', summary: {} });
    const p = await ctx.call('profile', { operation: 'status', detail: true });
    const contract = digAll(p.json, (x) => x.tool_name === 'storage' && Array.isArray(x.operations))[0];
    const row = contract?.operations?.find((op) => op.operation === 'summary');
    const sot = String(row?.source_of_truth ?? '');
    const mode = String(dig(o.json, 'metrics_mode') ?? '');
    const pass = o.ok && p.ok && /size estimates/.test(sot) && /estimated row counts/.test(sot) && !/exact row counts/.test(sot) && /estimated_row_counts/.test(mode);
    return result(pass, `summary ${o.ok ? `metrics_mode ${mode}` : said(o)}; contract storage.summary source_of_truth: ${p.ok ? (row ? JSON.stringify(sot) : 'row not found') : said(p)}`);
  },

  // out-of-range readback limits are refused, not clamped
  224: async (ctx) => {
    const a = await ctx.call('verification', { operation: 'sources', sources: { max: 0 } });
    const b = await ctx.call('escalation', { operation: 'list', list: { limit: 501 } });
    const c = await ctx.call('assist', { operation: 'suggestion_list', suggestion_list: { limit: 1001 } });
    const pass = !a.ok && /max must be between 1 and 1000; got 0/.test(a.message) && !b.ok && /limit must be between 1 and 500; got 501/.test(b.message) && !c.ok && /limit must be between 1 and 1000; got 1001/.test(c.message);
    return result(pass, `sources max 0: ${clip(a.ok ? 'accepted' : a.message, 80)} | escalation 501: ${clip(b.ok ? 'accepted' : b.message, 80)} | suggestion 1001: ${clip(c.ok ? 'accepted' : c.message, 80)}`);
  },

  // operations with all-default payloads may omit the payload; matching_payload_present tells the truth
  226: async (ctx) => {
    const s = await ctx.call('storage', { operation: 'summary' });
    const m = await ctx.call('model', { operation: 'list' });
    const h = await ctx.call('hygiene', { operation: 'flags' });
    const x = await ctx.call('hygiene', { operation: 'flags', scan_text: { text: 'x' } });
    const present = dig(x.json, 'matching_payload_present');
    return result(s.ok && m.ok && h.ok && !x.ok && present === false, `storage summary ${s.ok ? 'ok' : said(s)}; model list ${m.ok ? 'ok' : m.code}; hygiene flags ${h.ok ? 'ok' : h.code}; flags + scan_text payload: ${x.ok ? 'accepted' : x.code}, matching_payload_present ${present}`);
  },

  // task create refuses a template_id with no stored template
  227: async (ctx) => {
    const o = await ctx.call('task', { operation: 'create', create: { task_id: `${TAG}-t227`, title: 'x', template_id: `${TAG}-no-such-template` } });
    return result(!o.ok && /AGENT_TEMPLATE_NOT_FOUND/.test(o.raw), said(o));
  },

  // a terminal run_shell readback reports output lengths measured after exit (4 runs)
  229: async (ctx) => {
    const seen = []; let pass = true;
    for (let i = 0; i < 4; i += 1) {
      const o = await ctx.call('act', { action: { verb: 'run_shell', command: 'powershell.exe', args: ['-NoProfile', '-Command', "(Start-Process powershell -ArgumentList '-NoProfile','-Command','Start-Sleep 120' -WindowStyle Hidden -PassThru).Id"], timeout_ms: 20000 } });
      const out = String(dig(o.json, 'stdout') ?? '');
      const child = Number(out.trim()); if (child > 0) ctx.track(child);
      const len = dig(o.json, 'stdout_len_bytes'); const state = dig(o.json, 'output_state');
      const ok = o.ok && out.length > 0 && len === Buffer.byteLength(out) && state !== 'terminal_no_output';
      pass &&= ok; seen.push(o.ok ? `${JSON.stringify(out.trim())} len ${len} ${state}` : o.code);
    }
    return result(pass, seen.join('; '));
  },

  // shell cancel names the process set its termination covers (WMI-started process escapes)
  235: async (ctx) => {
    const ip = `127.0.2.${2 + crypto.randomInt(250)}`;
    const jobId = `${TAG}-235`;
    const s = await shellStart(ctx, { command: 'powershell.exe', args: ['-NoProfile', '-Command', `Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{CommandLine='ping.exe -n 14 ${ip}'} | Out-Null; Start-Sleep 30`], timeout_ms: 40000, job_id: jobId });
    if (!s.ok) return result(false, `start: ${said(s)}`);
    await sleep(4000);
    const c = await ctx.call('shell', { operation: 'cancel', job_id: jobId });
    const l = await ctx.call('process', { operation: 'list', command_line_contains: ip, include_command_line: true });
    const escaped = digAll(l.json, (o) => typeof o.pid === 'number' && typeof o.command_line === 'string' && o.command_line.includes(ip)).map((o) => o.pid);
    escaped.forEach((p) => ctx.track(p));
    const scope = dig(c.json, 'termination_scope'); const note = String(dig(c.json, 'termination_scope_note') ?? '');
    return result(c.ok && scope === 'job_root_process_and_parent_pid_descendants' && /WMI/.test(note), `termination_status ${dig(c.json, 'termination_status')}, scope ${scope}; note: ${clip(note, 110)}; escaped ping pids [${escaped.join(',')}] (closed)`);
  },

  // the shell record is the line the child got; policy spawned_line_v2; batch files say not reproduced
  236: async (ctx) => {
    const jobId = `${TAG}-236`;
    const s = await shellStart(ctx, { command: 'cmd.exe', args: ['/c', 'ping -n 4 127.0.0.98'], timeout_ms: 20000, job_id: jobId });
    const st = await ctx.call('shell', { operation: 'status', job_id: jobId, tail_bytes: 0 });
    const rec = dig(st.json, 'command_line'); const policy = dig(st.json, 'command_metadata_policy'); const pid = proc(st) || proc(s);
    const pl = await ctx.call('process', { operation: 'list', pid, include_command_line: true });
    const live = dig(pl.json, 'command_line');
    const bat = path.join(RUN_DIR, `lc-236-${STAMP}.cmd`);
    fs.writeFileSync(bat, '@echo %1\r\n');
    const b = await ctx.call('shell', { command: bat, args: ['a'] });
    const nr = String(dig(b.json, 'command_line_not_reproduced') ?? '');
    const pass = s.ok && rec && rec === live && policy === 'spawned_line_v2' && nr.startsWith('batch_file_cmd_exe_wrapper');
    return result(pass, `recorded ${JSON.stringify(rec)} live ${JSON.stringify(live)}; policy ${policy}; .cmd: ${b.ok ? clip(nr, 60) || 'no command_line_not_reproduced' : said(b)}`);
  },

  // a second request with an id still in flight in the same session is refused
  243: async (ctx) => {
    const http = await ctx.httpServer(); const s = await http.session();
    const slow = s.post({ jsonrpc: '2.0', id: 7, method: 'tools/call', params: { name: 'workspace', arguments: { operation: 'wait', wait: { key: `${TAG}-absent`, timeout_ms: 4000 } } } });
    await sleep(500);
    const dup = await s.post({ jsonrpc: '2.0', id: 7, method: 'tools/call', params: { name: 'health', arguments: {} } });
    const first = await slow;
    const reuse = await s.post({ jsonrpc: '2.0', id: 7, method: 'tools/call', params: { name: 'health', arguments: {} } });
    await s.del();
    const e = rpcErr(dup); const firstMsg = first.msgs.find((m) => m.id === 7);
    const reuseOk = reuse.msgs.some((m) => m.id === 7 && m.result);
    return result(dup.status === 400 && e?.code === -32600 && /still in flight/.test(e?.message ?? '') && !!firstMsg && reuseOk, `duplicate id 7: HTTP ${dup.status} ${e ? `${e.code} ${clip(e.message, 90)}` : clip(dup.text, 60)}; first call answered: ${!!firstMsg}; id 7 reused after: ${reuseOk}`);
  },

  // browser_form runs the shared cdp_target_id validator
  245: async (ctx) => {
    const sv = (id) => ctx.call('browser_form', { operation: 'set_value', set_value: { selector: '#z', text: 'x', cdp_target_id: id } });
    const a = await sv(''); const b = await sv('a\u0000b'); const c = await sv(`chrome-tab:${'A'.repeat(600)}`);
    const f = await ctx.call('browser_form', { operation: 'fill', fill: { cdp_target_id: '  ', fields: [{ selector: '#z', type: 'text', text: 'x' }] } });
    const pass = a.code === 'TOOL_PARAMS_INVALID' && /must not be empty/.test(a.message) && b.code === 'TOOL_PARAMS_INVALID' && /must not contain NUL/.test(b.message)
      && c.code === 'TOOL_PARAMS_INVALID' && /at most 512/.test(c.message) && c.bytes < 3000 && f.code === 'TOOL_PARAMS_INVALID';
    return result(pass, `'': ${clip(a.message, 60)} | NUL: ${clip(b.message, 50)} | 600 chars: ${clip(c.message, 60)} (${c.bytes} bytes) | fill '  ': ${f.code}`);
  },

  // act refusals echo at most 256 characters of a caller value
  248: async (ctx) => {
    const o = await ctx.call('act', { action: { verb: 'v'.repeat(100000) } });
    const m = await ctx.call('act', { action: { verb: 'click', x: 1, y: 1, modifiers: ['z'.repeat(10000)] } });
    return result(!o.ok && /\(100000 characters\)/.test(o.message) && o.bytes < 4000 && m.bytes < 4000, `verb 100000 chars: ${o.code}, ${o.bytes} bytes, ${clip(o.message.slice(-60), 70)} | modifier 10000 chars: ${m.code}, ${m.bytes} bytes`);
  },

  // cdp_debug=true puts no Chromium flags on a non-Chromium launch
  249: async (ctx) => {
    const t0 = Date.now();
    const o = await ctx.call('process', { operation: 'launch', target: 'C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe', args: ['-NoProfile', '-Command', 'Start-Sleep 120'], windows_console_window_state: 'hidden', cdp_debug: true });
    const ms = Date.now() - t0; ctx.track(proc(o));
    const line = String(dig(o.json, 'command_line') ?? ''); const port = dig(o.json, 'cdp_debug_port');
    return result(o.ok && ms < 8000 && port == null && !line.includes('--remote-debugging-port'), `${o.ok ? `pid ${proc(o)} in ${ms} ms, cdp_debug_port ${port}, command_line ${clip(line, 120)}` : `${said(o)} after ${ms} ms`}`);
  },

  // browser tab tools check an explicit window_hwnd first (dead, non-browser, browser without tab; HTTP session)
  250: async (ctx, n) => {
    const w = await ctx.form(n);
    const c = await ctx.chrome(n);
    const s = await ctx.session();
    const shots = [await ctx.shot(n, w.hwnd, 'before')];
    if (c) shots.push(await ctx.shot(n, c.hwnd, 'chrome-before'));
    const dead = await s.call('browser_storage', { operation: 'get', window_hwnd: 4294967295 });
    const form = await s.call('browser_dom', { operation: 'content', content: { window_hwnd: w.hwnd } });
    const tabless = c ? await s.call('browser_dom', { operation: 'content', content: { window_hwnd: c.hwnd } }) : null;
    shots.push(await ctx.shot(n, w.hwnd, 'after'));
    if (c) shots.push(await ctx.shot(n, c.hwnd, 'chrome-after'));
    const pass = dead.code === 'TARGET_WINDOW_NOT_FOUND' && form.code === 'ACTION_TARGET_INVALID' && /requires a Chromium browser window/.test(form.message)
      && (!tabless || (tabless.code === 'TARGET_NOT_SET' && /does not select a tab/.test(tabless.message)));
    return result(pass, `dead: ${dead.code} ${clip(dead.message, 80)} | form: ${form.code} ${clip(form.message, 100)} | chrome no tab: ${tabless ? `${tabless.code} ${clip(tabless.message, 100)}` : 'no Chrome'}; ${shots.join(' ')}`);
  },

  // chrome-tab id plus a non-browser hwnd: one typed refusal before any bridge call (HTTP session)
  251: async (ctx, n) => {
    const w = await ctx.form(n);
    const s = await ctx.session();
    const shots = [await ctx.shot(n, w.hwnd, 'before')];
    const dom = await s.call('browser_dom', { operation: 'content', content: { cdp_target_id: 'chrome-tab:abc', window_hwnd: w.hwnd } });
    const tabs = await s.call('browser_tabs', { operation: 'select', cdp_target_id: 'chrome-tab:abc', window_hwnd: w.hwnd });
    shots.push(await ctx.shot(n, w.hwnd, 'after'));
    const good = (o) => o.code === 'ACTION_TARGET_INVALID' && /requires a Chromium browser window/.test(o.message) && o.bytes < 2000;
    return result(good(dom) && good(tabs), `browser_dom: ${dom.code} (${dom.bytes} bytes) ${clip(dom.message, 110)} | browser_tabs: ${tabs.code} (${tabs.bytes} bytes) ${clip(tabs.message, 110)}; ${shots.join(' ')}`);
  },

  // browser_tabs refuses a field the operation does not take before its format (HTTP session)
  252: async (ctx) => {
    const s = await ctx.session();
    const a = await s.call('browser_tabs', { operation: 'list', url: 'x' });
    const b = await s.call('browser_tabs', { operation: 'select', url: 'x', cdp_target_id: 'chrome-tab:1' });
    return result(a.code === 'TOOL_PARAMS_INVALID' && /does not accept url/.test(a.message) && b.code === 'TOOL_PARAMS_INVALID' && /does not accept url/.test(b.message), `list: ${said(a)} | select: ${said(b)}`);
  },

  // workspace get on an expired key is WORKSPACE_KEY_ABSENT
  253: async (ctx) => {
    const key = `${TAG}-ttl1`;
    const p = await ctx.call('workspace', { operation: 'put', put: { key, value: 1, ttl_ms: 1 } });
    await sleep(50);
    const g = await ctx.call('workspace', { operation: 'get', get: { key } });
    return result(p.ok && !g.ok && g.code === 'WORKSPACE_KEY_ABSENT', `put ${p.ok ? 'ok' : said(p)}; get: ${said(g)} [detail ${dig(g.json, 'detail_code')}]`);
  },

  // duplicate task create carries AGENT_TASK_ALREADY_EXISTS; one of 15 parallel creates wins
  254: async (ctx) => {
    const id = `${TAG}-t254`;
    const a = await templateAndTask(ctx, id);
    const b = await ctx.call('task', { operation: 'create', create: { task_id: id, title: 'dup', template_id: `${TAG}-tpl` } });
    const race = await Promise.all(Array.from({ length: 15 }, () => ctx.call('task', { operation: 'create', create: { task_id: `${id}-race`, title: 'race', template_id: `${TAG}-tpl` } })));
    const wins = race.filter((o) => o.ok).length; const conflicts = race.filter((o) => /AGENT_TASK_ALREADY_EXISTS/.test(o.raw)).length;
    return result(a.ok && /AGENT_TASK_ALREADY_EXISTS/.test(b.raw) && wins === 1 && conflicts === 14, `duplicate: ${b.code} detail ${dig(b.json, 'detail_code')}; 15 parallel creates: ${wins} created, ${conflicts} AGENT_TASK_ALREADY_EXISTS`);
  },

  // an output path whose parent is a file is a parameter error
  256: async (ctx, n) => {
    const w = await ctx.form(n);
    const shots = [await ctx.shot(n, w.hwnd, 'before')];
    const file = path.join(RUN_DIR, `exist-${STAMP}.png`); fs.writeFileSync(file, 'not a directory');
    const o = await ctx.call('screenshot', { operation: 'capture', window_hwnd: w.hwnd, path: path.join(file, 'child0.png') });
    shots.push(await ctx.shot(n, w.hwnd, 'after'));
    return result(!o.ok && o.code === 'TOOL_PARAMS_INVALID' && /output path parent is not a directory/.test(o.message), `${said(o)}; ${shots.join(' ')}`);
  },

  // a request stream that ends without its reply ends with a JSON-RPC error
  258: async (ctx) => {
    const http = await ctx.httpServer(); const s = await http.session();
    // The issue used agent wait; workspace wait is the same long in-flight call without a write grant.
    const pending = s.post({ jsonrpc: '2.0', id: 11, method: 'tools/call', params: { name: 'workspace', arguments: { operation: 'wait', wait: { key: `${TAG}-never`, timeout_ms: 8000 } } } }, 30000);
    await sleep(1000);
    const d = await s.del();
    const r = await pending;
    const m = r.msgs.find((x) => x.id === 11);
    const reason = m?.error?.data?.reason;
    return result(r.status === 200 && !!m && (m.result !== undefined || (m.error?.code === -32603 && reason === 'response_stream_ended_without_reply')), `DELETE HTTP ${d.status}; in-flight call: HTTP ${r.status}, ${m ? (m.error ? `error ${m.error.code} reason ${reason}` : 'result') : `no message (${clip(r.text, 60) || 'empty stream'})`}`);
  },

  // a terminated adopt owner is the adopt tool's own refusal (3 HTTP sessions, one test window).
  // Adoption (target_claims.rs ensure_same_agent_adoption_allowed) needs both sessions live, the
  // same known agent_kind (inferred from clientInfo.name: "claude" -> claude), the same
  // client_name, and the adopter started later than the owner. Two adopters then race for one
  // owner: the winner terminates it; per 6b21a52b the loser gets TARGET_CLAIM_ADOPT_REFUSED
  // "owner_session_id was already terminated" with requester_session_id = its own session, not
  // HTTP_SESSION_INVALID naming the owner as if it were the caller.
  259: async (ctx, n) => {
    const CLIENT = 'claude-code'; // agent_kind claude for every session of this check
    const w = await ctx.form(n);
    const shots = [await ctx.shot(n, w.hwnd, 'before')];
    const target = { kind: 'window', window_hwnd: w.hwnd };
    const a = await ctx.session(CLIENT);
    await sleep(50); // adopters must start strictly later than the owner (ms clock)
    const [b1, b2] = await Promise.all([ctx.session(CLIENT), ctx.session(CLIENT)]);
    const claim = await a.call('target', { operation: 'claim', target, ttl_ms: 60000 });
    if (!claim.ok) return result(false, `claim: ${said(claim)}; ${shots.join(' ')}`);
    const res = await Promise.all([b1, b2].map((b) => b.call('target', { operation: 'adopt', owner_session_id: a.sid, target, ttl_ms: 60000 })));
    shots.push(await ctx.shot(n, w.hwnd, 'after'));
    const won = res.filter((o) => o.ok).length;
    const li = res.findIndex((o) => !o.ok); const lost = res[li]; const loserSid = [b1, b2][li]?.sid;
    const reqSid = lost ? dig(lost.json, 'requester_session_id') : undefined;
    const pass = won === 1 && lost?.code === 'TARGET_CLAIM_ADOPT_REFUSED' && /owner_session_id was already terminated/.test(lost.message) && reqSid === loserSid;
    return result(pass, `claim ok; adopts won ${won}; loser: ${lost ? `${said(lost)} [requester_session_id ${reqSid === loserSid ? 'is the loser' : JSON.stringify(reqSid)}, owner ${dig(lost.json, 'owner_session_id') === a.sid ? 'is A' : 'other'}]` : 'none'}; ${shots.join(' ')}`);
  },

  // unknown-field errors on the session facade list its accepted fields
  260: async (ctx) => {
    const a = await ctx.call('session', { operation: 'list', bogus: 1 });
    const b = await ctx.call('session', { operation: 'list', 'a`b`c': true });
    const acc = dig(a.json, 'accepted_fields'); const uf = dig(b.json, 'unknown_field'); const accB = dig(b.json, 'accepted_fields');
    return result(Array.isArray(acc) && acc.includes('include_closed') && uf === 'a`b`c' && Array.isArray(accB) && accB.includes('include_closed') && !accB.includes('c'), `bogus: accepted_fields [${(acc || []).join(',')}] | a\`b\`c: unknown_field ${JSON.stringify(uf)}, accepted ${(accB || []).length}`);
  },

  // act invoke ttl_ms refusal names foreground and lease_acquire
  263: async (ctx) => {
    const o = await ctx.call('act', { operation: 'invoke', action: { verb: 'press', key: 'Shift' }, ttl_ms: 1000 });
    return result(!o.ok && /ttl_ms is only valid with foreground or lease_acquire/.test(o.message), said(o));
  },

  // a request with an invalid or repeated id is refused, not dropped as a notification
  264: async (ctx) => {
    const http = await ctx.httpServer(); const s = await http.session();
    const tail = '"method":"tools/call","params":{"name":"health","arguments":{}}}';
    const cases = [['{}', `{"jsonrpc":"2.0","id":{},${tail}`], ['true', `{"jsonrpc":"2.0","id":true,${tail}`], ['1.5', `{"jsonrpc":"2.0","id":1.5,${tail}`], ['[1]', `{"jsonrpc":"2.0","id":[1],${tail}`], ['10+11', `{"jsonrpc":"2.0","id":10,"id":11,${tail}`]];
    const seen = []; let pass = true;
    for (const [name, body] of cases) {
      const r = await s.post(body); const e = rpcErr(r);
      const ok = r.status === 400 && e?.code === -32600; pass &&= ok; seen.push(`id ${name}: ${r.status}/${e?.code ?? (clip(r.text, 20) || 'empty')}`);
    }
    await s.del();
    return result(pass, seen.join('; '));
  },

  // a case-variant job id is a typed not-found
  265: async (ctx) => {
    const id = `${TAG}-case`;
    const s = await shellStart(ctx, { command: 'cmd.exe', args: ['/c', 'echo x'], timeout_ms: 10000, job_id: id });
    const st = await ctx.call('shell', { operation: 'status', job_id: id.toUpperCase() });
    const c = await ctx.call('shell', { operation: 'cancel', job_id: id.toUpperCase() });
    const good = (o) => !o.ok && o.code === 'TOOL_PARAMS_INVALID' && /case-sensitive/.test(o.message);
    return result(s.ok && good(st) && good(c), `${s.ok ? '' : `start: ${said(s)}; `}status: ${said(st)} | cancel: ${c.code}`);
  },

  // an /mcp body over 1 MiB gets a JSON-RPC 413 that states the limit
  266: async (ctx) => {
    const http = await ctx.httpServer(); const s = await http.session();
    const r = await s.post({ jsonrpc: '2.0', id: 9, method: 'tools/call', params: { name: 'shell', arguments: { command: 'cmd.exe', args: ['/c', 'echo', 'A'.repeat(1048576)] } } });
    await s.del();
    const e = rpcErr(r);
    return result(r.status === 413 && r.ctype.includes('application/json') && e?.code === -32600 && /1048576/.test(e?.message ?? ''), `HTTP ${r.status} ${r.ctype}; ${e ? `${e.code} ${clip(e.message, 120)}` : clip(r.text, 80)}`);
  },

  // browser_tabs description states the passive discovery the code performs
  267: async (ctx) => {
    const d = (await ctx.tool('browser_tabs'))?.description ?? '';
    const m = d.match(/With no window_hwnd and no session target[^]*?refuses to guess\.?/);
    return result(!!m && /passively uses the single visible Chromium window/.test(d), clip(m ? m[0] : `not found in: ${d}`, 300));
  },

  // published limit schemas carry the runtime bounds
  270: async (ctx) => {
    const props = [['hygiene', ['flags', 'limit'], 1, 1000], ['hygiene', ['report', 'limit'], 1, 1000], ['hygiene', ['scan_storage', 'limit_rows'], 1, 10000], ['hygiene', ['scan_text', 'min_score'], 0, 100], ['session', ['limit'], 1, 500]];
    const seen = []; let pass = true;
    for (const [tool, p, min, max] of props) {
      const s = await ctx.prop(tool, ...p);
      const ok = s?.minimum === min && s?.maximum === max; pass &&= ok;
      seen.push(`${tool}.${p.join('.')} ${s?.minimum}..${s?.maximum}${ok ? '' : `!=${min}..${max}`}`);
    }
    return result(pass, seen.join('; '));
  },

  // process limit schema names the per-operation bounds
  271: async (ctx) => {
    const p = await ctx.prop('process', 'limit');
    const d = p?.description ?? '';
    return result(/1\.\.=200/.test(d) && /1\.\.=1000/.test(d), `limit ${p?.minimum}..${p?.maximum}: ${clip(d, 200)}`);
  },

  // browser_fill_form target refusals name browser_fill_form
  272: async (ctx) => {
    const o = await ctx.call('browser_form', { operation: 'fill', fill: { fields: [{ selector: '#z', type: 'text', text: 'x' }] } });
    return result(!o.ok && /browser_fill_form/.test(o.message) && !/browser_set_value/.test(o.message), said(o));
  },

  // live-window refusals name the calling tool (HTTP session)
  273: async (ctx) => {
    const s = await ctx.session();
    const o = await s.call('browser_tabs', { operation: 'list', window_hwnd: 1 });
    return result(o.code === 'TARGET_WINDOW_NOT_FOUND' && /^browser_tabs window_hwnd 0x1 is not a live window/.test(o.message), said(o));
  },

  // workspace keys are stored as sent; surrounding whitespace is refused
  274: async (ctx) => {
    const put = (key) => ctx.call('workspace', { operation: 'put', put: { key, value: 'v' } });
    const a = await put(`${TAG}-trim `); const b = await put(` ${TAG}-lead`); const c = await put(`${TAG}-nb\u00a0`);
    const d = await put(`${TAG}-trim`);
    const e = await ctx.call('workspace', { operation: 'exists', exists: { key: `${TAG}-trim\u3000` } });
    const existsTrue = e.ok && dig(e.json, 'exists') === true;
    const pass = [a, b, c].every((o) => o.code === 'TOOL_PARAMS_INVALID') && d.ok && !existsTrue;
    return result(pass, `trailing: ${a.code} | leading: ${b.code} | NBSP: ${c.code} | plain put ${d.ok ? 'ok' : d.code} | exists 'key\\u3000': ${e.ok ? `exists=${dig(e.json, 'exists')}` : e.code}`);
  },

  // DELETE: live 202 (or 200), terminated again 200, unknown id 404
  277: async (ctx) => {
    const http = await ctx.httpServer(); const s = await http.session();
    const first = await s.del(); const second = await s.del();
    const unknown = await new HttpSession(http, crypto.randomUUID()).del();
    return result([200, 202].includes(first.status) && second.status === 200 && unknown.status === 404, `live DELETE ${first.status}; again ${second.status}; unknown id ${unknown.status}`);
  },

  // session teardown waits for owned processes off the async workers: other calls stay fast
  278: async (ctx) => {
    const http = await ctx.httpServer();
    const s1 = await http.session(); const s2 = await http.session();
    const pids = [];
    for (let i = 0; i < 2; i += 1) {
      const o = await s1.call('process', { operation: 'launch', target: 'powershell.exe', args: ['-NoProfile', '-Command', 'Start-Sleep 60'], windows_console_window_state: 'hidden' });
      const pid = proc(o); if (pid) { pids.push(pid); ctx.track(pid); }
    }
    let stop = false; const lat = [];
    const loop = (async () => { while (!stop) { const t = Date.now(); await s2.call('health', {}); lat.push(Date.now() - t); await sleep(150); } })();
    const t0 = Date.now(); const d = await s1.del(); const delMs = Date.now() - t0;
    await sleep(1000); stop = true; await loop; await s2.del();
    let alive = pids.filter(pidAlive);
    for (let i = 0; i < 20 && alive.length; i += 1) { await sleep(500); alive = pids.filter(pidAlive); }
    const max = Math.max(0, ...lat);
    return result(pids.length === 2 && [200, 202].includes(d.status) && max < 2000 && alive.length === 0, `owned pids [${pids.join(',')}]; DELETE ${d.status} in ${delMs} ms; ${lat.length} health calls meanwhile, slowest ${max} ms; still alive [${alive.join(',')}]`);
  },

  // shell cancel terminates in well under the old 5 s
  283: async (ctx) => {
    const jobId = `${TAG}-283`;
    const s = await shellStart(ctx, { command: 'ping.exe', args: ['-n', '16', `127.0.0.${2 + crypto.randomInt(250)}`], timeout_ms: 60000, job_id: jobId });
    if (!s.ok) return result(false, `start: ${said(s)}`);
    await sleep(2000);
    const t0 = Date.now(); const c = await ctx.call('shell', { operation: 'cancel', job_id: jobId }); const ms = Date.now() - t0;
    const pid = proc(s); const alive = pid ? pidAlive(pid) : false;
    return result(c.ok && ms < 1500 && !alive, `cancel took ${ms} ms; termination_status ${dig(c.json, 'termination_status')}; job pid ${pid} alive after: ${alive}`);
  },

  // new shell job ids refuse Windows reserved device names
  284: async (ctx) => {
    const seen = []; let pass = true;
    for (const id of ['AUX', 'com1', 'LPT9']) {
      const o = await shellStart(ctx, { command: 'cmd.exe', args: ['/c', 'echo', 'x'], timeout_ms: 5000, job_id: id });
      const reason = dig(o.json, 'reason'); const ok = !o.ok && o.code === 'TOOL_PARAMS_INVALID' && reason === 'job_id_reserved_device_name';
      pass &&= ok; seen.push(`${id}: ${o.ok ? 'created' : `${o.code}/${reason}`}`);
    }
    return result(pass, seen.join('; '));
  },

  // cleanup without behaviour change: lease owner still printed as a plain id or "none"
  // (e85a5697 lease::owner_label). Two HTTP sessions: A holds the lease; B reads status, is refused
  // acquire (held by session <A>) and release (current holder <A>); after A releases, B's release
  // names holder none. No answer may carry Rust's "Some(".
  289: async (ctx) => {
    const a = await ctx.session(); const b = await ctx.session();
    const lease = (s, op, extra = {}) => s.call('act', { operation: op, ...extra });
    const acq = await lease(a, 'lease_acquire', { ttl_ms: 10000 });
    if (!acq.ok) return result(false, `A lease_acquire: ${said(acq)}`);
    let st; let busy; let rel;
    try {
      st = await lease(b, 'lease_status');
      busy = await lease(b, 'lease_acquire', { ttl_ms: 10000 });
      rel = await lease(b, 'lease_release');
    } finally { await lease(a, 'lease_release'); }
    const relNone = await lease(b, 'lease_release');
    const all = [acq, st, busy, rel, relNone];
    const some = all.some((o) => /Some\(/.test(o.raw));
    // A non-holder's release is an idempotent ok (outcome not_held) carrying owner_session_id
    // (id or null); a refusal that names a holder must name the right one (id or "none").
    const names = (o, sid) => (o.ok ? dig(o.json, 'owner_session_id') === sid : new RegExp(`(held by session|current holder) "?${sid ?? 'none'}\\b`).test(o.message));
    const owner = dig(st.json, 'owner_session_id');
    const pass = !some && st.ok && owner === a.sid && !busy.ok && busy.message.includes(`held by session ${a.sid}`) && names(rel, a.sid) && names(relNone, null);
    const anon = (t) => String(t).replaceAll(a.sid, '<A>').replaceAll(b.sid, '<B>');
    const show = (o) => (o.ok ? `ok ${dig(o.json, 'outcome')} owner ${anon(JSON.stringify(dig(o.json, 'owner_session_id')))}` : `${o.code}: ${clip(anon(o.message), 110)}`);
    return result(pass, `B status: owner ${owner === a.sid ? '<A>' : JSON.stringify(owner)} | B acquire: ${show(busy)} | B release while A holds: ${show(rel)} | B release after A released: ${show(relNone)}; "Some(" in any answer: ${some}`);
  },

  // maintenance denials name the tool's read-only operations (8228ff5e). HTTP session with the
  // default read-only grants and profile normal_agent: require_maintenance_profile refuses before
  // any permission check, with remediation "...; normal_agent may use <read-only ops> first".
  // (The stdio server is unscoped break_glass, so it never denies.) Payloads are valid ones.
  311: async (ctx) => {
    const s = await ctx.session();
    const prof = async () => String(dig((await s.call('profile', { operation: 'status' })).json, 'profile') ?? '?');
    let profile = await prof();
    if (profile !== 'normal_agent') {
      const set = await s.call('profile', { operation: 'set', profile: 'normal_agent', reason: 'live check #311' });
      if (!set.ok) return result(false, `profile ${profile}; set normal_agent: ${said(set)}`);
      profile = await prof();
    }
    const cases = [
      ['storage gc_once', await s.call('storage', { operation: 'gc_once', gc_once: { cf_name: 'CF_EVENTS', soft_cap_rows: 1000000, hard_cap_rows: 1000000 } }), /normal_agent may use inspect or summary first/],
      ['model register', await s.call('model', { operation: 'register', register: { name: `${TAG}-m`, base_url: 'http://127.0.0.1:9', model_id: 'x' } }), /normal_agent may use list or status first/],
      ['hygiene scan_storage', await s.call('hygiene', { operation: 'scan_storage', scan_storage: {} }), /normal_agent may use scan_text without persist, flags or report first/],
    ];
    const rem = (o) => String(dig(o.json, 'remediation') ?? '');
    let pass = profile === 'normal_agent'; const seen = [];
    for (const [name, o, want] of cases) {
      const ok = !o.ok && o.code === 'TOOL_PROFILE_POLICY_DENIED' && want.test(rem(o));
      pass &&= ok;
      seen.push(`${name}: ${o.ok ? 'accepted' : `${o.code} remediation ...${clip(rem(o).match(/; normal_agent[^"]*/)?.[0] ?? (rem(o) || o.message), 90)}`}${ok ? '' : '!'}`);
    }
    return result(pass, `profile ${profile} | ${seen.join(' | ')}`);
  },

  // delivery fact of an error raised after delivered input: needs a read-back that fails after input
  312: async () => skip('no live trigger: needs an action whose read-back fails after its input was delivered (fault injection), which the real app does not provide on demand'),
  313: async () => skip('no live trigger: needs a CDP or bridge sequence that fails after its first acknowledged event (fault injection), which the real app does not provide on demand'),

  // privacy denials name break_glass / full_capability through the shared gate
  316: async (ctx) => {
    const o = await ctx.call('privacy', { operation: 'pause', pause: {} });
    const rem = String(dig(o.json, 'remediation') ?? '');
    return result(o.code === 'TOOL_PROFILE_POLICY_DENIED' && /^privacy operation=pause is not allowed for profile/.test(o.message) && /break_glass/.test(rem) && !/privacy\/admin/.test(o.raw), `${said(o)}; remediation: ${clip(rem, 120)}`);
  },
};

// ---------------------------------------------------------------- run

const ISSUES = [144, 146, 148, 162, 163, 174, 180, 181, 183, 184, 186, 189, 191, 193, 194, 195, 199, 200, 206, 207, 209, 210, 211, 212, 218, 219, 220, 221, 222, 223, 224, 226, 227, 229, 235, 236, 243, 245, 248, 249, 250, 251, 252, 253, 254, 256, 258, 259, 260, 263, 264, 265, 266, 267, 270, 271, 272, 273, 274, 277, 278, 283, 284, 289, 311, 312, 313, 316];

async function main() {
  if (!fs.existsSync(EXE)) { console.error(`missing ${EXE}`); process.exit(2); }
  fs.appendFileSync(RESULTS, `# ${new Date().toISOString()} run=${argv.run} invocation=${STAMP}${ONLY ? ` only=${[...ONLY].join(',')}` : ''}\n`);
  const cwd = path.join(RUN_DIR, `stdio-cwd-${STAMP}`); fs.mkdirSync(cwd, { recursive: true });
  const srv = new McpServer({
    exe: EXE, db: path.join(RUN_DIR, `db-stdio-${STAMP}`), outDir: RUN_DIR, tag: `stdio-${STAMP}`, cwd, args: ['--mode', 'stdio'],
    // Read grants plus WRITE_STORAGE, so storage-writing checks reach the behaviour under test.
    env: { SYNAPSE_SHELL_JOB_ROOT: path.join(RUN_DIR, `shell-jobs-stdio-${STAMP}`), SYNAPSE_MCP_ALLOWED_PERMISSIONS: 'READ_EVENTS,READ_REFLEX,READ_PROFILE,READ_STORAGE,WRITE_STORAGE' },
  });
  const ctx = new Ctx(srv);
  // Killing the HTTP server ends its sessions; windows and processes are closed here.
  const onSignal = () => { ctx.sessions.length = 0; ctx.cleanup(); ctx.http?.stop(); srv.kill(); process.exit(130); };
  process.on('SIGINT', onSignal);
  let counts = { PASS: 0, FAIL: 0, SKIP: 0 };
  try {
    await srv.init('synapse-live-check');
    for (const n of ISSUES) {
      if (ONLY && !ONLY.has(n)) continue;
      let r;
      try { r = await CHECKS[n](ctx, n); } catch (e) { r = { status: 'FAIL', observed: `check error: ${e.message}` }; } finally { await ctx.cleanup(); }
      const line = `#${n} ${r.status} ${clip(r.observed)}`;
      counts[r.status] += 1;
      console.log(line);
      fs.appendFileSync(RESULTS, `${line}\n`);
    }
  } finally {
    await ctx.cleanup();
    ctx.http?.stop();
    await srv.close();
  }
  const sum = `# done: ${counts.PASS} PASS, ${counts.FAIL} FAIL, ${counts.SKIP} SKIP`;
  console.log(sum); fs.appendFileSync(RESULTS, `${sum}\n`);
  process.exit(0);
}

main().catch((e) => { console.error(`driver error: ${e.stack || e.message}`); process.exit(1); });
