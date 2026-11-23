import fs from 'node:fs';
import path from 'node:path';
const folder = process.argv[2], logPath = process.argv[3];
let failed = 0;
function check(name, ok, value) { console.log(`${ok ? 'PASS' : 'FAIL'} ${name}: ${JSON.stringify(value)}`); if (!ok) failed++; }
try {
  const clip = JSON.parse(fs.readFileSync(path.join(folder, 'clip.json')));
  const lines = fs.readFileSync(path.join(folder, 'frames.jsonl'), 'utf8').trim().split('\n').map(JSON.parse);
  const log = logPath ? JSON.parse(fs.readFileSync(logPath)) : {};
  const segments = clip.moving?.segments || [];
  check('format and ready', clip.way === 'DYN1' && clip.schema_version === 3 && clip.state === 'ready' && clip.geometry.scale === 1, [clip.way, clip.schema_version, clip.state, clip.geometry]);
  const allowed = /^(clip\.json(?:\.lock)?|recorder\.lock|frames\.jsonl|frames|moving|views)$/;
  check('folder names', fs.readdirSync(folder).every(n => allowed.test(n)), fs.readdirSync(folder));
  check('all instants', lines.every((l,i) => l.instant === i), lines.length);
  let previous;
  for (const l of lines) {
    if (l.x) {
      const [group,offset,bytes] = l.x;
      const size = fs.statSync(path.join(folder, 'frames', `group-${String(group).padStart(6,'0')}.zst`)).size;
      check(`exact ${l.instant}`, offset + bytes <= size && (!previous || previous[0] !== group ? offset === 0 : offset > previous[1]), l.x);
      previous = l.x;
    }
    if (l.mv) {
      const s = segments.find(s => s.index === l.mv[0]);
      check(`moving ${l.instant}`, s && l.mv[1] === l.instant - s.first_instant && (l.instant !== s.first_instant || JSON.stringify(l.mv_rect) === JSON.stringify(s.rect)), l.mv);
    }
  }
  const names = ['clip.json','frames.jsonl',...fs.readdirSync(path.join(folder,'frames')).map(n=>'frames/'+n),...fs.readdirSync(path.join(folder,'moving')).map(n=>'moving/'+n)];
  const bytes = names.reduce((sum,n) => sum+fs.statSync(path.join(folder,n)).size,0);
  check('stored bytes', clip.conditions.t.detail.bytes === bytes, [clip.conditions.t.detail.bytes,bytes]);
  check('no pending segments', !names.some(n=>n.endsWith('.part')), names.filter(n=>n.endsWith('.part')));
  check('conditions', ['c','g','b','t'].every(k=>clip.conditions[k]?.status === 'held') && clip.conditions.q?.status === (segments.length?'held':'exact') && clip.lost.length === 0, clip.conditions);
  check('live ready time', Number.isFinite(log.ready_ms) && log.ready_ms <= 1000, log.ready_ms);
  check('idempotent stop', log.stop_twice_equal === true && log.status_has_frames_waiting === false, log);
  if (clip.recorded_ms >= 70000) check('60 second split', segments.length === 2 && segments[0].frames === 60000*clip.fps/1000 && segments[1].first_instant === segments[0].last_instant+1, segments);
} catch(error) { check('readable dynamic folder',false,error.message); }
console.log(`dyn-folder: ${failed} failed`); process.exitCode = failed ? 1 : 0;
