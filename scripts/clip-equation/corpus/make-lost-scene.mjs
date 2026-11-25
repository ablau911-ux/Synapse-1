// Builds the corpus scene `lost-then-same-640x360` from `still-640x360`: 60 instants of one unchanged
// picture with one failed capture at instant 20 (`"lost": true`). Picture A, a lost instant, A again.
//   node scripts/clip-equation/corpus/make-lost-scene.mjs
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const BASE = path.dirname(fileURLToPath(import.meta.url));
const ROOT = JSON.parse(fs.readFileSync(path.join(BASE, 'corpus.json'), 'utf8')).root;
const SOURCE = 'still-640x360', NAME = 'lost-then-same-640x360';
const INSTANTS = 60, LOST_INSTANT = 20;

const source = path.join(ROOT, SOURCE), out = path.join(ROOT, NAME);
fs.rmSync(out, { recursive: true, force: true });
fs.mkdirSync(out, { recursive: true });
const rows = fs.readFileSync(path.join(source, 'frames.jsonl'), 'utf8').trim().split(/\r?\n/).map((l) => JSON.parse(l)).slice(0, INSTANTS);
if (rows.length !== INSTANTS) throw new Error(`${SOURCE} has fewer than ${INSTANTS} instants`);
if (new Set(rows.map((r) => r.frame)).size !== 1) throw new Error(`${SOURCE} is expected to show one unchanged picture`);
const frame = `frame-${String(rows[0].frame).padStart(4, '0')}.zst`;
fs.copyFileSync(path.join(source, frame), path.join(out, frame));
const lines = rows.map((row, k) => ({ ...row, instant: k, lost: k === LOST_INSTANT }));
fs.writeFileSync(path.join(out, 'frames.jsonl'), lines.map((l) => JSON.stringify(l)).join('\n') + '\n');
const meta = JSON.parse(fs.readFileSync(path.join(source, 'meta.json'), 'utf8'));
fs.writeFileSync(path.join(out, 'meta.json'), JSON.stringify({ scenario: 'lost-then-same', built_from: SOURCE, lost_instant: LOST_INSTANT, instants: INSTANTS, instant_ms: meta.instant_ms }, null, 2) + '\n');
console.log(`${out}: ${INSTANTS} instants, instant ${LOST_INSTANT} lost`);
