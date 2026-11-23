import fs from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';
import { fileURLToPath } from 'node:url';

export const SCENES = [
  { name: 'noise', boxes: [[400, 304, 320, 240]] },
  { name: 'small', boxes: [[400, 304, 48, 48]] },
  { name: 'bar', boxes: [[400, 304, 512, 16]] },
  { name: 'flat', boxes: [[400, 304, 320, 240]], flat: true },
  { name: 'two', boxes: [[400, 304, 320, 240], [1408, 704, 160, 128]] },
  { name: 'odd', boxes: [[403, 301, 321, 243]] },
  { name: 'corner', width: 1001, height: 701, boxes: [[681, 461, 320, 240]] },
  { name: 'slow', boxes: [[400, 304, 320, 240]], every: 3 },
];
export const INSTANTS = 200;
export const FIRST_CHANGE = 40;
export const LAST_CHANGE = 120;
const WIDTH = 1920, HEIGHT = 1080, CHANNELS = 3, INSTANT_MS = 50;
const SEED = 0x6d2b79f5, XOR_LEFT = 13, XOR_RIGHT = 17, XOR_FINAL = 5;

export function renderScenes(root, background) {
  const base = zlib.zstdDecompressSync(fs.readFileSync(background));
  for (const scene of SCENES) {
    const width = scene.width || WIDTH, height = scene.height || HEIGHT;
    const folder = path.join(root, scene.name);
    fs.mkdirSync(folder, { recursive: true });
    const picture = Buffer.alloc(width * height * CHANNELS);
    for (let y = 0; y < height; y++) base.copy(picture, y * width * CHANNELS, y * WIDTH * CHANNELS, (y * WIDTH + width) * CHANNELS);
    let seed = SEED, frame = -1;
    const lines = [];
    for (let instant = 0; instant < INSTANTS; instant++) {
      const changed = instant >= FIRST_CHANGE && instant <= LAST_CHANGE && (instant - FIRST_CHANGE) % (scene.every || 1) === 0;
      if (changed) {
        for (const [x, y, w, h] of scene.boxes) {
          for (let row = y; row < y + h; row++) {
            for (let at = (row * width + x) * CHANNELS; at < (row * width + x + w) * CHANNELS; at++) {
              seed ^= seed << XOR_LEFT; seed ^= seed >>> XOR_RIGHT; seed ^= seed << XOR_FINAL;
              picture[at] = scene.flat ? (instant % 2 ? 32 : 224) : seed & 255;
            }
          }
        }
      }
      const fresh = instant === 0 || changed;
      if (fresh) {
        frame++;
        fs.writeFileSync(path.join(folder, `frame-${String(frame).padStart(4, '0')}.zst`), zlib.zstdCompressSync(picture));
      }
      lines.push({ instant, new: fresh, frame, width, height });
    }
    fs.writeFileSync(path.join(folder, 'frames.jsonl'), lines.map(x => JSON.stringify(x)).join('\n') + '\n');
    fs.writeFileSync(path.join(folder, 'meta.json'), JSON.stringify({ instant_ms: INSTANT_MS, width, height }));
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) renderScenes(process.argv[2], process.argv[3]);
