import fs from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';
import { McpServer } from '../live-test/mcp-stdio.mjs';

const [exe, folder, source, out] = process.argv.slice(2);
const LABEL_ROWS = 40, CHANNELS = 3, INSTANT_MS = 50, SEEK_LIMIT = 20;
const SSIM_SIDE = 8, SSIM_STRIDE = 4, SSIM_MIN = 0.95;
const SSIM_C1 = (0.01 * 255) ** 2, SSIM_C2 = (0.03 * 255) ** 2;
let failed=0, longest=0;
const check=(name,ok,value)=>{console.log(`${ok?'PASS':'FAIL'} ${name}: ${JSON.stringify(value)}`);if(!ok)failed++;};
function png(file) {
  const data=fs.readFileSync(file), chunks=[];let width,height,channels;
  for(let p=8;p<data.length;){const n=data.readUInt32BE(p),kind=data.toString('ascii',p+4,p+8),b=data.subarray(p+8,p+8+n);p+=n+12;
    if(kind==='IHDR'){width=b.readUInt32BE(0);height=b.readUInt32BE(4);channels=b[9]===2?3:b[9]===6?4:0;if(b[8]!==8||!channels)throw Error('unsupported PNG');}
    if(kind==='IDAT')chunks.push(b);
  }
  const packed=zlib.inflateSync(Buffer.concat(chunks)),stride=width*channels,raw=Buffer.alloc(stride*height);let p=0;
  for(let y=0;y<height;y++){const filter=packed[p++];for(let x=0;x<stride;x++){
    const a=x>=channels?raw[y*stride+x-channels]:0,b=y?raw[(y-1)*stride+x]:0,c=y&&x>=channels?raw[(y-1)*stride+x-channels]:0;
    const pred=a+b-c,pa=Math.abs(pred-a),pb=Math.abs(pred-b),pc=Math.abs(pred-c);
    const add=[0,a,b,Math.floor((a+b)/2),pa<=pb&&pa<=pc?a:pb<=pc?b:c][filter];if(add===undefined)throw Error('unsupported filter');
    raw[y*stride+x]=(packed[p++]+add)&255;
  }}return {width,height,channels,raw};
}
let server;
function compare(image, bgr, line, region) {
  let mismatches = 0;
  const a = [], b = [];
  for (let y = LABEL_ROWS; y < region.h; y++) for (let x = 0; x < region.w; x++) {
    const p = (y * image.width + x) * image.channels;
    const q = ((y + region.y) * line.width + x + region.x) * CHANNELS;
    for (let c = 0; c < CHANNELS; c++) if (image.raw[p+c] !== bgr[q+CHANNELS-1-c]) mismatches++;
    a.push(Math.round(0.2126*image.raw[p] + 0.7152*image.raw[p+1] + 0.0722*image.raw[p+2]));
    b.push(Math.round(0.2126*bgr[q+2] + 0.7152*bgr[q+1] + 0.0722*bgr[q]));
  }
  const n = SSIM_SIDE ** 2;
  let sum = 0, count = 0;
  for (let y = 0; y + SSIM_SIDE <= region.h-LABEL_ROWS; y += SSIM_STRIDE)
    for (let x = 0; x + SSIM_SIDE <= region.w; x += SSIM_STRIDE) {
      let sa=0,sb=0,saa=0,sbb=0,sab=0;
      for (let row=y; row<y+SSIM_SIDE; row++) for (let col=x; col<x+SSIM_SIDE; col++) {
        const p=a[row*region.w+col],q=b[row*region.w+col];
        sa+=p;sb+=q;saa+=p*p;sbb+=q*q;sab+=p*q;
      }
      const ma=sa/n,mb=sb/n;
      sum+=((2*ma*mb+SSIM_C1)*(2*(sab/n-ma*mb)+SSIM_C2))/((ma*ma+mb*mb+SSIM_C1)*(saa/n-ma*ma+sbb/n-mb*mb+SSIM_C2));
      count++;
    }
  return { mismatches, ssim: sum/count };
}
try {
  const clip=JSON.parse(fs.readFileSync(path.join(folder,'clip.json')));
  const lines=fs.readFileSync(path.join(folder,'frames.jsonl'),'utf8').trim().split('\n').map(JSON.parse);
  const sources=fs.readFileSync(path.join(source,'frames.jsonl'),'utf8').trim().split('\n').map(JSON.parse);
  check('dynamic input',clip.way==='DYN1'&&clip.schema_version===3,clip.way);
  const counts=new Map();let maxExact=0,maxMoving=0;
  for(const l of lines){if(l.x){const n=(counts.get(l.x[0])||0)+1;counts.set(l.x[0],n);maxExact=Math.max(maxExact,n);}if(l.mv){const s=clip.moving.segments.find(s=>s.index===l.mv[0]);const key=s?.key_frames.findLast(k=>k[0]<=l.mv[1]);maxMoving=Math.max(maxMoving,key?l.mv[1]-key[0]+1:Infinity);}}
  check('seek bounds',maxExact<=SEEK_LIMIT&&maxMoving<=SEEK_LIMIT,[maxExact,maxMoving]);
  fs.mkdirSync(out,{recursive:true});
  server=new McpServer({exe,db:path.join(out,'db'),outDir:out,env:{LOCALAPPDATA:path.join(out,'local'),SYNAPSE_SHELL_JOB_ROOT:path.join(out,'shell')}});
  await server.init('dyn-viewer');
  async function frame(instant,region){const start=performance.now();const r=await server.call({operation:'clip_frame',folder,t_ms:instant*INSTANT_MS,region,scale:1});longest=Math.max(longest,performance.now()-start);if(r.error||r.isError||r.images!==1)throw Error(r.texts.join('\n'));const files=fs.readdirSync(out).filter(n=>/^clip_frame-\d+\.png$/.test(n)).sort((a,b)=>Number(a.match(/\d+/)[0])-Number(b.match(/\d+/)[0]));return png(path.join(out,files.at(-1)));}
  const instants=[0,10,40,43,44,60,90,120,134,135];
  for(const instant of instants){const region={x:32,y:64,w:256,h:240};const image=await frame(instant,region),line=sources[instant],bgr=zlib.zstdDecompressSync(fs.readFileSync(path.join(source,`frame-${String(line.frame).padStart(4,'0')}.zst`)));let mismatches=0;
    for(let y=LABEL_ROWS;y<region.h;y++)for(let x=0;x<region.w;x++)for(let c=0;c<CHANNELS;c++)if(image.raw[(y*image.width+x)*image.channels+c]!==bgr[((y+region.y)*line.width+x+region.x)*CHANNELS+CHANNELS-1-c])mismatches++;
    check(`exact outside ${instant}`,mismatches===0,mismatches);
  }
  function compareSource(image, instant, region) {
    const line=sources[instant];
    const bgr=zlib.zstdDecompressSync(fs.readFileSync(path.join(source,`frame-${String(line.frame).padStart(4,'0')}.zst`)));
    return compare(image,bgr,line,region);
  }
  const box={x:400,y:304,w:320,h:240};
  const rendered=[];for(const instant of [44,90,134]) {
    const image=await frame(instant,box);rendered.push(image);
    const result=compareSource(image,instant,box);
    check(`moving quality ${instant}`,result.ssim>=SSIM_MIN,result.ssim);
  }
  check('moving pictures differ',!rendered[0].raw.equals(rendered[1].raw)&&!rendered[1].raw.equals(rendered[2].raw),rendered.map(p=>p.raw.length));
  const stopped=compareSource(await frame(135,box),135,box);
  check('exact after stop',stopped.mismatches===0,stopped.mismatches);
  for(let i=0;i<SEEK_LIMIT;i++)await frame((i*73+19)%lines.length,{x:400,y:304,w:320,h:240});
  const metadataPath=path.join(folder,'clip.json'),original=fs.readFileSync(metadataPath);
  try {
    const missing={...clip,moving:{...clip.moving,segments:[]}};
    fs.writeFileSync(metadataPath,JSON.stringify(missing));
    const fallback=compareSource(await frame(90,box),43,box);
    check('missing segment keeps exact pixels',fallback.mismatches===0,fallback.mismatches);
  } finally { fs.writeFileSync(metadataPath,original); }
  console.log(`longest_seek_ms=${longest}`);
}catch(error){check('viewer readable',false,error.message);}finally{if(server)await server.close();}
console.log(`dyn-viewer: ${failed} failed`);process.exitCode=failed?1:0;
