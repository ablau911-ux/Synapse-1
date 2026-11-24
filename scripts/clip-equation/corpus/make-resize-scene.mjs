// Builds the corpus scene `resize-640x360-to-506x333` from two existing scenes: a window that changes
// size twice while it is recorded. Instants 0-119 scroll at 640x360, 120-239 typing at 506x333 (it
// settles, so stills appear at the new size), 240-359 scroll at 640x360 again.
//   node scripts/clip-equation/corpus/make-resize-scene.mjs
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const BASE = path.dirname(fileURLToPath(import.meta.url));
const ROOT = JSON.parse(fs.readFileSync(path.join(BASE, 'corpus.json'), 'utf8')).root;
const NAME = 'resize-640x360-to-506x333';
const SEGMENT = 120; // instants per size
const FRAME_OFFSET = 1000; // frame numbers of the second scene, so the two scenes' frames never collide
const PARTS = [
  { scene: 'scroll-640x360', from: 0, offset: 0 },
  { scene: 'typing-506x333', from: 0, offset: FRAME_OFFSET },
  { scene: 'scroll-640x360', from: SEGMENT, offset: 0 },
];

const out = path.join(ROOT, NAME);
fs.rmSync(out, { recursive: true, force: true });
fs.mkdirSync(out, { recursive: true });
const frameFile = (n) => `frame-${String(n).padStart(4, '0')}.zst`;
const lines = [];
for (const part of PARTS) {
  const source = path.join(ROOT, part.scene);
  const rows = fs.readFileSync(path.join(source, 'frames.jsonl'), 'utf8').trim().split(/\r?\n/).map((l) => JSON.parse(l)).slice(part.from, part.from + SEGMENT);
  if (rows.length !== SEGMENT) throw new Error(`${part.scene} has fewer than ${part.from + SEGMENT} instants`);
  for (const row of rows) {
    const frame = row.frame + part.offset;
    if (!fs.existsSync(path.join(out, frameFile(frame)))) fs.copyFileSync(path.join(source, frameFile(row.frame)), path.join(out, frameFile(frame)));
    // `new` here means: the picture differs from the instant before it (the first instant of a segment always does)
    const previous = lines.at(-1);
    lines.push({ ...row, instant: lines.length, frame, new: !previous || previous.frame !== frame });
  }
}
fs.writeFileSync(path.join(out, 'frames.jsonl'), lines.map((l) => JSON.stringify(l)).join('\n') + '\n');
const meta = JSON.parse(fs.readFileSync(path.join(ROOT, PARTS[0].scene, 'meta.json'), 'utf8'));
fs.writeFileSync(path.join(out, 'meta.json'), JSON.stringify({ scenario: 'resize', built_from: PARTS, instants: lines.length, instant_ms: meta.instant_ms }, null, 2) + '\n');
console.log(`${out}: ${lines.length} instants, sizes ${[...new Set(lines.map((l) => `${l.width}x${l.height}`))].join(', ')}`);
