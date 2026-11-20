// Run the installed page modules against an in-memory library, clock and Windows host.
'use strict';
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const assert = require('node:assert/strict');

function page(options) {
  const c = vm.createContext({ console, setTimeout, clearTimeout, setInterval, clearInterval, ...options });
  c.window = c;
  return c;
}
function load(c, name) {
  vm.runInContext(fs.readFileSync(path.join(__dirname, '../src/wwwroot', name + '.js'), 'utf8'), c, {filename:name});
}

async function run() {
  const queue = { items: [], orig: [], at: -1 };
  const player = page({ N: { S: {queue}, lib: { get: id => ({id}) }, on(){}, emit(){}, save(){} },
    Audio: class { addEventListener(){} }, navigator: {}, belt: {onFlush(){}} });
  load(player,'player');
  const playback = player.N.player;
  playback.addToQueue(['A']); playback.addToQueue(['B']); playback.playNext(['C']);
  assert.deepEqual(Array.from(queue.items, x => x.id), ['C','A','B']);
  console.log('ok   queued songs accumulate before playback, including Play next');
  const original = Array.from({length:1500}, (_,i) => 'track-' + i);
  const legacy = {at:100,items:original.slice(150,1050).map(id => ({id,m:0}))};
  playback.restoreLegacyOrder(legacy, original);
  assert.deepEqual(Array.from(legacy.items, x=>x.id),original);
  assert.equal(legacy.at,250);
  console.log('ok   older unshuffled queue regains both ends and its current song');

  let release;
  const gate = new Promise(r => {release=r;});
  const visited = [];
  const scanner = page({N:{S:{roots:['A'],files:[],edits:{}}},belt:{inBelt:false,native:{
    async list(p){visited.push(p); await gate; return [];}
  }}});
  load(scanner,'library');
  const first=scanner.N.lib.scan();
  scanner.N.S.roots.push('B');
  const second=scanner.N.lib.scan();
  release(); await Promise.all([first,second]);
  assert(visited.includes('B'));
  console.log('ok   folder added during scanning is visited automatically');

  let now=0, seq=0;
  const timers=new Map(), writes=[];
  const clock=page({ N:{}, navigator:{language:'en-US'}, addEventListener(){},
    chrome:{webview:{addEventListener(){},postMessage(m){if(m.kind==='store') writes.push(JSON.parse(JSON.stringify(m.data)));}}},
    setTimeout(fn,ms){const id=++seq; timers.set(id,{fn,at:now+ms});return id;},clearTimeout(id){timers.delete(id);}
  });
  load(clock,'host'); load(clock,'stats'); clock.N.stats.load(null);
  for(let i=0;i<3600;i++){
    now+=1000;
    for(const [id,t] of timers) if(t.at<=now){timers.delete(id);t.fn();}
    clock.N.stats.add({id:'test',T:'Test'},1);
  }
  assert(writes.length>=179);
  clock.N.stats.saveNow();
  assert.equal(Object.values(writes.at(-1).days).reduce((sum,d)=>sum+d.s,0),3600);
  console.log('ok   continuous playback checkpoints throughout the hour and flushes the final seconds');

  // ---- 2026-11-20 review fixes ----
  class FakeAudio {
    constructor() { this.currentTime = 0; this.duration = NaN; this.readyState = 0; this.attrs = {}; }
    addEventListener() { } pause() { } load() { } play() { return Promise.resolve(); }
    set src(v) { this.attrs.src = v; } get src() { return this.attrs.src || ''; }
    getAttribute(k) { return this.attrs[k] ?? null; } removeAttribute(k) { delete this.attrs[k]; }
  }
  const songs = { A: { id: 'A', path: 'A', T: 'A', unplayable: true }, B: { id: 'B', path: 'B', T: 'B', duration: 200 }, C: { id: 'C', path: 'C', T: 'C', duration: 200 }, D: { id: 'D', path: 'D', T: 'D', duration: 200 } };
  const q2 = { items: [], orig: [], at: -1 };
  const deck = page({ N: { S: { queue: q2, loops: {}, recent: [], set: {} }, lib: { get: id => songs[id] }, on() { }, emit() { }, save() { } },
    Audio: FakeAudio, navigator: {}, belt: { onFlush() { }, native: { mediaUrl: p => p } } });
  load(deck, 'player');
  const P2 = deck.N.player;
  P2.playIds(['A', 'B', 'C', 'D'], 1);
  assert.equal(q2.items[q2.at].id, 'B');
  P2.playIds(['A', 'B', 'C', 'D'], 0);   // the row clicked can't play: the next one that can
  assert.equal(q2.items[q2.at].id, 'B');
  P2.playIds(['B', 'A', 'C', 'D'], 2);
  assert.equal(q2.items[q2.at].id, 'C');
  console.log('ok   clicking a song plays that song when an earlier one in the list can\'t play');

  deck.N.S.loops.B = { a: 199.8, b: null };   // ends 0.08 s after it starts: too short to be a loop
  P2.playIds(['B'], 0);
  assert.equal(P2.loop(), null);
  const lp = P2.setLoop(199.9, null);           // a start set at the very end moves back to make a second
  assert(lp && lp.a <= 198.9 && lp.a >= 198.8, JSON.stringify(lp));
  const lp2 = P2.setLoop(null, 0.2);            // an end set at the very start moves on
  assert(lp2 && lp2.b >= 1, JSON.stringify(lp2));
  console.log('ok   a loop is at least a second long, and a too-short saved one counts as no loop');

  // library: a folder that can't be listed keeps its songs; a song whose tags failed is read again
  const tracks = new Map();
  const lib = page({ N: { S: { roots: ['M:\\Music', 'M:\\Locked'], files: [], edits: {} }, art: { source: () => null } },
    StringTags: { readTags: async () => ({ title: 'Real title', format: 'mp3', duration: 100 }) },
    belt: { inBelt: false, native: {
      mediaUrl: p => p,
      async list(p) {
        if (p === 'M:\\Locked') throw new Error('Access is denied');
        return p === 'M:\\Music' ? [{ name: 'busy.mp3', dir: false, size: 10, mtime: 5 }] : [];
      },
    } } });
  load(lib, 'library');
  const L2 = lib.N.lib;
  const mk = (path, extra) => ({ path, id: path.toLowerCase(), name: path.split('\\').pop(), dir: path.slice(0, path.lastIndexOf('\\')), size: 10, mtime: 5, duration: 100, T: 'x', A: '', B: '', ...extra });
  L2.tracks.set('m:\\locked\\kept.mp3', mk('M:\\Locked\\kept.mp3'));
  L2.tracks.set('m:\\music\\busy.mp3', mk('M:\\Music\\busy.mp3', { readError: 1, format: '' }));
  L2.tracks.set('m:\\music\\gone.mp3', mk('M:\\Music\\gone.mp3'));
  const res = await L2.scan();
  assert(L2.tracks.has('m:\\locked\\kept.mp3'), 'song under an unreadable folder was dropped');
  assert(!L2.tracks.has('m:\\music\\gone.mp3'), 'a song really gone stays in the library');
  const busy = L2.tracks.get('m:\\music\\busy.mp3');
  assert.equal(busy.title, 'Real title');
  assert(!busy.readError);
  assert.equal(res.removed, 1);
  console.log('ok   an unreadable folder keeps its songs, and a song whose tags failed is read again');

  // saving: a write the host says failed is kept, tried again, and reported once saved
  let hnow = 0, hseq = 0;
  const htimers = new Map(), sent = [], listeners = [], toasts = [];
  const hostPage = page({ N: {}, navigator: {}, addEventListener() { },
    console: { ...console, log: m => { if (String(m).startsWith('toast: ')) toasts.push(m); else console.log(m); } },
    chrome: { webview: { addEventListener(k, fn) { listeners.push(fn); }, postMessage(m) { sent.push(JSON.parse(JSON.stringify(m))); } } },
    setTimeout(fn, ms) { const id = ++hseq; htimers.set(id, { fn, at: hnow + ms }); return id; }, clearTimeout(id) { htimers.delete(id); } });
  load(hostPage, 'host');
  const hb = hostPage.belt;
  const advance = ms => { hnow += ms; for (const [id, t] of [...htimers]) if (t.at <= hnow) { htimers.delete(id); t.fn(); } };
  const deliver = m => listeners.forEach(fn => fn({ data: m }));
  let made = 0;
  hb.save(() => ({ playlist: 'new', n: ++made }));
  hb.save(() => ({ playlist: 'new', n: ++made }));
  advance(300);
  let saves = sent.filter(m => m.kind === 'save');
  assert.equal(saves.length, 1); assert.equal(made, 1, 'the snapshot is made once per burst, when the save goes out');
  deliver({ kind: 'saved', name: 'state', seq: saves[0].seq, ok: false, error: 'The disk is full' });
  assert.deepEqual([...hb.unsaved()], ['state']);
  assert.equal(toasts.length, 1);
  advance(10000);
  saves = sent.filter(m => m.kind === 'save');
  assert.equal(saves.length, 2); assert.equal(saves[1].data.playlist, 'new');
  deliver({ kind: 'saved', name: 'state', seq: saves[1].seq, ok: true });
  assert.equal(hb.unsaved().length, 0);
  assert(/saved again/.test(toasts.at(-1)));
  console.log('ok   a save the disk refused is kept, tried again, and reported once it works');

  // lyrics: LRCLIB out for a moment doesn't make the song's own unsynced lyrics the answer for good
  let lrclibUp = false;
  const cacheWrites = [];
  const ly = page({ N: { S: { set: { lyricsOnline: true } }, art: { hash: () => 7 }, rangeReader: () => async () => new Uint8Array(0) },
    StringTags: { readLyrics: async () => 'plain line one\nplain line two' },
    belt: { native: {
      paths: async () => ({ data: 'D' }), readText: async () => null, mediaUrl: p => p,
      writeText: async (p, text) => { cacheWrites.push(JSON.parse(text)); },
      lrclib: async () => lrclibUp ? { status: 200, body: JSON.stringify({ syncedLyrics: '[00:01.00] synced', trackName: 'S', artistName: 'X', duration: 100 }) } : { status: 503, body: '' },
    } } });
  load(ly, 'lyrics');
  const song = { id: 's', path: 'S.mp3', T: 'S', A: 'X', B: '', duration: 100, size: 1 };
  const first2 = await ly.N.lyrics.get(song);
  assert.equal(first2.source, 'file'); assert.equal(first2.synced, false); assert(first2.retry);
  assert.equal(cacheWrites.length, 0, 'a failed LRCLIB ask must not be remembered');
  lrclibUp = true;
  const again = await ly.N.lyrics.get(song, { force: true });
  assert.equal(again.source, 'lrclib'); assert.equal(again.synced, true);
  assert.equal(cacheWrites.at(-1).asked, true);
  console.log('ok   lyrics: LRCLIB out for a moment leaves nothing stuck; the synced ones come when it\'s back');
}
// the page modules leave timers running (a loop's watch, a lyrics retry, Discord status), as they would in String
run().catch(e=>{console.error(e);process.exitCode=1;}).finally(()=>setImmediate(()=>process.exit(process.exitCode||0)));
