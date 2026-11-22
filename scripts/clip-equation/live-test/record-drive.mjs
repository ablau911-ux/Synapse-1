// Clip recorder live test driver, round 2.
// Usage: node clipdrive3.mjs <exe> <db> <out_dir> <json_options>
// options: { hwnd, duration_ms, resize: [{at_ms, w, h}], stop_at_ms, kill_server_at_ms,
//            restart_after_kill: bool, overview: bool, max_wait_ms }
// Resizes use SetWindowPos on the test window only (no input to any app).
import { spawn, execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

const [exe, db, outDir, optJson] = process.argv.slice(2);
const opt = JSON.parse(optJson);
fs.mkdirSync(outDir, { recursive: true });
const log = [];
const L = (k, v) => { log.push({ t: new Date().toISOString(), k, v }); fs.writeFileSync(path.join(outDir, 'log.json'), JSON.stringify(log, null, 1)); };
const sleep = ms => new Promise(r => setTimeout(r, ms));

class Server {
  constructor(tag) {
    this.p = spawn(exe, ['--db', db, '--log-level', 'info'], { stdio: ['pipe', 'pipe', 'pipe'] });
    this.p.stderr.pipe(fs.createWriteStream(path.join(outDir, `stderr-${tag}.log`)));
    this.buf = ''; this.id = 0; this.wait = new Map(); this.img = 0; this.tag = tag;
    this.p.stdout.on('data', d => {
      this.buf += d; let i;
      while ((i = this.buf.indexOf('\n')) >= 0) {
        const line = this.buf.slice(0, i); this.buf = this.buf.slice(i + 1);
        if (!line.trim()) continue; let m; try { m = JSON.parse(line); } catch { continue; }
        if (m.id != null && this.wait.has(m.id)) { this.wait.get(m.id)(m); this.wait.delete(m.id); }
      }
    });
    this.p.on('exit', code => { for (const w of this.wait.values()) w({ error: { message: `server exited ${code}` } }); this.wait.clear(); });
  }
  rpc(method, params) { return new Promise(res => { const i = ++this.id; this.wait.set(i, res); this.p.stdin.write(JSON.stringify({ jsonrpc: '2.0', id: i, method, params }) + '\n'); }); }
  async init() { const r = await this.rpc('initialize', { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'clip-driver', version: '0' } }); this.p.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized', params: {} }) + '\n'); return r; }
  async call(a) {
    const r = await this.rpc('tools/call', { name: 'screenshot', arguments: a });
    const texts = [];
    for (const x of r.result?.content || []) {
      if (x.type === 'image') { const f = path.join(outDir, `${this.tag}-${a.operation}-${++this.img}.png`); fs.writeFileSync(f, Buffer.from(x.data, 'base64')); texts.push(`[image ${f}]`); }
      else if (x.type === 'text') texts.push(x.text);
    }
    let json = null; try { json = JSON.parse(texts[0]); } catch { }
    return { error: r.error, isError: r.result?.isError, texts, json };
  }
  kill() { try { execFileSync('taskkill', ['/PID', String(this.p.pid), '/F']); } catch { } }
  async close() { this.p.stdin.end(); await sleep(2000); this.kill(); }
}

const resizeWindow = (hwnd, w, h) => execFileSync('powershell', ['-NoProfile', '-Command',
  `Add-Type -Namespace W -Name U -MemberDefinition '[DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr a, int x, int y, int cx, int cy, uint f);'; [W.U]::SetWindowPos([IntPtr]${hwnd}, [IntPtr](-1), 0, 0, ${w}, ${h}, 0x0012) | Out-Null`]);

const waitingStats = folder => {
  try { const dir = path.join(folder, 'waiting'); const fl = fs.readdirSync(dir); let bytes = 0; for (const f of fl) bytes += fs.statSync(path.join(dir, f)).size; return { files: fl.length, bytes }; }
  catch (e) { return { error: String(e.code || e) }; }
};
const statusOf = async (srv, cid) => {
  const st = await srv.call({ operation: 'record_status', clip_id: cid });
  const r = st.json?.record_status || st.json?.record || st.json || {};
  return { st, state: r.state, seconds: r.seconds_recorded, stop: r.stop_reason, raw: st.texts.join(' ').slice(0, 600) };
};

let srv = new Server('a');
try {
  L('init', (await srv.init()).result?.serverInfo);
  const t0 = Date.now();
  const rec = await srv.call({ operation: 'record', window_hwnd: Number(opt.hwnd), duration_ms: Number(opt.duration_ms) });
  const r = rec.json?.record || {};
  const cid = r.clip_id, folder = r.folder;
  L('record', { ms: Date.now() - t0, cid, folder, isError: rec.isError, text: rec.texts.join(' ').slice(0, 800) });
  if (!cid) throw new Error('no clip_id');
  const timers = [];
  for (const z of opt.resize || []) timers.push(setTimeout(() => { try { resizeWindow(opt.hwnd, z.w, z.h); L('resized', z); } catch (e) { L('resize_error', String(e)); } }, z.at_ms));
  let killed = false, stopped = false, final = null;
  while (Date.now() - t0 < (opt.max_wait_ms || 900000)) {
    await sleep(2000);
    const el = Date.now() - t0;
    if (opt.stop_at_ms && !stopped && el >= opt.stop_at_ms) {
      stopped = true; const s = await srv.call({ operation: 'record_stop', clip_id: cid });
      L('record_stop', s.texts.join(' ').slice(0, 600));
      const s2 = await srv.call({ operation: 'record_stop', clip_id: cid });
      L('record_stop_again', { isError: s2.isError, text: s2.texts.join(' ').slice(0, 300) });
    }
    if (opt.kill_server_at_ms && !killed && el >= opt.kill_server_at_ms) {
      killed = true; L('kill_server', { at_s: el / 1000, waiting: waitingStats(folder) }); srv.kill(); await sleep(3000);
      if (!opt.restart_after_kill) break;
      srv = new Server('b'); L('restart_init', (await srv.init()).result?.serverInfo);
      continue;
    }
    const s = await statusOf(srv, cid);
    L('status', { s: Math.round(el / 1000), state: s.state, seconds: s.seconds, stop: s.stop, waiting: waitingStats(folder), raw: s.state ? undefined : s.raw });
    if (s.state === 'ready' || s.state === 'failed') { final = s; break; }
  }
  timers.forEach(clearTimeout);
  L('total_s', (Date.now() - t0) / 1000);
  if (folder && fs.existsSync(path.join(folder, 'clip.json'))) {
    fs.copyFileSync(path.join(folder, 'clip.json'), path.join(outDir, 'clip.json'));
    const c = JSON.parse(fs.readFileSync(path.join(folder, 'clip.json'), 'utf8'));
    L('clip', { state: c.state, stop_reason: c.stop_reason, recorded_ms: c.recorded_ms, lost: (c.lost || []).length, lost_sample: (c.lost || []).slice(0, 3), segments: (c.segments || []).map(s => ({ i: s.index, bytes: s.bytes, inst: `${s.first_instant}-${s.last_instant}`, ms: `${s.start_ms}-${s.end_ms}`, wh: `${s.width}x${s.height}` })), cond: Object.fromEntries(Object.entries(c.conditions || {}).map(([k, v]) => [k, v.status])), g: c.conditions?.g?.detail, t: c.conditions?.t?.detail, b: c.conditions?.b?.detail ? { max_sum: c.conditions.b.detail.max_sum_percent, live: c.conditions.b.detail.max_live_share_percent, worker: c.conditions.b.detail.max_worker_share_percent } : null });
    L('waiting_after', waitingStats(folder));
    if (opt.overview) {
      const ov = await srv.call({ operation: 'clip_overview', folder: folder.replace(/\\/g, '/') });
      fs.writeFileSync(path.join(outDir, 'overview.txt'), ov.texts.join('\n\n'));
      L('overview', { isError: ov.isError, error: ov.error, head: ov.texts.join('\n').split('\n').filter(l => /key frames|Note:|cut at/.test(l)).join(' | ').slice(0, 900) });
    }
  }
} catch (e) { L('fatal', String(e && e.stack || e)); }
await srv.close(); process.exit(0);
