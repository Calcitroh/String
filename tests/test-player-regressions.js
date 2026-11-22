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
    constructor() { this.currentTime = 0; this.duration = NaN; this.readyState = 1; this.attrs = {}; this.events = {}; }
    addEventListener(name, fn) { this.events[name] = fn; }
    fire(name) { this.events[name]?.(); }
    pause() { } load() { }
    play() {
      if (this.blocked) return Promise.reject(new DOMException('Blocked', 'NotAllowedError'));
      this.fire('playing');
      return Promise.resolve();
    }
    set src(v) { this.attrs.src = v; this.currentTime = 0; } get src() { return this.attrs.src || ''; }
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

  // autoplay & continuous skip: 1-song album, liked songs, all songs, and mid-list shuffle
  const libSongs = {
    s1: { id: 's1', path: 's1.mp3', T: 'Single Track', A: 'Artist A', artistKey: 'artist a', B: 'Album 1', duration: 120 },
    s2: { id: 's2', path: 's2.mp3', T: 'Album Two A', A: 'Artist B', artistKey: 'artist b', B: 'Album 2', duration: 120 },
    s3: { id: 's3', path: 's3.mp3', T: 'Album Two B', A: 'Artist B', artistKey: 'artist b', B: 'Album 2', duration: 120 },
    s4: { id: 's4', path: 's4.mp3', T: 'Same Artist Extra', A: 'Artist A', artistKey: 'artist a', B: '', duration: 120 },
  };
  const albums = new Map([
    ['alb1', { key: 'alb1', name: 'Album 1', ids: ['s1'] }],
    ['alb2', { key: 'alb2', name: 'Album 2', ids: ['s2', 's3'] }],
  ]);
  const q3 = { items: [], orig: [], at: -1, repeat: 'off', shuffle: false, ctx: null };
  const autoPage = page({
    N: {
      S: { queue: q3, loops: {}, recent: [], set: { autoplay: true } },
      lib: { get: id => libSongs[id], all: ['s1', 's2', 's3', 's4'], albums, artists: new Map(), tracks: new Map(Object.entries(libSongs)) },
      on() { }, emit() { }, save() { },
    },
    Audio: FakeAudio, navigator: {}, belt: { onFlush() { }, native: { mediaUrl: p => p } },
  });
  load(autoPage, 'player');
  const P3 = autoPage.N.player;

  // 1-song album -> Skip moves to next album and keeps playing; Previous returns to the 1-song album
  P3.playIds(['s1'], 0, { kind: 'album', key: 'alb1', name: 'Album 1' });
  assert.equal(P3.current().id, 's1');
  P3.next(true);
  assert.equal(P3.current().id, 's2');
  assert.equal(P3.playing, true);
  assert.equal(q3.ctx.key, 'alb2');
  P3.prev();
  assert.equal(P3.current().id, 's1');
  // skipping from the last song of Album 2 wraps to Album 1
  P3.playIds(['s2', 's3'], 1, { kind: 'album', key: 'alb2', name: 'Album 2' });
  P3.next(true);
  assert.equal(P3.current().id, 's1');
  assert.equal(q3.ctx.key, 'alb1');
  console.log('ok   skipping at the end of a single-song or multi-song album moves to the next album and keeps history');

  // Liked songs with 1 song -> Skip continues to library songs (same artist prioritized first)
  P3.playIds(['s1'], 0, { kind: 'liked', key: '', name: 'Liked songs' });
  P3.next(true);
  assert.equal(P3.current().id, 's4');
  assert.equal(P3.playing, true);
  // Liked songs when all library songs are liked -> wraps to another liked song instead of pausing
  P3.playIds(['s1', 's2', 's3', 's4'], 3, { kind: 'liked', key: '', name: 'Liked songs' });
  P3.next(true);
  assert.equal(P3.current().id, 's1');
  assert.equal(P3.playing, true);
  console.log('ok   skipping at the end of Liked songs continues instead of pausing');

  // Songs at the last song -> Skip wraps to the first song instead of pausing
  P3.playIds(['s1', 's2', 's3', 's4'], 3, { kind: 'songs', key: '', name: 'All songs' });
  P3.next(true);
  assert.equal(P3.current().id, 's1');
  assert.equal(P3.playing, true);
  // Turning Shuffle on while at the last song of Songs shuffles all other songs ahead
  P3.playIds(['s1', 's2', 's3', 's4'], 3, { kind: 'songs', key: '', name: 'All songs' });
  P3.setShuffle(true);
  assert.equal(P3.upcoming().length, 3);
  assert(!P3.upcoming().some(x => x.id === 's4'));
  P3.setShuffle(false);
  console.log('ok   Songs wraps on skip and turning shuffle on at the end queues the rest of the list');

  // Autoplay off -> stops at end as before
  autoPage.N.S.set.autoplay = false;
  P3.playIds(['s1'], 0, { kind: 'album', key: 'alb1', name: 'Album 1' });
  P3.next(true);
  assert.equal(P3.playing, false);
  assert.equal(P3.current().id, 's1');
  console.log('ok   turning Autoplay off stops at the end of a list');

  const decks = [], gain = () => ({ value: 1, setTargetAtTime() {}, setValueAtTime() {}, cancelScheduledValues() {}, linearRampToValueAtTime() {} });
  let playbackNow = 0, playbackTimer = 0;
  const pendingTimers = new Map();
  const advancePlayback = ms => {
    playbackNow += ms;
    for (const [id, t] of pendingTimers) if (t.at <= playbackNow) { pendingTimers.delete(id); t.fn(); }
  };
  const auto = page({
    N: {
      S: { queue: { items: [], orig: [], at: -1, repeat: 'off' }, loops: {}, recent: [], plays: {}, set: {} },
      lib: autoPage.N.lib, on() {}, emit() {}, save() {},
    },
    Audio: class extends FakeAudio { constructor() { super(); this.duration = 120; decks.push(this); } },
    AudioContext: class {
      state = 'running';
      async suspend() { this.state = 'suspended'; }
      async resume() { this.state = 'running'; }
      createGain() { return { gain: gain(), connect: () => ({ connect() {} }) }; }
      createAnalyser() { return {}; }
      createMediaElementSource() { return { connect: () => ({ connect() {} }) }; }
    },
    navigator: {}, belt: { onFlush() {}, native: { mediaUrl: p => p } },
    setTimeout(fn, ms = 0) { pendingTimers.set(++playbackTimer, { fn, at: playbackNow + ms }); return playbackTimer; },
    clearTimeout(id) { pendingTimers.delete(id); },
  });
  load(auto, 'player');
  const a = auto.N.player, aq = auto.N.S.queue, as = auto.N.S;
  const album1 = { kind: 'album', key: 'alb1', name: 'Album 1' }, liked = { kind: 'liked', key: '' };
  const flush = async () => { for (let i = 0; i < 5; i++) await Promise.resolve(); };
  const active = () => decks.find(d => d.src === a.current().path);
  const tick = (d, t) => { d.currentTime = t; d.fire('timeupdate'); };
  const ids = () => Array.from(a.upcoming(), x => x.id);

  a.playIds(['s1'], 0, album1);
  await flush();
  const firstDeck = active(), spare = decks.find(d => d !== firstDeck);
  tick(firstDeck, 105);
  assert.equal(spare.src, 's2.mp3', 'autoplay successor is loaded before ended');
  assert.equal(a.current().id, 's1');
  firstDeck.fire('ended');
  await flush();
  assert.equal(active(), spare, 'gapless autoplay uses the preloaded deck');
  a.prev();
  assert.equal(a.current().id, 's1');
  assert.equal(a.context().key, 'alb1');
  a.next();
  a.cycleRepeat();
  a.next();
  a.next();
  assert.equal(a.current().id, 's2', 'Repeat All wraps in the new album');
  console.log('ok   Repeat All stays in the new list and Previous restores the old context');

  aq.repeat = 'off';
  as.set.fade = 5;
  a.playIds(['s1'], 0, album1);
  await flush();
  tick(active(), 116);
  assert.equal(a.current().id, 's2');
  assert.equal(a.crossfading(), true);
  await flush();
  assert.equal(a.history[0], 's2', 'the incoming deck is recorded before the play threshold');
  a.playIds(['s1'], 0, album1);
  await flush();
  tick(active(), 105);
  a.cycleRepeat();
  active().fire('ended');
  assert.equal(a.current().id, 's1', 'enabling repeat cancels a prepared autoplay successor');
  a.cycleRepeat();
  tick(active(), 116);
  assert.equal(a.crossfading(), false, 'Repeat One blocks the autoplay handover');
  a.cycleRepeat();
  a.setAutoplay(false);
  tick(active(), 116);
  assert.equal(a.crossfading(), false, 'Autoplay off blocks the autoplay handover');
  a.setAutoplay(true);
  aq.repeat = 'off';
  as.loops.s1 = { a: 10, b: 110 };
  tick(active(), 105);
  a.next();
  assert.equal(a.current().id, 's1', 'a loop suppresses autoplay even on skip at the boundary');
  delete as.loops.s1;
  console.log('ok   autoplay preloads and crossfades through the normal decks and honors repeat and loops');

  a.playIds(['s1', 's2', 's3', 's4'], 1);
  a.removeAt(2);
  a.playNext(['s3']);
  a.move(2, 4);
  a.setShuffle(true);
  assert(ids().includes('s1'), 'played songs are eligible for the new shuffle');
  assert.equal(ids().filter(id => id === 's3').length, 1, 'only the manually added occurrence remains');
  assert.equal(ids()[1], 's3', 'a manual item keeps its position');
  a.setShuffle(false);
  assert.equal(ids().filter(id => id === 's3').length, 1);
  console.log('ok   shuffle honors explicit removals and manual positions');

  const before = [...a.history];
  decks.forEach(d => { d.blocked = true; });
  a.playIds(['s4'], 0);
  assert.deepEqual([...a.history], before);
  tick(active(), 80);
  assert.equal(as.plays.s4, undefined, 'unconfirmed clock movement cannot populate Recent through Play counts');
  await flush();
  assert.deepEqual([...a.history], before, 'blocked playback does not go in Recent');
  decks.forEach(d => { d.blocked = false; });
  a.restore();
  assert.deepEqual([...a.history], before, 'restoring paused does not go in Recent');
  await a.play();
  assert.equal(a.history[0], 's4');
  console.log('ok   Recent records confirmed playback, including a resumed restored song');

  auto.N.lib.all = ['s1', 's2'];
  a.clearQueue();
  a.playIds(['s1'], 0, liked);
  active().fire('error');
  advancePlayback(350);
  assert.equal(a.current().id, 's2');
  delete libSongs.s1.unplayable;
  console.log('ok   autoplay continues if exactly one other song can play');

  const playedFrom = [], toastsShown = [], globalEvents = {}, nodes = new Map();
  class Element {
    constructor() {
      this.events = {};
      this.dataset = {};
      this.classList = { toggle() {}, contains() { return false; }, add() {}, remove() {} };
      this.style = { setProperty() {} };
      this.nodes = new Map();
      this.scrollTop = 0;
      this.value = '';
    }
    set innerHTML(html) {
      this.html = html;
      this.nodes.clear();
      for (const [re, selector] of [[/id="list"/, '#list'], [/<input[^>]+data-lf\b/, '[data-lf]'], [/<button[^>]+data-lf-x\b/, '[data-lf-x]'], [/id="glow"/, '#glow']]) {
        if (re.test(html)) this.nodes.set(selector, new Element());
      }
      if (html.startsWith('<div class="tl')) {
        this.firstElementChild = new Element();
        for (const selector of ['.tl-h', '.tl-body']) this.firstElementChild.nodes.set(selector, new Element());
      }
    }
    get innerHTML() { return this.html; }
    querySelector(s) { return this.nodes.get(s) || null; }
    querySelectorAll() { return []; }
    addEventListener(name, fn) { this.events[name] = fn; }
    fire(name, e = {}) { this.events[name]?.({ target: this, preventDefault() {}, ...e }); }
    focus() { document.activeElement = this; }
    matches(s) { return s === '[data-lf]' && input() === this; }
    closest(s) { return this.matches(s) || (s.startsWith('input') && input() === this) ? this : null; }
    setSelectionRange(start, end, direction) { Object.assign(this, { selectionStart: start, selectionEnd: end, selectionDirection: direction }); }
    getBoundingClientRect() { return { top: 0, right: 0, bottom: 0, width: 0, height: 0 }; }
    appendChild(el) { if (el.id === 'menu') nodes.set('#menu', el); }
    scrollTo() {}
    remove() {}
  }
  const document = new Element();
  document.body = document.activeElement = new Element();
  document.documentElement = new Element();
  document.createElement = () => new Element();
  document.querySelector = s => {
    if (!/^#[\w-]+$/.test(s)) return null;
    if (!nodes.has(s)) nodes.set(s, new Element());
    return nodes.get(s);
  };
  const view = document.querySelector('#view'), input = () => view.querySelector('[data-lf]');
  for (const song of Object.values(libSongs)) song.q = song.T.toLowerCase();
  const L = { ...auto.N.lib, all: Object.keys(libSongs), loaded: true, coll: new Intl.Collator(), norm: s => s.toLowerCase(),
    ids: ids => ids.filter(id => libSongs[id]), progress: null };
  const ui = page({
    N: { lib: L, on() {}, art: { source() {}, html: () => '<div class="art"></div>', fill() {}, url: async () => null },
      player: { current: () => null, context: () => null, queueItems: () => ui.N.S.queue === aq ? a.queueItems() : ui.N.S.queue.items,
        playIds(ids, i) { playedFrom.push([Array.from(ids), i]); } } },
    belt: { icon: () => '<svg></svg>', load: () => new Promise(() => {}), save() {}, toast: msg => toastsShown.push(msg),
      onLook() {}, onUpdate() {}, onShow() {}, onHide() {}, onFlush() {}, onCloseBlocked() {}, onMedia() {} },
    document, addEventListener: (name, fn) => { globalEvents[name] = fn; }, matchMedia: () => ({ matches: true }),
    StringStats: { calendarDay: () => 0 }, innerWidth: 800, innerHeight: 600,
  });
  load(ui, 'string');
  const U = ui.N.ui, state = ui.N.S;
  state.roots = ['music'];
  const filter = text => { const el = input(); el.value = text; el.fire('input'); };
  const playlist = { id: 'mix', name: 'Mix', items: Object.keys(libSongs).map(id => ({ id })) };
  const press = key => globalEvents.keydown({ target: document.body, key, preventDefault() {} });
  state.playlists.push(playlist);
  const clickAct = act => document.fire('click', { target: { closest: s => s === '[data-act]' ? { dataset: { act } } : null } });
  U.go('pl/mix');
  filter('album');
  let list = U.lists()[0];
  assert.deepEqual(Array.from(list.ids), ['s2', 's3']);
  clickAct('playCtx');
  assert.deepEqual(playedFrom.at(-1), [['s2', 's3'], 0]);
  list.select(1);
  const row = { dataset: { i: '1' } };
  const rowTarget = { closest: s => s === '.tr' ? row : null };
  list.body.fire('dragstart', { target: rowTarget, dataTransfer: { setData() {}, setDragImage() {} } });
  list.body.fire('dragover', { clientY: 0, dataTransfer: {} });
  list.body.fire('drop', { clientY: 0, dataTransfer: {} });
  assert.deepEqual(playlist.items.map(x => x.id), ['s1', 's2', 's3', 's4'], 'filtered drag does not reorder the stored list');
  list.body.fire('dragend');
  press('Enter');
  assert.deepEqual(playedFrom.at(-1), [['s2', 's3'], 1]);
  press('Delete');
  assert.deepEqual(playlist.items.map(x => x.id), ['s1', 's2', 's4'], 'Delete removes the visible song');
  list = U.lists()[0];
  list.select(0);
  U.trackMenu(list.selected(), 0, 0, list, 0);
  const menu = document.querySelector('#menu');
  const remove = /data-i="(\d+)"[^<]*>(?:(?!<button).)*Remove from this playlist/.exec(menu.innerHTML);
  playlist.items.unshift({ id: 'missing' });
  menu.onclick({ target: { closest: () => ({ dataset: { i: remove[1] } }) } });
  assert.deepEqual(playlist.items.map(x => x.id), ['missing', 's1', 's4'], 'the menu removes the visible entry even if the stored positions changed');
  console.log('ok   filtered playlist drag, selection, playback, Delete and menu target the visible rows');

  U.go('songs');
  filter('album');
  list = U.lists()[0];
  list.select(1);
  press('Enter');
  assert.deepEqual(playedFrom.at(-1), [['s2', 's3'], 1]);
  input().focus();
  input().setSelectionRange(1, 4, 'forward');
  const heading = U.lists()[0].el.querySelector('.tl-h');
  const sortTarget = { closest: () => ({ dataset: { sort: 'duration' } }) };
  let focusKept = false;
  heading.fire('mousedown', { target: sortTarget, preventDefault() { focusKept = true; } });
  assert.equal(focusKept, true);
  heading.fire('click', { target: sortTarget });
  assert.equal(input().value, 'album');
  assert.equal(document.activeElement, input());
  assert.equal(input().selectionStart, 1);
  L.all = ['s1', 's3', 's4'];
  U.render({ keep: true });
  assert.deepEqual(Array.from(U.lists()[0].ids), ['s3']);
  state.liked = { s2: 2, s3: 1 };
  U.go('liked');
  filter('album');
  input().focus();
  U.toggleLike(['s2']);
  assert.equal(input().value, 'album');
  assert.equal(document.activeElement, input());
  assert.deepEqual(Array.from(U.lists()[0].ids), ['s3']);
  clickAct('playCtx');
  assert.deepEqual(playedFrom.at(-1), [['s3'], 0]);
  console.log('ok   sorting, library redraws and unliking preserve the filter and input focus');

  L.all = ['s1', 'missing', 's2'];
  auto.N.ctxIds = ui.N.ctxIds;
  a.playIds(['s1'], 0, liked);
  a.next();
  assert.equal(a.current().id, 's2');
  console.log('ok   autoplay skips stale library IDs before sorting');

  auto.N.sortedAlbums = () => [albums.get('alb1'), albums.get('alb2'), { key: 'alb3', name: 'Album 3', ids: ['s4'] }];
  a.playIds(['s2'], 0, { kind: 'album', key: 'alb2' });
  libSongs.s2.unplayable = true;
  a.next(false);
  assert.equal(a.current().id, 's4');
  delete libSongs.s2.unplayable;
  console.log('ok   autoplay keeps page order when the current album is unplayable');

  L.all = auto.N.lib.all = Object.keys(libSongs);
  a.playIds(L.all, 3, { kind: 'songs', key: '' });
  aq.shuffle = true;
  as.hidden = { s4: 1 };
  a.next();
  assert(!ids().includes('s4') && a.current().id !== 's4');
  aq.shuffle = false;
  delete as.hidden.s4;
  console.log('ok   a shuffled Songs wrap leaves out the song just hidden');

  const uiPlayer = ui.N.player, qBody = document.querySelector('#qBody');
  uiPlayer.history = ['s4', 's1'];
  state.plays = { s3: [1, 1] };
  state.hidden = { s4: 1, s3: 1 };
  uiPlayer.upcoming = () => [{ id: 's2', i: 1 }];
  uiPlayer.context = () => ({ kind: 'songs', key: '' });
  U.go('songs');
  U.openQueue();
  assert.match(qBody.innerHTML, /data-go-to="songs" data-focus-id="s2"/);
  let scrolledTo = '';
  U.lists()[0].scrollTo = id => { scrolledTo = id; return true; };
  document.fire('click', { target: { closest: s => s === '[data-go-to]' ? { dataset: { goTo: 'songs', focusId: 's2' } } : null } });
  assert.equal(scrolledTo, 's2');
  document.querySelector('#qTab').fire('click', { target: { closest: () => ({ dataset: { k: 'recent' } }) } });
  assert.match(qBody.innerHTML, /data-rec="s1"/);
  assert.doesNotMatch(qBody.innerHTML, /data-rec="s[34]"/);
  uiPlayer.current = () => libSongs.s2;
  uiPlayer.context = () => album1;
  libSongs.s2.albumKey = 'alb2';
  document.querySelector('#nowT').fire('click');
  assert.equal(U.route(), 'album/alb2');
  console.log('ok   queue links find the intended song and Recent excludes hidden copies');

  uiPlayer.addToQueue = a.addToQueue;
  U.go('songs');
  list = U.lists()[0];
  list.select(list.ids.indexOf('s1'));
  const dragRow = { closest: s => s === '.tr' ? { dataset: { i: list.ids.indexOf('s1') } } : null };
  list.body.fire('dragstart', { target: dragRow, dataTransfer: { setData() {}, setDragImage() {} } });
  libSongs.s1.unplayable = true;
  const beforeDrop = toastsShown.length;
  document.querySelector('#qBtn').fire('drop');
  assert.equal(toastsShown.length, beforeDrop);
  delete libSongs.s1.unplayable;
  document.querySelector('#qBtn').fire('drop');
  assert.equal(toastsShown.at(-1), 'Single Track added to the queue');
  list.body.fire('dragend');
  a.clearQueue();
  console.log('ok   queue drop reports the accepted songs');

  a.playIds(['s1'], 0, album1);
  await flush();
  tick(active(), 105);
  ui.N.S = { ...state, queue: aq };
  const decoded = saved => ui.N.mergeState(state, JSON.parse(JSON.stringify(saved)));
  assert.equal(decoded(ui.N.snapshot()).queue.items.length, 1, 'a speculative successor is not saved');
  a.next();
  const restored = decoded(ui.N.snapshot());
  assert.equal(restored.queue.listStart, 1);
  assert.equal(restored.queue.past[0].ctx.key, 'alb1');
  auto.N.S.queue = restored.queue;
  a.restore();
  a.cycleRepeat();
  a.next();
  a.next();
  assert.equal(a.current().id, 's2');
  a.prev();
  assert.equal(a.context().key, 'alb1');
  const legacyState = decoded({ queue: { items: ['s1', 's2'], orig: ['s1', 's2'], at: 1, ctx: liked, repeat: 'all' } });
  auto.N.S.queue = legacyState.queue;
  a.restore();
  a.next();
  assert.equal(a.current().id, 's1');
  console.log('ok   restored queues keep their list range and context, including legacy saves');
}
// the page modules leave timers running (a loop's watch, a lyrics retry, Discord status), as they would in String
run().catch(e=>{console.error(e);process.exitCode=1;}).finally(()=>setImmediate(()=>process.exit(process.exitCode||0)));
