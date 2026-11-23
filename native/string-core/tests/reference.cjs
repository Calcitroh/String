// Run the shipped modules with a small filesystem host for compatibility tests.
'use strict';
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const { execFileSync } = require('node:child_process');
const root = path.resolve(__dirname, '../../..');
const scripts = path.join(root, 'src/wwwroot');
const stats = require(path.join(scripts, 'stats.js'));
const tags = require(path.join(scripts, 'tags.js'));
const plain = x => JSON.parse(JSON.stringify(x));

function media() {
  const dir = path.join(root, 'test-runs/string-media');
  if (!fs.existsSync(path.join(dir, 'Loose/098. no tags at all.mp3'))) {
    execFileSync(process.execPath, [path.join(root, 'tests/test-tags.js')], { stdio: ['ignore', 'pipe', 'inherit'] });
  }
  return dir;
}
async function song(p) {
  const bytes = fs.readFileSync(p);
  const read = async (o,n) => bytes.subarray(o,o+n);
  const t = await tags.readTags(read,bytes.length);
  const pic = t.art && await tags.readArt(read,bytes.length);
  const hasCover = !!t.art;
  delete t.art;
  return {path:p, tags:t, hasCover, cover:pic?.mime || null, lyrics:await tags.readLyrics(read,bytes.length)};
}
async function readMedia() {
  const dir = media();
  const results = [];
  async function walk(folder) {
    for (const e of fs.readdirSync(folder, { withFileTypes: true })) {
      const p = path.join(folder,e.name);
      if (e.isDirectory()) { await walk(p); continue; }
      if (!/\.(mp3|flac|m4a|ogg|opus|wav)$/.test(e.name)) continue;
      results.push(await song(p));
    }
  }
  await walk(dir);
  return results;
}

function page(dir) {
  let pending;
  const el = {addEventListener(){}, querySelector(){return el;}, querySelectorAll(){return [];}, classList:{contains(){return false;}}};
  const bridge = {
    inBelt:true, load:()=>new Promise(()=>{}), icon:()=>'', onLook(){}, onUpdate(){}, onHide(){}, onShow(){},
    onFlush(){}, onCloseBlocked(){}, onMedia(){},
    native:{ paths:async()=>({data:dir}), readText:async()=>fs.readFileSync(path.join(dir,'library.json'),'utf8'),
      writeText:async(_,text)=>fs.writeFileSync(path.join(dir,'library.json'),text),
      list:async()=>[{name:'ONE.Mp3',size:100,mtime:123}],mediaUrl:p=>p }
  };
  const c = vm.createContext({console, belt:bridge, document:el, addEventListener(){},
    Date:class extends Date { static now(){return 12345678;} },
    setTimeout(fn){pending=fn;return 1;}, clearTimeout(){}, matchMedia:()=>({matches:false}),
    N:{art:{source:()=>null},player:{queueItems:()=>c.N.S.queue.items}},
    StringTags:{readTags:async()=>({title:'One',duration:4.5,format:'mp3',art:{o:8,n:6,m:'image/png'}})},
    StringStats:stats, navigator:{}, innerWidth:800,innerHeight:600});
  c.window=c;
  for (const name of ['library','string']) vm.runInContext(fs.readFileSync(path.join(scripts,name+'.js'),'utf8'),c,{filename:name});
  return {N:c.N,host:c,flush:()=>pending?.()};
}

async function scanMedia(dir) {
  const {N,host,flush} = page(dir);
  const root = media();
  N.S.roots = [root];
  host.belt.native.list = async p => fs.readdirSync(p,{withFileTypes:true}).map(e=>{
    const st=fs.statSync(path.join(p,e.name),{bigint:true});
    return {name:e.name,dir:e.isDirectory(),size:Number(st.size),mtime:Number(st.mtimeNs/1000000n)};
  });
  const reader = file => { const b=fs.readFileSync(file); return async (o,n) => b.subarray(o,o+n); };
  N.rangeReader=reader;
  let reads=0;
  host.StringTags={readTags(...args){reads++;return tags.readTags(...args);}};
  if (fs.existsSync(path.join(dir,'library.json'))) await N.lib.loadCache();
  await N.lib.scan(); await flush();
  const tracks=[];
  for (const t of N.lib.tracks.values()) {
    const saved={id:t.id,path:t.path,size:t.size,mtime:t.mtime,added:t.added,title:t.T};
    if (t.art) {
      const read=reader(t.path), pic=await tags.readArt(read,t.size);
      if (t.art.o!=null) require('node:assert/strict').deepEqual(await read(t.art.o,t.art.n),Buffer.from(pic.bytes));
      saved.coverMime=pic.mime;
    }
    tracks.push(saved);
  }
  tracks.sort((a,b)=>a.id.localeCompare(b.id));
  return {reads,tracks};
}

async function profile(mode,dir) {
  const {N,flush} = page(dir);
  const file = name => path.join(dir,name+'.json');
  if (mode==='write') {
    const id = 'm:\\music\\one.mp3', b='m:\\music\\😀.mp3', c='m:\\music\\😁.mp3';
    const ctx = {kind:'album',key:'second',name:'Two'};
    Object.assign(N.S, {roots:['M:\\Music'], hidden:{[id]:1},playlists:[{id:'p',items:[{id:b}]}], extension:{nested:['α',false,1.25]}});
    Object.assign(N.S.queue,{items:[{id,m:0},{id:b,m:1},{id:c,m:0},{id:b,m:0}], at:1,pos:12.125,
      ctx,orig:[c,b],listStart:1,past:[{start:0,ctx:{kind:'liked',key:''}}],shuffle:true,custom:'preserve'});
    fs.writeFileSync(file('state'), JSON.stringify(N.snapshot()));
    const m = stats.create(null);
    const date = new Date(2026,2,8,3);
    m.add({id:b,title:'Two',artist:'Artist'},0.25,date);
    m.addPhone({id:c,title:'Three',artist:'Artist'},210.5,date);
    fs.writeFileSync(file('listening'),JSON.stringify(m.doc));
    await N.lib.scan();
    await flush();
  } else {
    N.S = N.mergeState(N.S,JSON.parse(fs.readFileSync(file('state'),'utf8')));
    const m = stats.create(JSON.parse(fs.readFileSync(file('listening'),'utf8')));
    await N.lib.loadCache();
    const rows=Array.from(N.lib.tracks.values(),t=>({id:t.id,title:t.T}));
    N.lib.saveCache(); await flush();
    return {state:plain(N.S),snapshot:plain(N.snapshot()),listening:m.doc,library:JSON.parse(fs.readFileSync(file('library'),'utf8')),rows};
  }
}
(async () => {
  const [mode,dir]=process.argv.slice(2);
  let result;
  switch (mode) {
    case 'compare': {
      const read = name => JSON.parse(fs.readFileSync(path.join(dir,name),'utf8'));
      const assert = require('node:assert/strict');
      assert.deepEqual(read('before'),read('after'));
      assert.deepEqual(read('original-state'),read('state.json'));
      break;
    }
    case 'media': result=media(); break;
    case 'tags': result=await readMedia(); break;
    case 'song': result=await song(dir); break;
    case 'scan': result=await scanMedia(dir); break;
    default: result=await profile(mode,dir);
  }
  if (result) process.stdout.write(JSON.stringify(result));
})().catch(e=>{console.error(e);process.exitCode=1;});
