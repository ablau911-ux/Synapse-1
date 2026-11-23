// Three sequential repeats. Every child gets its own fixture/output directory.
import fs from 'node:fs';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

const REPEATS = 3;
const [group = 'core'] = process.argv.slice(2);
const root = process.cwd(), results = path.join(root, 'scripts/clip-equation/results');
const exe = path.join(root, 'target/release/synapse-mcp.exe');
const probe = path.join(root, 'scripts/clip-equation/probe/target/release/clip-probe.exe');
const summary = [];
const run = (label, args, env = {}) => {
  const output = path.join(results, `${label}.txt`);
  const fd = fs.openSync(output, 'w');
  const started = Date.now();
  const child = spawnSync('node', args, { cwd: root, stdio: ['ignore', fd, fd], env: { ...process.env, ...env } });
  fs.closeSync(fd);
  const row = { label, exit: child.status, elapsed_ms: Date.now()-started, error: child.error?.message };
  summary.push(row);
  fs.writeFileSync(path.join(results, `final-${group}-summary.json`), JSON.stringify(summary, null, 2));
  console.log(JSON.stringify(row));
};
if (group === 'core') {
  for (const check of ['dyn-exact', 'dyn-moving', 'dyn-motion']) for (let repeat=1; repeat<=REPEATS; repeat++)
    run(`final-${check}-${repeat}`, ['scripts/clip-equation/spec/run-spec.mjs', check, `out=${path.join(root, '.runs', `final-${check}-${repeat}`)}`]);
} else if (group === 'viewer') {
  for (let repeat=1; repeat<=REPEATS; repeat++) {
    const fixture = path.join(root, '.runs', `final-dyn-motion-${repeat}`);
    run(`final-dyn-seek-${repeat}`, ['scripts/clip-equation/spec/dyn-seek.mjs', probe, path.join(fixture, 'noise')]);
    run(`final-dyn-seek-video-${repeat}`, ['scripts/clip-equation/spec/dyn-seek.mjs', probe, path.join(fixture, 'video')]);
    run(`final-dyn-viewer-${repeat}`, ['scripts/clip-equation/spec/dyn-viewer.mjs', exe, path.join(fixture, 'noise'), path.join(fixture, 'sources/noise'), path.join(root, '.runs', `final-viewer-${repeat}`)]);
    const corpus=JSON.parse(fs.readFileSync(path.join(root,'scripts/clip-equation/corpus/corpus.json'))).root;
    run(`final-dyn-viewer-video-${repeat}`, ['scripts/clip-equation/spec/dyn-viewer.mjs', exe, path.join(fixture, 'video'), path.join(corpus,'video-1920x1080'), path.join(root, '.runs', `final-viewer-video-${repeat}`), 'video']);
  }
} else if (group === 'live') {
  for (const [name,scene,duration,kill] of [['ui','ui',20000,0],['video','video',20000,0],['long','video',70000,0],['recovery','video',20000,8000]])
    for (let repeat=1; repeat<=REPEATS; repeat++) {
      const label=`final-dyn-${name}-${repeat}`, out=path.join(root,'.runs',label);
      run(label,['scripts/clip-equation/spec/dyn-live.mjs',exe,out,scene,String(duration),String(kill)]);
    }
} else if (group === 'd5') {
  for (let repeat=1; repeat<=REPEATS; repeat++) {
    const out=path.join(root,'.runs',`final-d5-${repeat}`);
    run(`final-d5-${repeat}`,['scripts/clip-equation/live-test/d5-proof.mjs',exe,out],{LOCALAPPDATA:path.join(out,'local')});
    const videoOut=path.join(out,'dynamic-video');
    run(`final-d5-video-${repeat}`,['scripts/clip-equation/spec/dyn-live.mjs',exe,videoOut,'d5-video','25000','0']);
    // Adapt the unchanged legacy driver's output using the stored DYN1 index.
    const outputFile=path.join(out,'viewer-output.json');
    if (fs.existsSync(outputFile)) {
      const output=JSON.parse(fs.readFileSync(outputFile));
      for (const clip of Object.values(output)) if (clip.folder && clip.keyframes_ms) {
        const meta=JSON.parse(fs.readFileSync(path.join(clip.folder,'clip.json')));
        const lines=fs.readFileSync(path.join(clip.folder,'frames.jsonl'),'utf8').trim().split('\n').map(JSON.parse);
        const keys=new Set(lines.filter(l=>l.x?.[1]===0).map(l=>l.instant));
        for (const segment of meta.moving.segments) for (const [frame] of segment.key_frames) keys.add(segment.first_instant+frame);
        clip.keyframes_ms=[...keys].sort((a,b)=>a-b).map(k=>k*1000/meta.fps);
      }
      const log=JSON.parse(fs.readFileSync(path.join(videoOut,'log.json')));
      if (log.overview_ok) {
        const meta=JSON.parse(fs.readFileSync(path.join(log.folder,'clip.json')));
        const analysis=JSON.parse(fs.readFileSync(path.join(log.folder,'views/overview_analysis.json')));
        output.video={timeline:analysis.timeline,fps:meta.fps,frame_ms:1000/meta.fps,folder:log.folder};
      }
      fs.writeFileSync(outputFile,JSON.stringify(output,null,2));
      run(`final-d5-combined-${repeat}`,['scripts/clip-equation/spec/run-spec.mjs','d5',outputFile,path.join(out,'schedule.json')]);
    }
  }
} else throw Error(`unknown check group ${group}`);
process.exitCode=summary.some(r=>r.exit!==0)?1:0;
