// Experiment E1: R after the NV12 conversion alone (no H.264), on every text folder.
// Same probe arguments as run-d.mjs (DW2, q100, gop 20, hint 1); `e1_nv12=1` makes OCR read
// source → NV12 (the encoder's converter) → BGRA instead of the decoded frame.
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

const BASE = 'C:/Users/Adam/Synapse-clip/scripts/clip-equation';
const PROBE = `${BASE}/probe/target/release/clip-probe.exe`;
const CORPUS = JSON.parse(fs.readFileSync(`${BASE}/corpus/corpus.json`, 'utf8'));
const enc = JSON.parse(fs.readFileSync(`${BASE}/results/encoders.json`, 'utf8')).find((e) => e.codec === 'H.264' && e.vendor_id === '');
const rate = /rate=\d+/.exec(spawnSync(PROBE, ['exp-rate'], { encoding: 'utf8', windowsHide: true }).stdout)[0];
const f = CORPUS.flash;
const flash = `flash=${f.bgr.join(':')}:${f.box_left_fraction}:${f.box_top}:${f.box_width}:${f.box_height}:${f.min_pixels}`;
const regions = (sc) => (CORPUS.text_regions?.[sc] ?? []).map((g) => [g.left, g.top, g.right, g.bottom, g.max_width ?? 0, g.max_height ?? 0, g.max_width_fraction ?? 0].join(':')).join(';');

const rows = [];
for (const scenario of ['typing', 'scroll', 'ui']) {
  for (const s of CORPUS.sizes) {
    const folder = `${scenario}-${s.width}x${s.height}`;
    const scratch = path.join(os.tmpdir(), 'clip-e1', folder);
    fs.rmSync(scratch, { recursive: true, force: true });
    const common = [`folder=${path.join(CORPUS.root, folder)}`, 'encoder=DW2', `name=${enc.name.replace(/[^\x20-\x7E]/g, '')}`, 'vendor=', 'hw=0',
      'q=100', 'gop=20', 'hint=1', 'ratemode=quality', 'beta=0.1', `scratch=${scratch}`, 'repeat=1', rate, `window_width=${s.width}`, flash, `regions=${regions(scenario)}`];
    let out = {};
    for (const phase of ['live', 'worker']) {
      const res = spawnSync(PROBE, ['exp-d', `phase=${phase}`, ...common, ...(phase === 'worker' ? ['e1_nv12=1'] : [])], { encoding: 'utf8', windowsHide: true, maxBuffer: 1 << 24 });
      const line = (res.stdout || '').split(/\r?\n/).find((l) => l.startsWith('{'));
      out = line ? JSON.parse(line) : { status: `no JSON, exit ${res.status}`, error: (res.stderr || '').slice(0, 200) };
      if (phase === 'live' && out.status !== 'ok') break;
    }
    fs.rmSync(scratch, { recursive: true, force: true });
    rows.push({ folder, sigma: s.sigma, status: out.status, r: out.r, note: out.r_note, error: out.error });
    console.log(folder, 'σ', s.sigma, out.status, 'R_420', out.r, out.error ?? '');
  }
}
fs.writeFileSync(path.join(BASE, 'results', 'e1-nv12.json'), JSON.stringify(rows, null, 1));
