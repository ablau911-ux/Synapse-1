// Usage: node dyn-youtube.mjs <clip-folder> <youtube-run.json>.
// Driver measurements: ram_peak_delta_bytes, cpu_share_percent, ready_ms,
// profile_processes_left, screenshot_max_difference, video_crop_difference,
// video_crop_ranges [two channel ranges], overview_ok, timeline.
// Driver observations in clip coordinates/ms: video_before, video_after,
// right_column, search_box ([x,y,w,h]); scroll_ms, type_ms, pause_ms; keys [{t_ms}].
import fs from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';
import { fileURLToPath } from 'node:url';
import { runFeature } from './feature.mjs';
const MB=1_000_000, WIDTH=1920, HEIGHT=1080, FPS=20, DURATION=60_000, INSTANTS=1200;
const PAD=24, IOU_MIN=0.80, MOVING_SHARE=0.95, RAM_LIMIT=500*MB, CPU_LIMIT=10;
const MOVING_RATE=535*MB, EXACT_ALLOWANCE=9.4*MB, INDEX_ALLOWANCE=0.3*MB, START_PICTURES=4, CHANNELS=3;
const KEY_COUNT=12, TYPE_DELAY=300, SCROLL_DELAY=2000, HOLD_SLACK=950, READY_LIMIT=1000;
const [folder,runFile]=process.argv.slice(2);
try {
  const clip=JSON.parse(fs.readFileSync(path.join(folder,'clip.json'))), run=JSON.parse(fs.readFileSync(runFile));
  const lines=fs.readFileSync(path.join(folder,'frames.jsonl'),'utf8').trim().split('\n').map(JSON.parse);
  const segments=clip.moving.segments, byIndex=new Map(segments.map(s=>[s.index,s]));
  const rect=l=>l?.mv?byIndex.get(l.mv[0])?.rect:null;
  const at=ms=>lines[Math.floor(ms*FPS/1000)];
  const intersection=(a,b)=>!a||!b?0:Math.max(0,Math.min(a[0]+a[2],b[0]+b[2])-Math.max(a[0],b[0]))*Math.max(0,Math.min(a[1]+a[3],b[1]+b[3])-Math.max(a[1],b[1]));
  const area=r=>r?r[2]*r[3]:0, iou=(a,b)=>intersection(a,b)/(area(a)+area(b)-intersection(a,b));
  const contains=(a,b)=>a&&b&&a[0]<=b[0]&&a[1]<=b[1]&&a[0]+a[2]>=b[0]+b[2]&&a[1]+a[3]>=b[1]+b[3];
  const bands=new Map();let group=-1,decoded,offset=0;
  for(const l of lines){if(!l.x)continue;if(l.x[0]!==group){group=l.x[0];offset=0;decoded=zlib.zstdDecompressSync(fs.readFileSync(path.join(folder,'frames',`group-${String(group).padStart(6,'0')}.zst`)));}
    const raw=decoded.subarray(offset,offset+l.x[3]);offset+=l.x[3];const count=raw.readUInt16LE(0),rows=[];
    for(let b=0;b<count;b++)rows.push([raw.readUInt16LE(2+b*4),raw.readUInt16LE(4+b*4)]);bands.set(l.instant,rows);
  }
  const before=lines.filter(l=>l.t_ms>=1000&&l.t_ms<run.scroll_ms), moving=lines.filter(l=>l.mv), last=moving.at(-1), lastRect=rect(last);
  const stored=['clip.json','frames.jsonl',...['frames','moving'].flatMap(d=>fs.readdirSync(path.join(folder,d)).map(n=>d+'/'+n))];
  const bytes=stored.reduce((n,f)=>n+fs.statSync(path.join(folder,f)).size,0);
  const bound=segments.reduce((n,s)=>n+MOVING_RATE*area(s.rect)/(WIDTH*HEIGHT)*s.frames/INSTANTS+START_PICTURES*area(s.rect)*CHANNELS,EXACT_ALLOWANCE+INDEX_ALLOWANCE);
  const report=(pass,measured)=>({pass:Boolean(pass),value:true,measured:JSON.stringify(measured)});
  const check={
    'The clip is complete':()=>report(clip.state==='ready'&&clip.stop_reason==='completed'&&clip.recorded_ms===DURATION&&lines.length===INSTANTS,[clip.state,clip.recorded_ms,lines.length]),
    'Nothing was lost':()=>report(clip.lost.length===0&&clip.conditions.c.status==='held'&&clip.conditions.t.status==='held',clip.lost),
    'The test ran at 1080p':()=>report(clip.geometry.window_width>=1900&&clip.geometry.window_height>=1000,clip.geometry),
    'RAM':()=>report(clip.geometry.window_width>=1900&&clip.geometry.window_height>=1000&&Number.isFinite(run.ram_peak_delta_bytes)&&run.ram_peak_delta_bytes<=RAM_LIMIT,run.ram_peak_delta_bytes),
    'CPU':()=>report(Number.isFinite(run.cpu_share_percent)&&run.cpu_share_percent<=CPU_LIMIT&&clip.conditions.b.status==='held',run.cpu_share_percent),
    'The encoder':()=>report(clip.moving.status==='available'&&clip.moving.encoder?.hardware&&clip.moving.encoder.quality_level===90&&clip.conditions.q.status==='held',clip.moving),
    'The video moves':()=>report(before.length>0&&before.filter(l=>l.mv).length/before.length>=MOVING_SHARE,before.filter(l=>l.mv).length/before.length),
    'The box is the video':()=>{const v=run.video_before;return report(v&&iou(rect(at(5000)),v)>=IOU_MIN&&contains([v[0]-PAD,v[1]-PAD,v[2]+2*PAD,v[3]+2*PAD],rect(at(5000))),rect(at(5000)));},
    'Text beside the video is exact':()=>report(run.right_column&&before.length>0&&before.every(l=>intersection(rect(l),run.right_column)===0),before.length),
    'The scroll is stored':()=>report(lines.some(l=>l.x&&l.t_ms>=run.scroll_ms&&l.t_ms<=run.scroll_ms+SCROLL_DELAY),run.scroll_ms),
    'After the scroll the box is the video again':()=>report(iou(rect(at(run.scroll_ms+3000)),run.video_after)>=IOU_MIN,rect(at(run.scroll_ms+3000))),
    'Typing is exact and does not move':()=>report(run.keys?.length===KEY_COUNT&&run.search_box&&run.keys.every(k=>lines.some(l=>l.t_ms>=k.t_ms&&l.t_ms<=k.t_ms+TYPE_DELAY&&(bands.get(l.instant)||[]).some(([y,h])=>y<run.search_box[1]+run.search_box[3]&&y+h>run.search_box[1])))&&JSON.stringify(rect(at(run.type_ms-1000)))===JSON.stringify(rect(at(run.keys.at(-1).t_ms+1000))),run.keys),
    'The stop':()=>report(last&&last.t_ms<=run.pause_ms+HOLD_SLACK,last?.t_ms),
    'One exact picture after the stop':()=>report(lastRect&&Array.from({length:lastRect[3]},(_,i)=>lastRect[1]+i).every(y=>(bands.get(last.instant+1)||[]).some(([first,n])=>y>=first&&y<first+n)),bands.get(last?.instant+1)),
    'Nothing changes nothing written':()=>{const bad=lines.filter((l,i)=>!l.new&&l.x&&JSON.stringify(rect(l))===JSON.stringify(rect(lines[i-1])));return report(bad.length===0,bad);},
    'Sample count':()=>report(segments.length>0&&segments.every(s=>s.complete&&s.frames===s.last_instant-s.first_instant+1),segments.map(s=>s.frames)),
    'Size bound':()=>report(bytes<=bound,{bytes,bound}),
    'The picture is the screen':()=>report(run.screenshot_max_difference===0,run.screenshot_max_difference),
    'The viewer sees it':()=>{const t=run.timeline||[];const meets=(e,r)=>intersection(e.bbox?[e.bbox.x,e.bbox.y,e.bbox.w,e.bbox.h]:null,r)>0;return report(run.overview_ok&&t.some(e=>e.kind==='animating'&&meets(e,run.video_before))&&t.some(e=>e.kind==='change'&&e.start_ms>=run.scroll_ms-50&&e.start_ms<=run.scroll_ms+SCROLL_DELAY)&&t.some(e=>e.kind==='change'&&meets(e,run.search_box)&&e.start_ms>=run.type_ms&&e.start_ms<=run.keys?.at(-1)?.t_ms+TYPE_DELAY),t);},
    'The video is in the clip':()=>report(run.video_crop_difference>0&&run.video_crop_ranges?.length===2&&run.video_crop_ranges.every(n=>n>0),[run.video_crop_difference,run.video_crop_ranges]),
    'Ready time':()=>report(Number.isFinite(run.ready_ms)&&run.ready_ms<=READY_LIMIT,run.ready_ms),
    'The window is closed':()=>report(run.profile_processes_left===0,run.profile_processes_left),
  };
  process.exitCode=runFeature('dyn-youtube',path.join(path.dirname(fileURLToPath(import.meta.url)),'dyn-youtube.feature'),check);
}catch(error){console.error(`FAIL youtube proof input: ${error.message}`);process.exitCode=1;}
