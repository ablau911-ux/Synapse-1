// Clip viewer D5: the checks of scripts/clip-equation/spec/d5.feature.
// Spec: docs/clip-recorder-plan.md, "D5 as an equation", item 7, and Gitea issue #293.
//
//   node run-spec.mjs d5 <viewer-output.json> <schedule.json>
//
// viewer-output.json, one object per clip (a missing clip or field fails its rows):
//   {
//     "proof":          { "timeline": [...], "picks": [...], "fps": 20, "frame_ms": 50, "end_ms": 34000,
//                         "keyframes_ms": [0, 1000, ...], "schedule_start_ms": 2950,
//                         "viewport": { "x": 8, "y": 120, "scale": 1 } },
//     "still":          { "timeline": [...], "fps": 20, "frame_ms": 50, "end_ms": 20000, "keyframes_ms": [...] },
//     "spinner":        { "timeline": [...], "fps": 20, "frame_ms": 50 },
//     "flash":          { "timeline": [...], "fps": 20, "frame_ms": 50, "shown_ms": [1000, 2000, ...] },
//     "boundary_below": { "timeline": [...], "fps": 20, "frame_ms": 50, "end_ms": ..., "schedule_start_ms": ...,
//                         "viewport": {...}, "actual": [...] },
//     "boundary_at":    { same fields }
//   }
//   timeline and picks are the viewer's overview_analysis.json fields (ClipTimelineEntry: kind,
//   start_ms, end_ms, start_frame, end_frame, bbox {x, y, w, h} in recorded px, text; Pick: frame,
//   t_ms, reason). fps is the clip's f (clip.json); frame_ms must be 1000 / fps. end_ms is the clip
//   end, (last frame + 1) * frame_ms. schedule_start_ms is the clip time of the page's schedule
//   start. viewport maps the page's CSS px to recorded px: x_rec = x + x_css * scale. shown_ms of
//   the flash clip lists the clip time at which each flash was presented. A boundary clip is its own
//   page run: it carries that run's window.actualTiming as "actual"; it ends 150 ms (below) or
//   exactly 200 ms (at) after its "last change" row was shown.
// schedule.json, read from the proof run's page after the run:
//   { "start": <unix ms>, "schedule": window.schedule, "actual": window.actualTiming,
//     "always": window.always, "box_fits": window.scheduleBoxFits }
//
// Without inputs every row fails with "no input": the red result before the verification round.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { runFeature } from './feature.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const FEATURE = path.join(HERE, 'd5.feature');
const EVENT_KINDS = new Set(['change', 'short_lived', 'animating']);
// docs/clip-recorder-plan.md D5 items 2 and 3 (clip_viewer.rs SETTLE_MS, SHORT_LIVED_MS, BLOCK;
// page FLASH_MS).
const SETTLE_MS = 200;
const SHORT_LIVED_MS = 500;
const BLOCK_PX = 8;
const FLASH_MS = 50;
const SETTLED_PICK = 'settled frame after change';
const CONFIRMED = /settled at .*confirmed at frame (\d+)/;
// Schedule rows by id (ui-events-page.html).
const ROW = { fade: 'fade', still: 'still', alternate: 'alternate', darkflash: 'darkflash', boundary: 'boundary' };

/** A JSON file, or null when the path is missing. */
function readJson(file) {
  return file && fs.existsSync(file) ? JSON.parse(fs.readFileSync(file, 'utf8')) : null;
}

const finite = (value) => typeof value === 'number' && Number.isFinite(value);
const list = (events) => events.map((e) => `${e.kind}@${e.start_ms}`).join(', ') || '-';

/** One clip's viewer output, with its cadence checked. */
class ViewedClip {
  constructor(name, raw) {
    if (!raw || !Array.isArray(raw.timeline)) throw new Error(`no input: clip "${name}"`);
    if (!finite(raw.fps) || raw.fps <= 0) throw new Error(`no input: ${name}.fps`);
    if (raw.frame_ms !== 1000 / raw.fps) throw new Error(`${name}.frame_ms ${raw.frame_ms} is not 1000 / fps (${1000 / raw.fps})`);
    this.name = name;
    this.raw = raw;
    this.frameMs = raw.frame_ms;
    this.events = raw.timeline.filter((e) => EVENT_KINDS.has(e.kind));
    // The timeline must follow the cadence: an event's time is its frame times frame_ms.
    const off = this.events.find((e) => Math.abs(e.start_ms - e.start_frame * this.frameMs) > this.frameMs);
    if (off) throw new Error(`${name}: event at frame ${off.start_frame} has ${off.start_ms} ms, not ${off.start_frame} * ${this.frameMs}`);
  }

  require(field) {
    if (this.raw[field] === undefined || this.raw[field] === null) throw new Error(`no input: ${this.name}.${field}`);
    return this.raw[field];
  }

  get endMs() {
    const end = this.require('end_ms');
    if (!finite(end)) throw new Error(`${this.name}.end_ms is not a number`);
    return end;
  }

  /** The last instant of the clip: the boundary minus one. */
  get lastFrame() {
    return Math.round(this.endMs / this.frameMs) - 1;
  }

  count(kind) {
    return this.events.filter((e) => e.kind === kind).length;
  }

  /** `true` when the event's box meets `rect` (recorded px), one block of slack. */
  static meets(event, rect) {
    const b = event.bbox;
    if (!b || !rect) return false;
    return b.x < rect.x + rect.w + BLOCK_PX && rect.x < b.x + b.w + BLOCK_PX
      && b.y < rect.y + rect.h + BLOCK_PX && rect.y < b.y + b.h + BLOCK_PX;
  }
}

/** The schedule as one page run presented it, on one clip's timeline. */
class RunTiming {
  constructor(clip, page, actual) {
    this.clip = clip;
    this.page = page;
    this.actual = actual;
    this.startMs = clip.require('schedule_start_ms');
    this.viewport = clip.require('viewport');
    if (!finite(this.startMs)) throw new Error(`${clip.name}.schedule_start_ms is not a number`);
  }

  row(id) {
    const row = this.page.schedule.find((r) => r.id === id);
    if (!row) throw new Error(`no schedule row "${id}"`);
    return row;
  }

  actualOf(row) {
    const actual = this.actual.find((a) => a.name === row.name);
    if (!actual || !finite(actual.shown_ms) || !finite(actual.set_ms) || actual.shown_ms < actual.set_ms) {
      throw new Error(`no presentation time for row "${row.id}" (no fallback to the nominal ${row.t_ms} ms)`);
    }
    return actual;
  }

  /** Clip ms at which `row` was presented. */
  start(row) {
    return this.startMs + this.actualOf(row).shown_ms;
  }

  /** Clip ms at which `row` ends (its until_ms, shifted like its start). */
  until(row) {
    return this.start(row) + (row.until_ms - row.t_ms);
  }

  /** A CSS rect of the page in recorded px. */
  toRecorded(rect) {
    if (!rect) return null;
    const { x, y, scale } = this.viewport;
    return { x: x + rect.x * scale, y: y + rect.y * scale, w: rect.w * scale, h: rect.h * scale };
  }

  region(row) {
    const rect = this.toRecorded(this.actualOf(row).rect);
    if (!rect) throw new Error(`no region rect for row "${row.id}"`);
    return rect;
  }

  /** The events of `kind` in the region of `row` whose start is within one instant of it. */
  near(row, kind) {
    const t = this.start(row);
    const rect = this.region(row);
    return this.clip.events.filter((e) => e.kind === kind && ViewedClip.meets(e, rect) && Math.abs(e.start_ms - t) <= this.clip.frameMs);
  }
}

/**
 * One-to-one matching of the proof clip's events with what the page presented: each expected row
 * (scheduled, or running from page load) takes the events of its kind, in its region, within one
 * instant of its start. A row may take one event (several = duplicate) or, when `many`, all events
 * in its interval. Every event no row takes is unexpected.
 */
class ScheduleMatch {
  constructor(timing) {
    const clip = timing.clip;
    this.byRow = new Map();
    this.missed = [];
    this.duplicates = [];
    const used = new Set();
    const take = (key, events) => {
      events.forEach((e) => used.add(e));
      this.byRow.set(key, events);
    };
    for (const row of timing.page.always ?? []) {
      if (finite(row.until_ms) && timing.startMs + row.until_ms < 0) continue;
      const rect = timing.toRecorded(row.rect);
      if (!rect) throw new Error(`no region rect for always row "${row.id}"`);
      const found = clip.events.filter((e) => !used.has(e) && e.kind === row.expect && ViewedClip.meets(e, rect) && e.start_ms <= SHORT_LIVED_MS);
      this.judge(row, found, take);
    }
    for (const row of timing.page.schedule.filter((r) => r.expect !== 'none')) {
      const nominal = timing.startMs + row.t_ms;
      if (nominal < 0 || nominal > clip.endMs) continue;
      const t = timing.start(row);
      const rect = timing.region(row);
      const inTime = row.many
        ? (e) => e.start_ms >= t - clip.frameMs && e.start_ms <= timing.until(row) + SETTLE_MS + clip.frameMs
        : (e) => Math.abs(e.start_ms - t) <= clip.frameMs;
      const found = clip.events.filter((e) => !used.has(e) && e.kind === row.expect && ViewedClip.meets(e, rect) && inTime(e));
      if (!row.many) {
        this.judge(row, found, take);
        continue;
      }
      if (!found.some((e) => Math.abs(e.start_ms - t) <= clip.frameMs)) this.missed.push(row.id);
      take(row.id, found);
    }
    this.unexpected = clip.events.filter((e) => !used.has(e));
    this.used = used;
  }

  /** A row that takes one event: none is a miss, several are duplicates (reported, all taken). */
  judge(row, found, take) {
    if (found.length === 0) this.missed.push(row.id);
    else if (found.length > 1) this.duplicates.push(`${row.id}: ${list(found)}`);
    take(row.id, found);
  }

  /** `true` when row `id` took exactly one event. */
  single(id) {
    return (this.byRow.get(id) ?? []).length === 1;
  }

  get summary() {
    return `missed: ${this.missed.join(', ') || '-'}; duplicate: ${this.duplicates.join('; ') || '-'}; unexpected: ${list(this.unexpected)}`;
  }

  get clean() {
    return this.missed.length === 0 && this.duplicates.length === 0 && this.unexpected.length === 0;
  }
}

/** The inputs of one run: the viewer's output per clip and the page's schedule. */
class D5Inputs {
  constructor(viewerFile, scheduleFile) {
    this.viewer = readJson(viewerFile);
    this.page = readJson(scheduleFile);
    this.viewerFile = viewerFile ?? '(none given)';
    this.scheduleFile = scheduleFile ?? '(none given)';
    this.cache = new Map();
  }

  clip(name) {
    if (!this.viewer) throw new Error(`no input: viewer output ${this.viewerFile}`);
    return new ViewedClip(name, this.viewer[name]);
  }

  get schedule() {
    if (!this.page || !Array.isArray(this.page.schedule)) throw new Error(`no input: schedule in ${this.scheduleFile}`);
    return this.page;
  }

  /** The proof clip on the proof run's timing. */
  get proof() {
    if (!this.cache.has('proof')) {
      const clip = this.clip('proof');
      this.cache.set('proof', new RunTiming(clip, this.schedule, this.schedule.actual ?? []));
    }
    return this.cache.get('proof');
  }

  get match() {
    if (!this.cache.has('match')) this.cache.set('match', new ScheduleMatch(this.proof));
    return this.cache.get('match');
  }

  /** A boundary clip on its own run's timing. */
  boundary(name) {
    const clip = this.clip(name);
    return new RunTiming(clip, this.schedule, clip.require('actual'));
  }
}

/** #293-4: the boundary row's change, `quietMs` before the clip end, has `outcome`. */
function boundaryCheck(inputs, name, quietMs, outcome) {
  const timing = inputs.boundary(name);
  const found = timing.near(timing.row(ROW.boundary), 'change');
  if (found.length !== 1) return { value: `1 change, quiet ${quietMs} ms`, measured: `${found.length} changes: ${list(found)}`, pass: false };
  const quiet = timing.clip.endMs - found[0].end_ms;
  const ok = quiet === quietMs && outcome.test(found[0].text ?? '');
  return { value: `quiet ${quietMs} ms, ${outcome.source}`, measured: `quiet ${quiet} ms, "${found[0].text}"`, pass: ok };
}

/** The checks by scenario title. */
function checks(inputs) {
  return {
    'every scheduled event reported'() {
      const match = inputs.match;
      return { value: 'all matched, none extra', measured: match.summary, pass: match.clean };
    },

    'still period gives no event'() {
      const proof = inputs.proof;
      const row = proof.row(ROW.still);
      const from = proof.start(row);
      const found = proof.clip.events.filter((e) => e.start_ms >= from && e.start_ms < proof.until(row));
      return { value: 0, measured: `${found.length} events: ${list(found)}`, pass: found.length === 0 };
    },

    'still clip gives no event'() {
      const still = inputs.clip('still');
      return { value: 0, measured: `${still.events.length} events: ${list(still.events)}`, pass: still.events.length === 0 };
    },

    'lone spinner gives exactly one animation'() {
      const spinner = inputs.clip('spinner');
      const animations = spinner.count('animating');
      const others = spinner.events.length - animations;
      return { value: '1, 0', measured: `${animations}, ${others}`, pass: animations === 1 && others === 0 };
    },

    'keyframe instants give no event'() {
      const proof = inputs.proof;
      const match = inputs.match;
      const still = inputs.clip('still');
      const atKeyframe = (clip, exempt) => {
        const keys = clip.require('keyframes_ms');
        if (!Array.isArray(keys) || !keys.some((k) => k > clip.frameMs)) throw new Error(`${clip.name}: no keyframe after the first instant, so nothing is tested`);
        if (keys.some((k) => !finite(k) || k < 0 || k > clip.endMs)) throw new Error(`${clip.name}: a keyframe lies outside the clip`);
        return clip.events.filter((e) => e.start_ms > clip.frameMs && !exempt.has(e)
          && keys.some((k) => Math.abs(e.start_ms - k) <= clip.frameMs));
      };
      const found = [...atKeyframe(proof.clip, match.used), ...atKeyframe(still, new Set())];
      return { value: 0, measured: `${found.length} unscheduled events at keyframes: ${list(found)}`, pass: found.length === 0 };
    },

    'flash clip gives one FLASH per flash'() {
      const flash = inputs.clip('flash');
      const shown = flash.require('shown_ms');
      if (!Array.isArray(shown) || shown.length === 0 || !shown.every(finite)) throw new Error('no input: flash.shown_ms');
      const used = new Set();
      const missed = shown.filter((t) => {
        const found = flash.events.filter((e) => e.kind === 'short_lived' && !used.has(e) && Math.abs(e.start_ms - t) <= flash.frameMs);
        found.forEach((e) => used.add(e));
        return found.length !== 1;
      });
      const other = flash.events.filter((e) => !used.has(e));
      return { value: `${shown.length} FLASH, nothing else`, measured: `unmatched flashes: ${missed.join(', ') || '-'}; other events: ${list(other)}`, pass: missed.length === 0 && other.length === 0 };
    },

    '#293-1 a fade is one change from its start'() {
      const match = inputs.match;
      const taken = match.byRow.get(ROW.fade) ?? [];
      return { value: '1 change within one instant', measured: list(taken), pass: match.single(ROW.fade) };
    },

    '#293-2 an animation that stays SAME to its anchor'() {
      const proof = inputs.proof;
      const match = inputs.match;
      const row = proof.row(ROW.alternate);
      const rect = proof.region(row);
      const others = proof.clip.events.filter((e) => e.kind !== 'animating' && ViewedClip.meets(e, rect)
        && e.start_ms >= proof.start(row) && e.start_ms < proof.until(row));
      return { value: '1 animating, 0 others', measured: `${list(match.byRow.get(row.id) ?? [])}; others ${list(others)}`, pass: match.single(row.id) && others.length === 0 };
    },

    '#293-3 a change is pictured at its confirmed settle frame'() {
      const clip = inputs.proof.clip;
      const picks = clip.require('picks');
      const settleInstants = Math.round(SETTLE_MS / clip.frameMs);
      const settled = clip.events.filter((e) => e.kind === 'change' && /settled at/.test(e.text ?? ''));
      const bad = settled.filter((e) => {
        const confirmed = CONFIRMED.exec(e.text);
        if (!confirmed) return true;
        const frame = Number(confirmed[1]);
        const lowest = Math.min(e.end_frame + settleInstants, clip.lastFrame);
        const pictured = picks.some((p) => p.frame === frame && p.reason.includes(SETTLED_PICK));
        return !(frame >= lowest && frame <= clip.lastFrame && pictured);
      });
      return { value: 'pick at the confirmed frame, >= min(end + K_s, last frame)', measured: `${settled.length - bad.length}/${settled.length}; wrong: ${bad.map((e) => e.end_frame).join(', ') || '-'}`, pass: settled.length > 0 && bad.length === 0 };
    },

    '#293-4a quiet one instant less than SETTLE_MS before the clip end is not settled'() {
      const frameMs = inputs.clip('boundary_below').frameMs;
      return boundaryCheck(inputs, 'boundary_below', SETTLE_MS - frameMs, /still changing when the clip ended/);
    },

    '#293-4b quiet exactly SETTLE_MS before the clip end is settled'() {
      return boundaryCheck(inputs, 'boundary_at', SETTLE_MS, /settled at/);
    },

    '#293-5 only the animated stretch is exempt from FLASH'() {
      const proof = inputs.proof;
      const match = inputs.match;
      const row = proof.row(ROW.alternate);
      const rect = proof.region(row);
      const inStretch = proof.clip.events.filter((e) => e.kind === 'short_lived' && ViewedClip.meets(e, rect)
        && e.start_ms >= proof.start(row) && e.start_ms < proof.until(row));
      return { value: '0 in the stretch, 1 later', measured: `${list(inStretch)}; later ${list(match.byRow.get(ROW.darkflash) ?? [])}`, pass: inStretch.length === 0 && match.single(ROW.darkflash) };
    },

    '#293-6 flashes are timed from their presentation'() {
      const page = inputs.schedule;
      const rows = page.schedule.filter((r) => r.expect === 'short_lived');
      const bad = rows.filter((r) => {
        const a = (page.actual ?? []).find((x) => x.name === r.name);
        const times = a ? [a.set_ms, a.shown_ms, a.hide_ms, a.hidden_ms] : [];
        const ordered = times.length === 4 && times.every(finite) && times.every((t, i) => i === 0 || times[i - 1] <= t);
        return !(ordered && a.hide_ms - a.shown_ms >= FLASH_MS);
      });
      return { value: `set <= shown <= hide <= hidden, hide - shown >= ${FLASH_MS} ms`, measured: `${rows.length - bad.length}/${rows.length}; wrong: ${bad.map((r) => r.id).join(', ') || '-'}`, pass: rows.length > 0 && bad.length === 0 };
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
