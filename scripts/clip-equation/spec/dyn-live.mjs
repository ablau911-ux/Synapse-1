// Local UI/video/restart acceptance. YouTube and the reviewer drivers are untouched.
import fs from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';
import { spawn, spawnSync, execFileSync } from 'node:child_process';
import { McpServer, User32, sleep } from '../live-test/mcp-stdio.mjs';

const [exeArg, outArg, scene = 'ui', durationArg = '20000', killArg = '0'] = process.argv.slice(2);
const exe = path.resolve(exeArg), out = path.resolve(outArg);
const duration = Number(durationArg), killAt = Number(killArg);
const POLL_MS = 100, WINDOW_WAIT_MS = 15000, READY_SLACK_MS = 30000;
const WIDTH = 1280, HEIGHT = 900;
const chrome = ['C:/Program Files/Google/Chrome/Application/chrome.exe', 'C:/Program Files (x86)/Google/Chrome/Application/chrome.exe'].find(fs.existsSync);
if (!chrome) throw Error('Chrome not found');
fs.mkdirSync(out, { recursive: true });
const profile = path.join(out, 'chrome-profile');
const page = path.resolve('scripts/clip-equation', scene === 'ui' ? 'ui-events-page.html' : 'video-page.html');
const browser = spawn(chrome, [`--app=${pathToFileURL(page).href}`, `--user-data-dir=${profile}`,
  `--window-size=${WIDTH},${HEIGHT}`, '--no-first-run', '--no-default-browser-check',
  '--disable-backgrounding-occluded-windows', '--disable-renderer-backgrounding',
  '--disable-background-timer-throttling'], { stdio: 'ignore', windowsHide: true });
let server, folder;
const log = { scene, duration_ms: duration };
const save = () => fs.writeFileSync(path.join(out, 'log.json'), JSON.stringify(log, null, 2));
const newServer = tag => new McpServer({ exe, db: path.join(out, 'db'), outDir: out, tag,
  env: { LOCALAPPDATA: path.join(out, 'local'), SYNAPSE_SHELL_JOB_ROOT: path.join(out, 'shell') } });
try {
  const end = Date.now() + WINDOW_WAIT_MS;
  let hwnd = 0;
  while (!hwnd && Date.now() < end) {
    hwnd = Number(execFileSync('powershell.exe', ['-NoProfile', '-Command',
      `(Get-Process -Id ${browser.pid} -ErrorAction SilentlyContinue).MainWindowHandle`], { encoding: 'utf8' }).trim());
    if (!hwnd) await sleep(POLL_MS);
  }
  if (!hwnd) throw Error('test browser has no window');
  log.hwnd = hwnd; log.browser_pid = browser.pid; save();
  User32.restoreWithoutActivating(hwnd);
  User32.sendToBack(hwnd);
  await sleep(1000);
  server = newServer('record'); await server.init('dyn-live');
  const rec = await server.call({ operation: 'record', window_hwnd: hwnd, duration_ms: duration });
  if (!rec.json?.record?.clip_id) throw Error(rec.texts.join('\n'));
  const { clip_id: clipId } = rec.json.record;
  folder = rec.json.record.folder; log.folder = folder; save();
  const started = Date.now(); let killed = false, ready = false;
  while (Date.now()-started < duration+READY_SLACK_MS) {
    if (killAt && !killed && Date.now()-started >= killAt) {
      killed = true; server.kill(); await server.close();
      server = newServer('recovery'); await server.init('dyn-live-recovery');
    }
    const status = await server.call({ operation: 'record_status', clip_id: clipId });
    log.status_has_frames_waiting ||= JSON.stringify(status.json).includes('frames_waiting');
    const state = status.json?.record_status?.state ?? status.json?.record?.state ?? status.json?.state;
    if (state === 'ready' || state === 'failed') {
      ready = state === 'ready'; log.state = state;
      log.ready_ms = Date.now()-fs.statSync(path.join(folder, 'frames.jsonl')).mtimeMs;
      break;
    }
    await sleep(POLL_MS);
  }
  if (!ready) throw Error(`recorder did not become ready: ${log.state}`);
  if (!killAt) {
    const first = await server.call({ operation: 'record_stop', clip_id: clipId });
    const second = await server.call({ operation: 'record_stop', clip_id: clipId });
    log.stop_twice_equal = !first.isError && !second.isError && JSON.stringify(first.json) === JSON.stringify(second.json);
  }
  const frame = await server.call({ operation: 'clip_frame', folder, t_ms: 1000, scale: 1 });
  log.frame_ok = !frame.isError && !frame.error && frame.images === 1;
  const overview = await server.call({ operation: 'clip_overview', folder });
  log.overview_ok = !overview.isError && !overview.error;
  log.overview_text = overview.texts.filter(t => !t.startsWith('[image'));
  save();
  const check = spawnSync('node', [path.resolve('scripts/clip-equation/spec', killAt ? 'dyn-recovery.mjs' : 'dyn-folder.mjs'), folder, path.join(out, 'log.json')], { encoding: 'utf8' });
  fs.writeFileSync(path.join(out, 'check.txt'), check.stdout+check.stderr);
  console.log(`scene=${scene} duration=${duration} kill=${killAt} ready_ms=${log.ready_ms} frame_ok=${log.frame_ok} overview_ok=${log.overview_ok} check_exit=${check.status}`);
  console.log(check.stdout.split('\n').filter(l => l.startsWith('FAIL') || /failed$/.test(l)).join('\n'));
  process.exitCode = check.status || (!log.frame_ok || !log.overview_ok ? 1 : 0);
} catch (error) {
  log.error = error.stack; save(); console.error(error.message); process.exitCode = 1;
} finally {
  if (server) await server.close();
  spawnSync('taskkill', ['/PID', String(browser.pid), '/T', '/F'], { stdio: 'ignore' });
  try { process.kill(browser.pid, 0); console.error('test browser remains alive'); process.exitCode = 1; }
  catch (error) { if (error.code !== 'ESRCH') { console.error(error.message); process.exitCode = 1; } }
}
