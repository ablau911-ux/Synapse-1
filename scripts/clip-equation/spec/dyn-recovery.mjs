import fs from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';
const folder=process.argv[2], logPath=process.argv[3];
let failed=0;
function check(name,pass,value){ console.log(`${pass?'PASS':'FAIL'} ${name}: ${JSON.stringify(value)}`); if(!pass)failed++; }
try {
  const clip=JSON.parse(fs.readFileSync(path.join(folder,'clip.json')));
  const lines=fs.readFileSync(path.join(folder,'frames.jsonl'),'utf8').trim().split('\n').map(JSON.parse);
  const log=logPath?JSON.parse(fs.readFileSync(logPath)):{};
  check('recovered state',clip.state==='ready'&&clip.stop_reason==='service_restart',[clip.state,clip.stop_reason]);
  const lost=new Set(clip.lost.map(l=>l.instant));
  check('losses unique',lost.size===clip.lost.length,clip.lost.length);
  const missing=[];for(let k=lines.at(-1).instant+1;k<clip.end_ms*clip.fps/1000;k++)if(!clip.lost.some(l=>l.instant===k&&l.cause==='service_restart'))missing.push(k);
  check('planned tail named',missing.length===0,missing);
  check('no partial files',!fs.readdirSync(path.join(folder,'moving')).some(n=>n.endsWith('.part')),fs.readdirSync(path.join(folder,'moving')));
  const groups=Map.groupBy(lines.filter(l=>l.x),l=>l.x[0]);
  for(const [group,records]of groups){
    const packed=fs.readFileSync(path.join(folder,'frames',`group-${String(group).padStart(6,'0')}.zst`));
    const decoder=zlib.createZstdDecompress();let raw=0;
    decoder.on('data',b=>raw+=b.length);
    const error=new Promise(resolve=>decoder.on('error',resolve));
    for(const l of records)decoder.write(packed.subarray(l.x[1],l.x[1]+l.x[2]));
    await new Promise((resolve,reject)=>decoder.flush(zlib.constants.ZSTD_e_flush,e=>e?reject(e):resolve()));
    decoder.destroy();
    check(`group ${group} records decode`,raw===records.reduce((n,l)=>n+l.x[3],0),raw);
    void error;
  }
  const segments=clip.moving.segments;
  check('open segment accounted',lines.filter(l=>l.mv).every(l=>segments.some(s=>s.index===l.mv[0]&&l.mv[1]<s.frames)||lost.has(l.instant)),segments.map(s=>({frames:s.frames,complete:s.complete})));
  check('overview succeeds',log.overview_ok===true,log.overview_ok);
}catch(error){check('recovery readable',false,error.message);}
console.log(`dyn-recovery: ${failed} failed`);process.exitCode=failed?1:0;
