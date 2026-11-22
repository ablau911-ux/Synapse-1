// Shared by the live test drivers in this folder (README.md): a synapse-mcp build spoken to as a
// separate stdio MCP instance, the clip status read, and window placement through user32 (never
// input to any app).
import { spawn, execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

export const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/** One synapse-mcp process, spoken to with JSON-RPC lines over stdin and stdout. */
export class McpServer {
  /**
   * exe and db: the build and its own --db. outDir: where its stderr and returned images go.
   * tag: prefix of those files ('' for none). env: extra environment, for example its own
   * SYNAPSE_SHELL_JOB_ROOT so it does not collide with the running service.
   */
  constructor({ exe, db, outDir, tag = '', logLevel = 'info', env = {} }) {
    this.outDir = outDir;
    this.prefix = tag ? `${tag}-` : '';
    this.p = spawn(exe, ['--db', db, '--log-level', logLevel], { stdio: ['pipe', 'pipe', 'pipe'], env: { ...process.env, ...env } });
    this.p.stderr.pipe(fs.createWriteStream(path.join(outDir, tag ? `stderr-${tag}.log` : 'stderr.log')));
    this.buf = ''; this.id = 0; this.wait = new Map(); this.img = 0;
    this.p.stdout.on('data', (d) => {
      this.buf += d; let i;
      while ((i = this.buf.indexOf('\n')) >= 0) {
        const line = this.buf.slice(0, i); this.buf = this.buf.slice(i + 1);
        if (!line.trim()) continue; let m; try { m = JSON.parse(line); } catch { continue; }
        if (m.id != null && this.wait.has(m.id)) { this.wait.get(m.id)(m); this.wait.delete(m.id); }
      }
    });
    this.p.on('exit', (code) => { for (const w of this.wait.values()) w({ error: { message: `server exited ${code}` } }); this.wait.clear(); });
  }

  rpc(method, params) {
    return new Promise((res) => { const i = ++this.id; this.wait.set(i, res); this.p.stdin.write(JSON.stringify({ jsonrpc: '2.0', id: i, method, params }) + '\n'); });
  }

  async init(clientName) {
    const r = await this.rpc('initialize', { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: clientName, version: '0' } });
    this.p.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized', params: {} }) + '\n');
    return r;
  }

  /** One `screenshot` operation: its texts (images saved into outDir), the first text as JSON. */
  async call(args) {
    const r = await this.rpc('tools/call', { name: 'screenshot', arguments: args });
    const texts = []; let images = 0;
    for (const x of r.result?.content || []) {
      if (x.type === 'image') {
        const f = path.join(this.outDir, `${this.prefix}${args.operation}-${++this.img}.png`);
        fs.writeFileSync(f, Buffer.from(x.data, 'base64')); texts.push(`[image ${f}]`); images += 1;
      } else if (x.type === 'text') texts.push(x.text);
    }
    let json = null; try { json = JSON.parse(texts[0]); } catch { }
    return { error: r.error, isError: r.result?.isError, texts, json, images };
  }

  kill() { try { execFileSync('taskkill', ['/PID', String(this.p.pid), '/F']); } catch { } }
  async close() { this.p.stdin.end(); await sleep(2000); this.kill(); }
}

/** `record_status` of one clip: its state, seconds recorded and stop reason. */
export async function clipStatus(srv, clipId) {
  const st = await srv.call({ operation: 'record_status', clip_id: clipId });
  const r = st.json?.record_status || st.json?.record || st.json || {};
  return { st, state: r.state, seconds: r.seconds_recorded, stop: r.stop_reason, raw: st.texts.join(' ').slice(0, 600) };
}

const USER32 = '[DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr a, int x, int y, int cx, int cy, uint f);'
  + ' [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int c);'
  + ' [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);';
const HWND_BOTTOM = 1;
const SW_SHOWNOACTIVATE = 4;
const SWP_NOSIZE = 0x0001;
const SWP_NOMOVE = 0x0002;
const SWP_NOACTIVATE = 0x0010;

/** Window placement through user32, one PowerShell call each. Never input, never activation. */
export class User32 {
  static run(statements) {
    return execFileSync('powershell', ['-NoProfile', '-Command', `Add-Type -Namespace W -Name U -MemberDefinition '${USER32}'; ${statements}`], { encoding: 'utf8' }).trim();
  }

  static setWindowPos(hwnd, insertAfter, x, y, cx, cy, flags) {
    User32.run(`[W.U]::SetWindowPos([IntPtr]${hwnd}, [IntPtr](${insertAfter}), ${x}, ${y}, ${cx}, ${cy}, ${flags}) | Out-Null`);
  }

  /** Shows a minimized window in its last size and position without activating it. */
  static restoreWithoutActivating(hwnd) {
    User32.run(`if ([W.U]::IsIconic([IntPtr]${hwnd})) { [W.U]::ShowWindow([IntPtr]${hwnd}, ${SW_SHOWNOACTIVATE}) | Out-Null }`);
  }

  /** Puts a window behind all other windows, keeping its place, size and activation. */
  static sendToBack(hwnd) {
    User32.setWindowPos(hwnd, HWND_BOTTOM, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
  }
}
