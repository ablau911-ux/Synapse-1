#!/usr/bin/env node
// Runs the product probe, then independently decodes the format from section 4.2.
import fs from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { runFeature } from './feature.mjs';

const BASE = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const config = JSON.parse(fs.readFileSync(path.join(BASE, 'corpus/corpus.json'), 'utf8'));
const options = Object.fromEntries(process.argv.slice(process.argv[2] === 'dyn-exact' ? 3 : 2).map(arg => {
  const split = arg.indexOf('=');
  return [arg.slice(0, split), arg.slice(split + 1)];
}));
const ROOT = options.root || process.env.CLIP_CORPUS || config.root;
const OUT = path.resolve(options.out || path.join(BASE, '../../.runs/dyn-exact'));
const PROBE = path.resolve(options.probe || path.join(BASE, 'probe/target/release/clip-probe.exe'));
const MIB = 1024 ** 2;
const MINUTE_MS = 60_000;
const PERCENT_DENOMINATOR = 100;
const SIZE_SCENES = ['typing-1920x1080', 'ui-1920x1080', 'scroll-1920x1080', 'flash-1920x1080'];
const MAX_TYPING_ROW_SHARE = 0.05;
const WRITER_RAM_MIB = 141;
const MIXED_BASELINE_MIB = 3.24;
const BUDGET_MARGIN = 1.10;
const NAMES = [...Object.keys(config.scenarios).map(scene => `${scene}-1920x1080`), 'typing-506x333', 'mix-1920x1080'];

function jsonLines(file) {
  return fs.readFileSync(file, 'utf8').split(/\r?\n/).filter(Boolean).map(line => JSON.parse(line));
}

function inspect(name, folder, report) {
  const sourceFolder = path.join(ROOT, name);
  const sourceLines = jsonLines(path.join(sourceFolder, 'frames.jsonl'));
  const lines = jsonLines(path.join(folder, 'frames.jsonl'));
  if (lines.length !== sourceLines.length) throw new Error('instant count differs');
  const refs = lines.filter(line => line.x);
  const groups = Map.groupBy(refs, line => line.x[0]);
  const files = fs.readdirSync(path.join(folder, 'frames')).sort();
  let valid = files.length === groups.size && groups.size === report.groups;
  let decoded, rawAt = 0, group = -1, picture, source, sourceFrame = -1;
  let mismatches = 0, nonKeyRaw = 0, nonKeyCount = 0, nextOffset = 0;
  let broadRecords = 0, broadViolations = 0;
  let frameBytes = 0;
  for (let index = 0; index < lines.length; index++) {
    const line = lines[index], original = sourceLines[index];
    if (line.instant !== original.instant) throw new Error('instant mapping differs');
    if (line.x) {
      const [g, offset, bytes, raw] = line.x;
      if (g !== group) {
        if (decoded && rawAt !== decoded.length) valid = false;
        const compressed = fs.readFileSync(path.join(folder, 'frames', `group-${String(g).padStart(6, '0')}.zst`));
        frameBytes += compressed.length;
        // Whole-file decoding verifies the end marker and checksum too.
        decoded = zlib.zstdDecompressSync(compressed);
        rawAt = 0; nextOffset = 0; group = g;
        picture = Buffer.alloc(line.width * line.height * report.constants.BGR_BYTES_PER_PIXEL);
        valid &&= offset === 0 && raw === report.constants.EXACT_KEY_HEADER_BYTES + picture.length;
        valid &&= groups.get(g).length <= report.constants.EXACT_GROUP_PICTURES;
        valid &&= groups.get(g).every(row => row.x[1] + row.x[2] <= compressed.length);
      } else {
        nonKeyRaw += raw; nonKeyCount++;
      }
      valid &&= offset === nextOffset && bytes > 0;
      nextOffset = offset + bytes;
      const payload = decoded.subarray(rawAt, rawAt + raw);
      rawAt += raw;
      const bands = payload.readUInt16LE(0);
      let pixelsAt = report.constants.EXACT_COUNT_BYTES + bands * report.constants.EXACT_BAND_BYTES;
      let previousEnd = 0;
      let writtenRows = 0;
      for (let band = 0; band < bands; band++) {
        const at = report.constants.EXACT_COUNT_BYTES + band * report.constants.EXACT_BAND_BYTES;
        const row = payload.readUInt16LE(at), count = payload.readUInt16LE(at + report.constants.EXACT_COUNT_BYTES);
        valid &&= count > 0 && row >= previousEnd && row + count <= line.height;
        previousEnd = row + count;
        writtenRows += count;
        const length = count * line.width * report.constants.BGR_BYTES_PER_PIXEL;
        payload.copy(picture, row * line.width * report.constants.BGR_BYTES_PER_PIXEL, pixelsAt, pixelsAt + length);
        pixelsAt += length;
      }
      if (writtenRows * PERCENT_DENOMINATOR > line.height * report.constants.EXACT_WHOLE_FRAME_ROW_PERCENT) {
        broadRecords++;
        if (bands !== 1 || payload.readUInt16LE(report.constants.EXACT_COUNT_BYTES) !== 0 || writtenRows !== line.height) broadViolations++;
      }
      valid &&= pixelsAt === raw;
    }
    if (original.frame !== sourceFrame) {
      source = zlib.zstdDecompressSync(fs.readFileSync(path.join(sourceFolder, `frame-${String(original.frame).padStart(4, '0')}.zst`)));
      sourceFrame = original.frame;
    }
    if (!picture?.equals(source)) {
      for (let byte = 0; byte < source.length; byte++) mismatches += picture?.[byte] !== source[byte] ? 1 : 0;
    }
  }
  valid &&= rawAt === decoded?.length && frameBytes === report.bytes && refs.length === report.records;
  const total = frameBytes + fs.statSync(path.join(folder, 'frames.jsonl')).size;
  const duration = lines.length * JSON.parse(fs.readFileSync(path.join(sourceFolder, 'meta.json'), 'utf8')).instant_ms;
  return { name, ...report, mismatches, valid, total, broadRecords, broadViolations,
    mibPerMinute: total / MIB * MINUTE_MS / duration,
    rowShare: nonKeyCount ? nonKeyRaw / nonKeyCount / (lines[0].width * lines[0].height * report.constants.BGR_BYTES_PER_PIXEL) : 0 };
}

const rows = [], errors = [];
fs.mkdirSync(OUT, { recursive: true });
for (const name of NAMES) {
  try {
    if (!fs.existsSync(path.join(ROOT, name, 'frames.jsonl'))) throw new Error('corpus unavailable: BLOCKED');
    const folder = path.join(OUT, name);
    // The caller chooses a fresh OUT. Never remove an existing recording.
    const run = spawnSync(PROBE, ['exp-dyn', 'mode=exact', `folder=${path.join(ROOT, name)}`, `out=${folder}`], { encoding: 'utf8' });
    if (run.error || run.status !== 0) throw new Error(run.error?.message || run.stderr.trim() || `probe exit ${run.status}`);
    const report = JSON.parse(run.stdout.trim().split(/\r?\n/).at(-1));
    const baseline = spawnSync(PROBE, ['exp-z', `folder=${path.join(ROOT, name)}`,
      `out=${path.join(OUT, `${name}-baseline`)}`, 'way=ZW13', 'layout=bgr', 'mode=stream',
      `level=${report.constants.EXACT_ZSTD_LEVEL}`, `wlog=${report.constants.EXACT_WINDOW_LOG}`,
      `kz=${report.constants.EXACT_GROUP_PICTURES}`, 'ldm=0', 'keep=0'], { encoding: 'utf8' });
    if (baseline.error || baseline.status !== 0) throw new Error(baseline.error?.message || baseline.stderr.trim() || `baseline exit ${baseline.status}`);
    const measured = JSON.parse(baseline.stdout.trim().split(/\r?\n/).at(-1));
    if (measured.diff_sum !== 0) throw new Error('whole-frame baseline round trip failed');
    report.baseline_bytes = measured.bytes_disk;
    fs.writeFileSync(path.join(OUT, `${name}-baseline.json`), `${JSON.stringify(measured)}\n`);
    fs.writeFileSync(path.join(OUT, `${name}.json`), `${JSON.stringify(report)}\n`);
    rows.push(inspect(name, folder, report));
  } catch (error) {
    errors.push(`${name}: ${error.message}`);
  }
}
const complete = rows.length === NAMES.length;
const result = (pass, measured) => ({ pass: complete && pass, value: '-', measured: errors.length ? errors.join('; ') : measured });
const values = field => rows.map(row => `${row.name}=${row[field]}`).join('; ');
const table = {
  'corpus pictures round trip independently': () => result(rows.every(row => row.mismatches === 0), values('mismatches')),
  'product reader matches every instant and backward seeks': () => result(rows.every(row => row.reader_mismatches === 0), values('reader_mismatches')),
  'groups are complete independent zstd frames': () => result(rows.every(row => row.valid), values('groups')),
  'idle writes one record': () => result(rows.find(row => row.name === 'still-1920x1080')?.records === 1, values('records')),
  'typing records contain only changed rows': () => result(rows.filter(row => row.name.startsWith('typing')).every(row => row.rowShare < MAX_TYPING_ROW_SHARE), values('rowShare')),
  'broad row changes write the whole picture': () => result(rows.every(row => Number.isFinite(row.constants.EXACT_WHOLE_FRAME_ROW_PERCENT) && row.broadViolations === 0) && rows.find(row => row.name === 'scroll-1920x1080')?.broadRecords > 0, values('broadViolations')),
  'exact size meets whole-frame stream figures': () => result(rows.filter(row => SIZE_SCENES.includes(row.name)).every(row => row.total <= row.baseline_bytes), rows.filter(row => SIZE_SCENES.includes(row.name)).map(row => `${row.name}=${row.total} <= ${row.baseline_bytes}`).join('; ')),
  'exact writer RAM stays within budget': () => result(rows.every(row => row.ram_peak <= WRITER_RAM_MIB * MIB * BUDGET_MARGIN), values('ram_peak')),
  'mixed clip stays within ten percent': () => result(rows.find(row => row.name === 'mix-1920x1080')?.total <= BUDGET_MARGIN * MIXED_BASELINE_MIB * MIB, values('total')),
};
fs.writeFileSync(path.join(OUT, 'checks.json'), `${JSON.stringify({ rows, errors }, null, 2)}\n`);
process.exitCode = runFeature('dyn-exact', path.join(BASE, 'spec/dyn-exact.feature'), table);
