// Clip recorder live test driver, round 2.
// Usage: node clipdrive3.mjs <exe> <db> <out_dir> <json_options>
// options: { hwnd, duration_ms, resize: [{at_ms, w, h}], stop_at_ms, kill_server_at_ms,
//            restart_after_kill: bool, overview: bool, max_wait_ms }
// Resizes use SetWindowPos on the test window only (no input to any app).
import fs from 'node:fs';
import path from 'node:path';
import { McpServer, User32, clipStatus as statusOf, sleep } from './mcp-stdio.mjs';

const [exe, db, outDir, optJson] = process.argv.slice(2);
const opt = JSON.parse(optJson);
fs.mkdirSync(outDir, { recursive: true });
const log = [];
const L = (k, v) => { log.push({ t: new Date().toISOString(), k, v }); fs.writeFileSync(path.join(outDir, 'log.json'), JSON.stringify(log, null, 1)); };
const newServer = (tag) => new McpServer({ exe, db, outDir, tag, logLevel: 'info' });

const resizeWindow = (hwnd, w, h) => User32.setWindowPos(hwnd, -1, 0, 0, w, h, 0x0012);

const waitingStats = folder => {
  try { const dir = path.join(folder, 'waiting'); const fl = fs.readdirSync(dir); let bytes = 0; for (const f of fl) bytes += fs.statSync(path.join(dir, f)).size; return { files: fl.length, bytes }; }
  catch (e) { return { error: String(e.code || e) }; }
};

let srv = newServer('a');
try {
  L('init', (await srv.init('clip-driver')).result?.serverInfo);
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
      srv = newServer('b'); L('restart_init', (await srv.init('clip-driver')).result?.serverInfo);
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
