// Calls one viewer operation of the screenshot tool on a clip folder and saves
// the text and images. Usage: node clipview.mjs <exe> <db> <out_dir> <json_args>
import fs from 'node:fs';
import path from 'node:path';
import { McpServer } from './mcp-stdio.mjs';

const [exe, db, outDir, jsonArgs] = process.argv.slice(2);
fs.mkdirSync(outDir, { recursive: true });
const srv = new McpServer({ exe, db, outDir, logLevel: 'warn' });
let code = 0;
try {
  await srv.init('clip-view');
  const args = JSON.parse(jsonArgs);
  const r = await srv.call(args);
  fs.writeFileSync(path.join(outDir, `${args.operation}.txt`), (r.error ? 'RPC ERROR ' + JSON.stringify(r.error) + '\n' : '') + r.texts.join('\n\n'));
  console.log(`${args.operation}: isError=${r.isError ?? false} error=${r.error ? JSON.stringify(r.error) : 'none'} images=${r.images} chars=${r.texts.join('').length}`);
} catch (error) {
  console.log(`view-call failed: ${error?.message ?? error}`);
  code = 1;
}
await srv.close(); process.exit(code);
