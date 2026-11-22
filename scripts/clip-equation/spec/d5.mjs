// Clip viewer D5: the checks of scripts/clip-equation/spec/d5.feature.
// Spec: docs/clip-recorder-plan.md, "D5 as an equation", item 7, and Gitea issue #293.
//
//   node run-spec.mjs d5 <viewer-output.json> <schedule.json>
//
// viewer-output.json, one object per clip (missing clips fail their rows):
//   {
//     "proof":   { "timeline": [...], "picks": [...], "frame_ms": 50, "end_ms": 34000,
//                  "keyframes_ms": [0, 1000, ...], "schedule_start_ms": 2950 },
//     "still":   { "timeline": [...], "picks": [...], "keyframes_ms": [...] },
//     "spinner": { "timeline": [...] },
//     "flash":   { "timeline": [...], "flashes": 20 }
//   }
//   timeline and picks are the viewer's overview_analysis.json fields (ClipTimelineEntry:
//   kind, start_ms, end_ms, start_frame, end_frame, text; Pick: frame, t_ms, reason).
//   schedule_start_ms is the clip time at which the page's schedule starts (its ?start= minus the
//   clip's start, both in unix ms).
// schedule.json, read from the page after the run:
//   { "start": <unix ms>, "schedule": window.schedule, "actual": window.actualTiming,
//     "box_fits": window.scheduleBoxFits }
//
// Without inputs every row fails with "no input": the red result before the verification round.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { runFeature } from './feature.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const FEATURE = path.join(HERE, 'd5.feature');
const EVENT_KINDS = new Set(['change', 'short_lived', 'animating']);
const DEFAULT_FRAME_MS = 50;
// docs/clip-recorder-plan.md D5 items 2 and 3 (clip_viewer.rs SETTLE_MS, page FLASH_MS).
const SETTLE_MS = 200;
const FLASH_MS = 50;
const FADE_MS = 300;
const SETTLED_PICK = 'settled frame after change';
// Schedule rows by name prefix (ui-events-page.html).
const ROW = {
  fade: 'dialog fades in',
  still: 'still period',
  alternation: 'grey square alternates',
  laterFlash: '50 ms dark flash in the grey square',
};

/** A JSON file, or null when the path is missing. */
function readJson(file) {
  return file && fs.existsSync(file) ? JSON.parse(fs.readFileSync(file, 'utf8')) : null;
}

/** The inputs of one run: the viewer's output per clip and the page's schedule. */
class D5Inputs {
  constructor(viewerFile, scheduleFile) {
    this.viewer = readJson(viewerFile);
    this.page = readJson(scheduleFile);
    this.viewerFile = viewerFile ?? '(none given)';
    this.scheduleFile = scheduleFile ?? '(none given)';
  }

  /** The clip object `name`, or throws "no input". */
  clip(name) {
    const clip = this.viewer?.[name];
    if (!clip || !Array.isArray(clip.timeline)) throw new Error(`no input: clip "${name}" in ${this.viewerFile}`);
    return new ViewedClip(clip);
  }

  /** The page's schedule.json, or throws "no input". */
  get schedule() {
    if (!this.page || !Array.isArray(this.page.schedule)) throw new Error(`no input: schedule in ${this.scheduleFile}`);
    return this.page;
  }

  /** The schedule row whose name starts with `prefix`. */
  row(prefix) {
    const row = this.schedule.schedule.find((r) => r.name.startsWith(prefix));
    if (!row) throw new Error(`no schedule row "${prefix}"`);
    return row;
  }

  /** Clip ms at which `row` was presented: its actual time when the page recorded one. */
  rowStart(proof, row) {
    const actual = (this.schedule.actual ?? []).find((a) => a.name === row.name);
    const t = Number.isFinite(actual?.shown_ms) ? actual.shown_ms : row.t_ms;
    if (!Number.isFinite(proof.raw.schedule_start_ms)) throw new Error('no input: proof.schedule_start_ms');
    return proof.raw.schedule_start_ms + t;
  }
}

/** One clip's viewer output. */
class ViewedClip {
  constructor(raw) {
    this.raw = raw;
    this.frameMs = raw.frame_ms ?? DEFAULT_FRAME_MS;
    this.events = raw.timeline.filter((e) => EVENT_KINDS.has(e.kind));
  }

  /** Events of `kind` (any event kind when null) starting in [from, to). */
  startingIn(from, to, kind = null) {
    return this.events.filter((e) => (kind === null || e.kind === kind) && e.start_ms >= from && e.start_ms < to);
  }

  /** Events of `kind` whose start is within one instant of `t`. */
  near(t, kind) {
    return this.events.filter((e) => e.kind === kind && Math.abs(e.start_ms - t) <= this.frameMs);
  }

  count(kind) {
    return this.events.filter((e) => e.kind === kind).length;
  }
}

/** The checks by scenario title. */
function checks(inputs) {
  return {
    'every scheduled event reported'() {
      const proof = inputs.clip('proof');
      const rows = inputs.schedule.schedule.filter((r) => r.expect !== 'none');
      const missed = rows.filter((r) => proof.near(inputs.rowStart(proof, r), r.expect).length === 0).map((r) => `${r.t_ms} ${r.expect}`);
      return { value: `${rows.length} rows`, measured: `${rows.length - missed.length} matched${missed.length ? `; missed: ${missed.join(', ')}` : ''}`, pass: missed.length === 0 };
    },

    'still period gives no event'() {
      const proof = inputs.clip('proof');
      const row = inputs.row(ROW.still);
      const from = inputs.rowStart(proof, row);
      const found = proof.startingIn(from, from + (row.until_ms - row.t_ms));
      return { value: 0, measured: `${found.length} events${found.length ? `: ${found.map((e) => `${e.kind}@${e.start_ms}`).join(', ')}` : ''}`, pass: found.length === 0 };
    },

    'still clip gives no event'() {
      const still = inputs.clip('still');
      return { value: 0, measured: `${still.events.length} events`, pass: still.events.length === 0 };
    },

    'lone spinner gives exactly one animation'() {
      const spinner = inputs.clip('spinner');
      const animations = spinner.count('animating');
      const others = spinner.events.length - animations;
      return { value: '1, 0', measured: `${animations}, ${others}`, pass: animations === 1 && others === 0 };
    },

    'keyframe instants give no event'() {
      const proof = inputs.clip('proof');
      const still = inputs.clip('still');
      const scheduled = inputs.schedule.schedule.map((r) => inputs.rowStart(proof, r));
      const atKeyframe = (clip, starts) => clip.events.filter((e) =>
        (clip.raw.keyframes_ms ?? []).some((k) => Math.abs(e.start_ms - k) <= clip.frameMs)
        && !starts.some((t) => Math.abs(e.start_ms - t) <= clip.frameMs)
        && e.start_ms > clip.frameMs);
      if (!Array.isArray(proof.raw.keyframes_ms) || !Array.isArray(still.raw.keyframes_ms)) throw new Error('no input: keyframes_ms');
      const found = [...atKeyframe(proof, scheduled), ...atKeyframe(still, [])];
      return { value: 0, measured: `${found.length} events at keyframes${found.length ? `: ${found.map((e) => `${e.kind}@${e.start_ms}`).join(', ')}` : ''}`, pass: found.length === 0 };
    },

    'flash clip gives one FLASH per flash'() {
      const flash = inputs.clip('flash');
      const flashes = flash.raw.flashes;
      if (!Number.isInteger(flashes)) throw new Error('no input: flash.flashes');
      const short = flash.count('short_lived');
      const changes = flash.count('change');
      return { value: `${flashes}, 0`, measured: `${short}, ${changes}`, pass: short === flashes && changes === 0 };
    },

    '#293-1 a fade is one change from its start'() {
      const proof = inputs.clip('proof');
      const t = inputs.rowStart(proof, inputs.row(ROW.fade));
      const inFade = proof.startingIn(t - proof.frameMs, t + FADE_MS + SETTLE_MS, 'change');
      const startOk = inFade.length === 1 && Math.abs(inFade[0].start_ms - t) <= proof.frameMs;
      return { value: `1 at ${t}`, measured: `${inFade.length} at ${inFade.map((e) => e.start_ms).join(', ') || '-'}`, pass: startOk };
    },

    '#293-2 an animation that stays SAME to its anchor'() {
      const proof = inputs.clip('proof');
      const row = inputs.row(ROW.alternation);
      const t = inputs.rowStart(proof, row);
      const end = t + (row.until_ms - row.t_ms);
      const animations = proof.near(t, 'animating').length;
      const others = proof.startingIn(t, end, 'change').length + proof.startingIn(t, end, 'short_lived').length;
      return { value: '1, 0', measured: `${animations}, ${others}`, pass: animations === 1 && others === 0 };
    },

    '#293-3 a change is pictured at its confirmed settle frame'() {
      const proof = inputs.clip('proof');
      if (!Array.isArray(proof.raw.picks)) throw new Error('no input: proof.picks');
      const settleInstants = Math.round(SETTLE_MS / proof.frameMs);
      const settled = proof.events.filter((e) => e.kind === 'change' && /settled at/.test(e.text ?? ''));
      const bad = settled.filter((e) => !proof.raw.picks.some((p) => p.reason.includes(SETTLED_PICK) && p.frame >= e.end_frame + settleInstants && p.frame <= e.end_frame + 3 * settleInstants));
      return { value: `pick >= end + ${settleInstants}`, measured: `${settled.length - bad.length}/${settled.length}${bad.length ? `; wrong: ${bad.map((e) => e.end_frame).join(', ')}` : ''}`, pass: settled.length > 0 && bad.length === 0 };
    },

    '#293-4 quiet for SETTLE_MS before the clip end is settled'() {
      const proof = inputs.clip('proof');
      if (!Number.isFinite(proof.raw.end_ms)) throw new Error('no input: proof.end_ms');
      const unsettled = proof.events.filter((e) => /still changing when the clip ended/.test(e.text ?? ''));
      const bad = unsettled.filter((e) => proof.raw.end_ms - e.end_ms >= SETTLE_MS);
      return { value: `< ${SETTLE_MS} ms`, measured: `${unsettled.length} unsettled, ${bad.length} quiet >= ${SETTLE_MS} ms`, pass: bad.length === 0 };
    },

    '#293-5 only the animated stretch is exempt from FLASH'() {
      const proof = inputs.clip('proof');
      const row = inputs.row(ROW.alternation);
      const t = inputs.rowStart(proof, row);
      const inStretch = proof.startingIn(t, t + (row.until_ms - row.t_ms), 'short_lived').length;
      const later = proof.near(inputs.rowStart(proof, inputs.row(ROW.laterFlash)), 'short_lived').length;
      return { value: '0, 1', measured: `${inStretch}, ${later}`, pass: inStretch === 0 && later === 1 };
    },

    '#293-6 flashes are timed from their presentation'() {
      const flashRows = inputs.schedule.schedule.filter((r) => r.expect === 'short_lived');
      const actual = inputs.schedule.actual ?? [];
      const bad = flashRows.filter((r) => {
        const a = actual.find((x) => x.name === r.name);
        return !(Number.isFinite(a?.shown_ms) && Number.isFinite(a?.hidden_ms) && a.hidden_ms - a.shown_ms >= FLASH_MS);
      });
      return { value: `>= ${FLASH_MS} ms`, measured: `${flashRows.length - bad.length}/${flashRows.length} flashes${bad.length ? `; wrong: ${bad.map((r) => r.t_ms).join(', ')}` : ''}`, pass: flashRows.length > 0 && bad.length === 0 };
    },

    '#293-7 the schedule box shows every row'() {
      const fits = inputs.schedule.box_fits;
      return { value: true, measured: String(fits), pass: fits === true };
    },
  };
}

export function main(viewerFile, scheduleFile) {
  console.log(`d5: viewer output ${viewerFile ?? '(none given)'}, schedule ${scheduleFile ?? '(none given)'}`);
  return runFeature('d5', FEATURE, checks(new D5Inputs(viewerFile, scheduleFile)));
}
