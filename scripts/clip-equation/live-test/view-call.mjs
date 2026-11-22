// Calls one viewer operation of the screenshot tool on a clip folder and saves
// the text and images. Usage: node clipview.mjs <exe> <db> <out_dir> <json_args>
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

const [exe, db, outDir, jsonArgs] = process.argv.slice(2);
fs.mkdirSync(outDir, { recursive: true });
const p = spawn(exe, ['--db', db, '--log-level', 'warn'], { stdio: ['pipe', 'pipe', 'pipe'] });
p.stderr.pipe(fs.createWriteStream(path.join(outDir, 'stderr.log')));
let buf = '', id = 0; const wait = new Map();
p.stdout.on('data', d => { buf += d; let i; while ((i = buf.indexOf('\n')) >= 0) { const line = buf.slice(0, i); buf = buf.slice(i + 1); if (!line.trim()) continue; let m; try { m = JSON.parse(line); } catch { continue; } if (m.id != null && wait.has(m.id)) { wait.get(m.id)(m); wait.delete(m.id); } } });
p.on('exit', code => { for (const w of wait.values()) w({ error: { message: `server exited ${code}` } }); });
const rpc = (method, params) => new Promise(res => { const i = ++id; wait.set(i, res); p.stdin.write(JSON.stringify({ jsonrpc: '2.0', id: i, method, params }) + '\n'); });
await rpc('initialize', { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'clip-view', version: '0' } });
p.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized', params: {} }) + '\n');
const args = JSON.parse(jsonArgs);
const r = await rpc('tools/call', { name: 'screenshot', arguments: args });
const texts = []; let img = 0;
for (const x of r.result?.content || []) {
  if (x.type === 'image') { const f = path.join(outDir, `${args.operation}-${++img}.png`); fs.writeFileSync(f, Buffer.from(x.data, 'base64')); texts.push(`[image ${f}]`); }
  else if (x.type === 'text') texts.push(x.text);
}
fs.writeFileSync(path.join(outDir, `${args.operation}.txt`), (r.error ? 'RPC ERROR ' + JSON.stringify(r.error) + '\n' : '') + texts.join('\n\n'));
console.log(`${args.operation}: isError=${r.result?.isError ?? false} error=${r.error ? JSON.stringify(r.error) : 'none'} images=${img} chars=${texts.join('').length}`);
p.stdin.end(); setTimeout(() => { p.kill(); process.exit(0); }, 1500);
