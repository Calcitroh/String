/* String player: two decks (so the next song is ready before this one ends, and the two can crossfade), soft fades on
   pause and play, a Spotify-style queue (songs you queue yourself play before the rest of the album or playlist),
   shuffle, repeat, play counts, Windows media controls, and presence.json in the data folder (what's playing, for a
   Discord helper later).
   The queue lives in N.S.queue. Items have { id, m }; m = 1 for songs queued by hand. */
(function () {
  'use strict';
  const N = window.N;
  const L = N.lib;
  const P = N.player = { playing: false, history: [] };
  let ctx = null, mix = null, master = null, analyser = null, routed = false;
  let suspending = null;
  let A = 0;                 // the deck that's playing
  let xf = null;             // a crossfade under way: { from, timer }
  let errors = 0, counted = false, preloadedFor = '', lastPosSave = 0, token = 0, continuation = null;
  const decks = [deck(), deck()];
  const Q = () => N.S.queue;

  function deck() {
    const el = new Audio();
    el.preload = 'auto';
    const d = { el, gain: null, id: '', lastT: null, started: false };
    el.addEventListener('timeupdate', () => onTime(d));
    el.addEventListener('ended', () => onEnded(d));
    el.addEventListener('error', () => onError(d));
    // every (re)start of the sound refreshes Discord's time bar and the media controls: a repeat, a resume after a stall
    el.addEventListener('playing', () => {
      if (d !== decks[A] || !P.playing || !d.id || d.id !== P.current()?.id) return;
      d.started = true;
      if (P.history[0] !== d.id) {
        P.history.unshift(d.id);
        if (P.history.length > 80) P.history.length = 80;
        N.emit('history');
      }
      errors = 0; N.emit('time'); session(); presence();
    });
    el.addEventListener('loadedmetadata', () => { if (d === decks[A]) N.emit('time'); });
    el.addEventListener('seeked', () => { d.lastT = el.currentTime; if (d === decks[A]) N.emit('time'); });
    return d;
  }

  /* ---------- sound graph: deck → deck gain → mix → volume → speakers; the visualizer listens to the mix, before the volume.
     The decks are routed through it only once the browser lets it run (after the first click or key press in the tool):
     a song started before that (a media key right after String opens) plays straight, without fades or the
     visualizer, instead of silently. Routing can't be undone, so it waits until it's sure. ---------- */
  async function graph() {
    if (routed) return true;
    if (!window.AudioContext) return false;
    try {
      if (!ctx) {
        ctx = new AudioContext({ latencyHint: 'playback' });
        mix = ctx.createGain();
        master = ctx.createGain();
        analyser = ctx.createAnalyser();
        analyser.fftSize = 2048;
        analyser.smoothingTimeConstant = .7;
        analyser.minDecibels = -88;
        analyser.maxDecibels = -12;
        mix.connect(analyser);
        mix.connect(master).connect(ctx.destination);
      }
      if (ctx.state !== 'running') {
        if (navigator.userActivation && !navigator.userActivation.hasBeenActive) return false;
        await Promise.race([ctx.resume().catch(() => { }), new Promise(r => setTimeout(r, 300))]);
      }
      if (ctx.state !== 'running') return false;
      for (const d of decks) { d.gain = ctx.createGain(); ctx.createMediaElementSource(d.el).connect(d.gain).connect(mix); d.el.volume = 1; }
      routed = true;
      volume();
      if (N.viz) N.viz.attach(analyser, ctx);
      return true;
    } catch (e) { console.warn('web audio', e); return false; }
  }
  P.audio = () => ({ state: ctx ? ctx.state : 'none', routed });
  function ramp(d, to, secs) {
    if (!routed || !d.gain) return;
    const g = d.gain.gain, now = ctx.currentTime;
    g.cancelScheduledValues(now);
    g.setValueAtTime(g.value, now);
    g.linearRampToValueAtTime(to, now + Math.max(.01, secs));
  }
  function gainNow(d, v) {
    if (!routed || !d.gain) return;
    d.gain.gain.cancelScheduledValues(ctx.currentTime);
    d.gain.gain.setValueAtTime(v, ctx.currentTime);
  }
  function volume() {
    const S = N.S, v = S.muted ? 0 : Math.pow(Math.max(0, Math.min(1, S.vol)), 2);   // squared: halfway sounds like half
    if (routed) master.gain.setTargetAtTime(v, ctx.currentTime, .03);
    else decks.forEach(d => { d.el.volume = v; });
  }

  function load(d, t) {
    d.id = t.id;
    d.lastT = null;
    d.started = false;
    d.el.src = belt.native.mediaUrl(t.path);
  }
  function silence(d) {
    d.el.pause();
    d.el.removeAttribute('src');
    try { d.el.load(); } catch { }
    d.id = '';
  }
  function suspendMixer() {
    if (routed && !P.playing && !suspending && ctx.state === 'running')
      suspending = ctx.suspend().catch(() => {}).finally(() => { suspending = null; });
  }
  async function go(d) {
    const mine = token;
    await graph();
    if (suspending) await suspending;
    if (mine !== token || d !== decks[A] || !P.playing) return false;
    if (routed && ctx.state !== 'running') ctx.resume().catch(() => { });
    try { await d.el.play(); return mine === token; }
    catch (e) {
      if (mine !== token || (e && e.name === 'AbortError')) return false;
      if (e && e.name === 'NotAllowedError') { P.playing = false; suspendMixer(); changed('state'); N.emit('blocked'); }
      return false;
    }
  }
  const changed = (...evs) => { evs.forEach(ev => N.emit(ev)); session(); presence(); };

  /* ---------- the queue ---------- */
  const playable = id => { const t = L.get(id); return t && !t.unplayable && !N.S.hidden?.[id]; };
  // A prepared autoplay list becomes editable and saved only when playback reaches it.
  const queueEnd = () => continuation ? continuation.at : Q().items.length;
  P.queueItems = () => continuation ? Q().items.slice(0, continuation.at) : Q().items;
  // The latest list keeps its original order. Older lists keep their boundaries and contexts for Previous.
  function listRange(i) {
    const q = Q(), latest = q.listStart || 0;
    let end = queueEnd();
    if (i >= latest || !q.past?.length) return { start: latest, end, ctx: q.ctx };
    end = latest;
    for (let k = q.past.length - 1; k >= 0; k--) {
      const list = q.past[k];
      if (i >= list.start) return { ...list, end };
      end = list.start;
    }
    return { start: 0, end, ctx: null };
  }
  P.context = (i = Q().at) => listRange(i).ctx;

  function nextIndex(from = Q().at) {
    const q = Q(), wrap = q.repeat === 'all';
    const { start, end } = wrap ? listRange(from) : { start: 0, end: queueEnd() };
    for (let i = from + 1; i < end; i++) if (playable(q.items[i].id)) return i;
    if (wrap) for (let i = start; i <= from && i < end; i++) {
      if (playable(q.items[i].id)) return i;
    }
    return -1;
  }
  function prevIndex() {
    const q = Q();
    for (let i = q.at - 1; i >= 0; i--) if (playable(q.items[i].id)) return i;
    if (q.repeat === 'all' && q.at >= 0) for (let i = listRange(q.at).end - 1; i >= q.at; i--) {
      if (playable(q.items[i].id)) return i;
    }
    return -1;
  }
  const shuffled = a => { for (let i = a.length - 1; i > 0; i--) { const j = Math.floor(Math.random() * (i + 1)); [a[i], a[j]] = [a[j], a[i]]; } return a; };

  // starts item i of the queue (on the deck that already has it, if the next song was loaded ahead)
  async function start(i, { play = true, from = 0 } = {}) {
    const q = Q();
    if (!q.items[i]) return;
    const t = L.get(q.items[i].id);
    if (!t) return;
    discardContinuation();
    endCrossfade();
    q.at = i;
    q.pos = from;
    const o = decks[1 - A];
    if (o.id === t.id && from === 0) { silence(decks[A]); A = 1 - A; if (o.el.currentTime > .05) o.el.currentTime = 0; }
    else { silence(o); load(decks[A], t); if (from) decks[A].el.currentTime = from; }
    preloadedFor = '';
    counted = false;
    gainNow(decks[A], 1);
    P.playing = play;
    token++;
    changed('track', 'queue', 'state');
    N.save();
    if (play) await go(decks[A]);
  }

  P.current = () => { const q = Q(), it = q.items[q.at]; return it ? L.get(it.id) || null : null; };
  P.time = () => { const el = decks[A].el; return el.getAttribute('src') && el.readyState ? el.currentTime : Q().pos || 0; };
  P.duration = () => { const el = decks[A].el, t = P.current(); return isFinite(el.duration) && el.duration > 0 ? el.duration : t ? t.duration || 0 : 0; };
  P.buffered = () => { const el = decks[A].el; try { return el.buffered.length ? el.buffered.end(el.buffered.length - 1) : 0; } catch { return 0; } };
  P.analyser = () => analyser;
  P.upcoming = () => { const q = Q(); return q.items.slice(q.at + 1, queueEnd()).map((x, k) => ({ ...x, i: q.at + 1 + k })); };

  // play these songs (an album, a playlist, the search results…) starting at index `start`; -1 starts anywhere (shuffle)
  P.playIds = (ids, startAt = 0, from = null) => {
    const given = ids;
    ids = given.filter(playable);
    if (!ids.length) return;
    const q = Q();
    discardContinuation();
    // the clicked row counts in the list as shown: songs that can't play are left out before it, so the same song
    // starts (or, if that one can't play, the next one that can)
    if (startAt >= 0 && startAt < given.length) startAt = Math.min(given.slice(0, startAt).filter(playable).length, ids.length - 1);
    else startAt = Math.floor(Math.random() * ids.length);
    let base, at;
    if (q.shuffle) { const first = ids[startAt]; base = [first, ...shuffled(ids.filter((_, k) => k !== startAt))]; at = 0; }
    else { base = ids; at = startAt; }
    const manual = q.items.slice(q.at + 1).filter(x => x.m);   // songs queued by hand stay next, as in Spotify
    q.items = base.slice(0, at + 1).map(id => ({ id, m: 0 })).concat(manual, base.slice(at + 1).map(id => ({ id, m: 0 })));
    q.orig = ids.slice();
    q.ctx = from;
    q.listStart = 0;
    q.past = [];
    if (from) {
      const S = N.S;
      S.recent = [{ ...from, at: Date.now() }, ...(S.recent || []).filter(r => !(r.kind === from.kind && r.key === from.key))].slice(0, 16);
    }
    start(at, { play: true });
  };
  P.shuffleIds = (ids, from) => { if (!Q().shuffle) { Q().shuffle = true; } P.playIds(ids, -1, from); };

  P.play = async () => {
    const q = Q();
    if (!P.current()) {
      if (q.items.length) { const i = q.at < 0 ? 0 : nextIndex(q.at) >= 0 ? nextIndex(q.at) : 0; return start(i); }
      if (L.all.length) return P.shuffleIds(L.all, { kind: 'songs', key: '', name: 'All songs' });
      return;
    }
    const d = decks[A];
    if (!d.el.getAttribute('src')) { load(d, P.current()); if (q.pos) d.el.currentTime = q.pos; }
    P.playing = true;
    token++;
    gainNow(d, 0);
    changed('state');
    const ok = await go(d);
    if (ok && P.playing) ramp(d, 1, .22);
  };
  P.pause = () => {
    if (!P.playing) return;
    P.playing = false;
    Q().pos = P.time();
    changed('state');
    N.save();
    endCrossfade();
    const d = decks[A], mine = ++token;
    if (!routed) { d.el.pause(); return; }
    ramp(d, 0, .16);
    setTimeout(() => {
      if (mine !== token || P.playing) return;
      d.el.pause();
      suspendMixer();
    }, 180);
  };
  P.toggle = () => P.playing ? P.pause() : P.play();

  function pickContinuation() {
    const q = Q(), cur = P.current(), curId = cur ? cur.id : '', c = P.context(), shuf = !!q.shuffle;
    const recent = new Set(q.items.slice(Math.max(0, q.at - 30), q.at + 1).map(x => x.id));
    const pickFrom = (list, curKey) => {
      const idx = list.findIndex(x => x.key === curKey);
      const ordered = list.slice(idx + 1).concat(list.slice(0, Math.max(0, idx)));
      const others = ordered.map(item => ({ item, ids: item.ids.filter(playable) })).filter(x => x.ids.length);
      if (!others.length) return null;
      if (shuf) {
        const fresh = others.filter(x => x.ids.some(id => !recent.has(id)));
        const pool = fresh.length ? fresh : others;
        return pool[Math.floor(Math.random() * pool.length)];
      }
      return others[0];
    };
    if (c && c.kind === 'album' && L.albums && L.albums.size) {
      const albums = N.sortedAlbums ? N.sortedAlbums() : [...L.albums.values()];
      const next = pickFrom(albums, c.key);
      if (next) {
        const ids = shuf ? shuffled(next.ids.slice()) : next.ids.slice();
        return { ids, orig: next.ids.slice(), ctx: { kind: 'album', key: next.item.key, name: next.item.name } };
      }
    }
    if (c && c.kind === 'artist' && L.artists && L.artists.size) {
      const artists = N.sortedArtists ? N.sortedArtists() : [...L.artists.values()];
      const next = pickFrom(artists, c.key);
      if (next) {
        const ids = shuf ? shuffled(next.ids.slice()) : next.ids.slice();
        return { ids, orig: next.ids.slice(), ctx: { kind: 'artist', key: next.item.key, name: next.item.name } };
      }
    }
    const allLib = (N.ctxIds ? N.ctxIds({ kind: 'songs', key: '' }) : (L.all || [])).filter(playable);
    if (c && c.kind === 'songs' && allLib.some(id => id !== curId)) {
      const ids = shuf
        ? [...shuffled(allLib.filter(id => id !== curId)), ...allLib.filter(id => id === curId)]
        : (allLib[0] === curId ? [...allLib.slice(1), allLib[0]] : allLib.slice());
      return { ids, orig: allLib.slice(), ctx: { kind: 'songs', key: '', name: 'All songs' } };
    }
    if (allLib.some(id => id !== curId)) {
      const inList = new Set((c && ['liked', 'pl', 'search', 'album', 'artist'].includes(c.kind) ? (q.orig || []) : []).concat(curId ? [curId] : []));
      const outside = allLib.filter(id => !inList.has(id));
      if (outside.length) {
        const ak = cur && cur.artistKey;
        const sameArtist = ak ? outside.filter(id => (L.get(id) || {}).artistKey === ak) : [];
        const others = ak ? outside.filter(id => (L.get(id) || {}).artistKey !== ak) : outside;
        const ordered = (shuf ? shuffled(sameArtist) : sameArtist).concat(shuf ? shuffled(others) : others);
        return { ids: ordered, orig: ordered.slice(), ctx: { kind: 'songs', key: '', name: 'All songs' } };
      }
      const list = (Array.isArray(q.orig) && q.orig.length ? q.orig : allLib).filter(playable);
      if (list.some(id => id !== curId)) {
        const ids = shuf
          ? [...shuffled(list.filter(id => id !== curId)), ...list.filter(id => id === curId)]
          : (list[0] === curId ? [...list.slice(1), list[0]] : list.slice());
        return { ids, orig: list.slice(), ctx: c || { kind: 'songs', key: '', name: 'All songs' } };
      }
    }
    return null;
  }
  const canContinue = () => N.S.set?.autoplay !== false && (!Q().repeat || Q().repeat === 'off') && !P.loop();
  function discardContinuation() {
    if (!continuation) return;
    Q().items.length = continuation.at;
    continuation = null;
    silence(decks[1 - A]);
    preloadedFor = '';
  }
  function successor() {
    if (continuation && !canContinue()) discardContinuation();
    const i = nextIndex();
    if (i >= 0 || !canContinue()) return i;
    const q = Q();
    if (continuation) {
      for (let k = continuation.at; k < q.items.length; k++) if (playable(q.items[k].id)) return k;
      discardContinuation();
    }
    const next = pickContinuation();
    if (!next) return -1;
    continuation = { ...next, at: q.items.length };
    q.items = q.items.concat(next.ids.map(id => ({ id, m: 0 })));
    return continuation.at;
  }
  // Promote a prepared list only when its first playable song takes over.
  function advance(i, handover = start) {
    if (!continuation || i < continuation.at) return handover(i);
    const q = Q(), next = continuation;
    continuation = null;
    const keepFrom = Math.max(0, q.at - 199);
    const listStart = q.at + 1 - keepFrom;
    q.past = [...(q.past || []), { start: q.listStart || 0, ctx: q.ctx }]
      .flatMap((list, k, lists) => (lists[k + 1]?.start ?? q.at + 1) > keepFrom
        ? [{ ...list, start: Math.max(0, list.start - keepFrom) }] : []);
    q.items = q.items.slice(keepFrom, q.at + 1).concat(q.items.slice(next.at));
    q.at = listStart - 1;
    q.listStart = listStart;
    q.orig = next.orig;
    q.ctx = next.ctx;
    if (next.ctx && ['album', 'artist', 'pl'].includes(next.ctx.kind)) {
      const S = N.S;
      S.recent = [{ ...next.ctx, at: Date.now() }, ...(S.recent || []).filter(r => !(r.kind === next.ctx.kind && r.key === next.ctx.key))].slice(0, 16);
    }
    return handover(listStart + i - next.at);
  }

  P.next = (user = true) => {
    const q = Q();
    if (!q.items.length) return;
    if (!user && q.repeat === 'one') { P.seek(0); return; }
    const i = successor();
    if (i >= 0) { advance(i); return; }
    if (user && canContinue() && playable(P.current()?.id)) { P.seek(0); if (!P.playing) P.play(); return; }
    stopAtEnd();
  };
  P.prev = () => {
    if (P.time() > 3.5 || !Q().items.length) { P.seek(0); return; }
    const i = prevIndex();
    if (i < 0) { P.seek(0); return; }
    start(i, { play: true });   // like Next (and Spotify): moving to another song plays it
  };
  P.jump = i => { if (i < queueEnd() && Q().items[i] && playable(Q().items[i].id)) start(i, { play: true }); };
  P.seek = sec => {
    endCrossfade();
    const d = decks[A], dur = P.duration();
    sec = Math.max(0, Math.min(dur ? dur - .25 : sec, sec));
    Q().pos = sec;
    if (d.el.getAttribute('src')) d.el.currentTime = sec;
    N.emit('time');
    session();
    presence();
  };
  function stopAtEnd() {
    const d = decks[A];
    P.playing = false;
    token++;
    d.el.pause();
    suspendMixer();
    Q().pos = 0;
    try { d.el.currentTime = 0; } catch { }
    changed('state', 'time');
    N.save();
  }

  P.setVolume = v => { N.S.vol = Math.max(0, Math.min(1, v)); if (N.S.vol > 0) N.S.muted = false; volume(); N.emit('volume'); N.save(); };
  P.toggleMute = () => { N.S.muted = !N.S.muted; volume(); N.emit('volume'); N.save(); };

  P.setShuffle = on => {
    const q = Q();
    discardContinuation();
    q.shuffle = !!on;
    if (q.at >= 0 && q.items.length) {
      const { start, end } = listRange(q.at), after = q.items.slice(q.at + 1, end), cur = q.items[q.at].id;
      const orig = q.at >= (q.listStart || 0) ? (q.orig || []) : q.items.slice(start, end).filter(x => !x.m).map(x => x.id);
      const k = orig.indexOf(cur);
      let ids;
      if (on) {
        ids = orig.length ? orig.filter((id, i) => i !== k && playable(id)) : after.filter(x => !x.m).map(x => x.id);
        shuffled(ids);
      } else {
        ids = orig.slice(k + 1).filter(id => L.get(id) && !N.S.hidden?.[id]);
      }
      const rest = ids.map(id => ({ id, m: 0 }));
      after.forEach((item, i) => { if (item.m) rest.splice(i, 0, item); });
      editUpcoming(q.at + 1, after.length, rest);
      preloadedFor = '';
    }
    N.emit('queue'); N.emit('modes'); N.save();
  };
  P.cycleRepeat = () => { const q = Q(); discardContinuation(); q.repeat = q.repeat === 'off' ? 'all' : q.repeat === 'all' ? 'one' : 'off'; preloadedFor = ''; N.emit('modes'); N.save(); };
  P.setAutoplay = on => { N.S.set.autoplay = on; if (!on) discardContinuation(); N.save(); };

  function editUpcoming(at, count, items = []) {
    const q = Q(), start = q.listStart || 0;
    const shift = i => i > q.at && i >= at ? Math.max(at, i - count) + items.length : i;
    q.listStart = q.past?.length ? shift(start) : 0;
    q.past?.forEach(list => { list.start = shift(list.start); });
    q.items = q.items.slice(0, at).concat(items, q.items.slice(at + count));
  }

  // "Play next": right after this song. "Add to queue": after the other songs queued by hand
  P.playNext = ids => insert(ids, false);
  P.addToQueue = ids => insert(ids, true);
  function insert(ids, after) {
    ids = ids.filter(playable);
    if (!ids.length) return ids;
    discardContinuation();
    const q = Q();
    const items = ids.map(id => ({ id, m: 1 }));
    let i = q.at + 1;
    if (after) while (q.items[i] && q.items[i].m) i++;
    editUpcoming(i, 0, items);
    preloadedFor = '';
    N.emit('queue'); N.save();
    return ids;
  }
  P.removeAt = i => {
    const q = Q();
    if (i <= q.at || i >= queueEnd() || !q.items[i]) return;
    discardContinuation();
    const item = q.items[i];
    if (!item.m && i >= (q.listStart || 0)) {
      const k = q.orig?.indexOf(item.id) ?? -1;
      if (k >= 0) q.orig.splice(k, 1);
    }
    editUpcoming(i, 1);
    preloadedFor = '';
    N.emit('queue'); N.save();
  };
  P.move = (from, to) => {
    const q = Q();
    if (from <= q.at || from >= queueEnd() || to <= q.at || !q.items[from]) return;
    discardContinuation();
    const [item] = q.items.splice(from, 1);
    q.items.splice(to > from ? to - 1 : to, 0, item);
    preloadedFor = '';
    N.emit('queue'); N.save();
  };
  P.clearQueue = () => {
    const q = Q();
    discardContinuation();
    const lists = [...(q.past || []), { start: q.listStart || 0 }];
    let removed = 0, l = 0;
    q.items = q.items.filter((x, k) => {
      while (lists[l]?.start === k) lists[l++].start -= removed;
      if (k <= q.at || !x.m) return true;
      removed++;
      return false;
    });
    while (l < lists.length) lists[l++].start -= removed;
    q.listStart = lists.at(-1).start;
    preloadedFor = '';
    N.emit('queue'); N.save();
  };

  /* ---------- the song running out: preload, crossfade, gapless swap ---------- */
  function onTime(d) {
    if (d !== decks[A]) return;
    const el = d.el, dur = el.duration, t = el.currentTime, q = Q();
    q.pos = t;
    heard(d, t);
    if (P.playing && d.started && !counted && t > Math.min(30, (isFinite(dur) ? dur : 60) * .5)) { counted = true; countPlay(d.id); }
    if (Date.now() - lastPosSave > 15000) { lastPosSave = Date.now(); N.save(); }
    if (P.loop()) { loopCheck(); return; }   // a looped song never runs out: no preload, no crossfade
    if (!isFinite(dur) || !P.playing || q.repeat === 'one' || xf) return;
    const left = dur - t, fade = routed ? +N.S.set.fade || 0 : 0;
    if (left >= Math.max(20, fade)) return;
    const ni = successor();
    if (ni < 0) return;
    const nid = q.items[ni].id;
    if (fade > 0 && dur > fade * 2 + 4 && left <= fade && left > .4) { advance(ni, i => crossfade(i, left)); return; }
    if (left < 20 && preloadedFor !== nid) {
      const o = decks[1 - A];
      if (o.id !== nid) { load(o, L.get(nid)); o.el.pause(); }
      gainNow(o, 1);
      preloadedFor = nid;
    }
  }
  // listening time: the seconds of music since the last tick (a seek, a repeat or a stall jumps, and isn't counted)
  function heard(d, t) {
    if (P.playing && d.lastT != null && N.stats) { const dt = t - d.lastT; if (dt > 0 && dt < 1.5) N.stats.add(L.get(d.id), dt); }
    d.lastT = t;
  }

  /* ---------- the loop (as Spicetify's Loopy Loop): a stretch of one song, played over and over ----------
     N.S.loops[track id] = { a, b } in seconds; a missing a loops from the start, a missing b to the end. Remembered per
     song, so a long song's good part loops every time it plays, until the loop is cleared. */
  let loopTimer = 0, looping = false;
  const savedLoop = () => { const t = P.current(), lp = t && N.S.loops && N.S.loops[t.id]; return lp && (lp.a != null || lp.b != null) ? lp : null; };
  // a loop shorter than half a second (its start too near its end, as the song's length came in) is no loop at all:
  // it isn't shown, and preloading, crossfades and the end of the song work as without one
  P.loop = () => { const e = loopEnds(); return e && e.b - e.a >= .5 ? savedLoop() : null; };
  P.setLoop = (a, b) => {
    const t = P.current();
    if (!t) return null;
    const dur = P.duration();
    const fix = x => x == null || !isFinite(x) ? null : Math.max(0, Math.min(dur ? dur - .3 : x, +x));
    a = fix(a); b = fix(b);
    if (a != null && b != null && b < a) [a, b] = [b, a];
    // at least a second long, measured from where it really starts and ends (an unset start is the song's start, an
    // unset end its end): a start set near the end moves back, an end set near the start moves on
    const ea = a == null ? 0 : a, eb = b != null ? b : dur ? dur - .12 : Infinity;
    if (eb - ea < 1) { if (b != null) b = Math.min(dur || ea + 1, ea + 1); else if (a != null) a = Math.max(0, eb - 1); }
    if (a === 0) a = null;
    N.S.loops = N.S.loops || {};
    if (a == null && b == null) delete N.S.loops[t.id]; else N.S.loops[t.id] = { a, b };
    if (P.loop()) discardContinuation();
    N.save();
    loopWatch();
    N.emit('loop');
    return P.loop();
  };
  P.clearLoop = () => { const t = P.current(); if (t && N.S.loops) { delete N.S.loops[t.id]; N.save(); } loopWatch(); N.emit('loop'); };
  function loopEnds() {
    const lp = savedLoop(), dur = P.duration();
    return lp ? { a: lp.a || 0, b: lp.b != null ? lp.b : dur ? dur - .12 : Infinity } : null;
  }
  // back to the start of the loop, with a dip in the sound so the jump doesn't click; the heard time up to the jump counts
  function loopCheck() {
    const e = P.loop() && loopEnds(), d = decks[A];
    if (!e || !P.playing || looping) return;
    const t = d.el.currentTime;
    if (t < e.b - .06) return;
    looping = true;
    heard(d, Math.min(t, e.b));
    ramp(d, 0, .04);
    setTimeout(() => {
      looping = false;
      if (decks[A] !== d || !P.loop()) { gainNow(d, 1); return; }
      P.seek(e.a);
      ramp(d, 1, .08);
      if (d.el.paused && P.playing) go(d);
    }, routed ? 45 : 0);
  }
  // a fine-grained watch while a loop plays (timeupdate alone comes only 4 times a second)
  function loopWatch() {
    clearInterval(loopTimer);
    loopTimer = 0;
    if (P.loop() && P.playing) loopTimer = setInterval(loopCheck, 30);
  }
  N.on('state', loopWatch);
  N.on('track', loopWatch);

  function onEnded(d) {
    if (xf && d === decks[xf.from]) { endCrossfade(); return; }
    if (d !== decks[A] || !P.playing) return;
    if (P.loop()) { const e = loopEnds(); heard(d, d.el.duration); P.seek(e.a); go(d); return; }   // a loop to the very end
    if (Q().repeat === 'one') {
      // the same song again: Discord's time bar must start over too, or the helper sees the old one run out and clears
      d.el.currentTime = 0; Q().pos = 0; counted = false;
      go(d);
      changed('time');
      return;
    }
    P.next(false);
  }
  function crossfade(ni, left) {
    const q = Q(), from = A, to = 1 - A, t = L.get(q.items[ni].id), o = decks[to];
    if (o.id !== t.id) load(o, t);
    else if (o.el.currentTime > .05) o.el.currentTime = 0;
    gainNow(o, 0);
    xf = { from, timer: setTimeout(endCrossfade, (left + .8) * 1000) };
    A = to;
    q.at = ni;
    q.pos = 0;
    counted = false;
    preloadedFor = '';
    token++;
    go(o);
    ramp(o, 1, left * .8);
    ramp(decks[from], 0, left);
    changed('track', 'queue');
    N.save();
  }
  function endCrossfade() {
    if (!xf) return;
    const d = decks[xf.from];
    clearTimeout(xf.timer);
    xf = null;
    silence(d);
    gainNow(decks[A], 1);
  }
  P.crossfading = () => !!xf;
  function onError(d) {
    if (!d.el.getAttribute('src')) return;
    if (d !== decks[A]) { d.id = ''; preloadedFor = ''; return; }   // a preload that failed: found out properly when it's reached
    const t = L.get(d.id);
    if (t) t.unplayable = true;
    N.emit('error', t);
    if (++errors > 4) { P.playing = false; suspendMixer(); changed('state'); return; }
    const mine = token;
    setTimeout(() => { if (mine === token && P.playing) P.next(false); }, 350);
  }

  function countPlay(id) {
    const S = N.S, p = S.plays[id] || [0, 0];
    S.plays[id] = [p[0] + 1, Date.now()];
    N.emit('plays');
    N.save();
  }

  // after a restart: the last song, paused where it was
  P.restore = () => {
    const q = Q(), t = P.current();
    if (!t) { q.at = q.items.length ? Math.min(q.at, q.items.length - 1) : -1; N.emit('track'); return; }
    const d = decks[A];
    load(d, t);
    const pos = q.pos || 0;
    if (pos) d.el.addEventListener('loadedmetadata', () => { if (decks[A] === d && d.id === t.id) d.el.currentTime = Math.min(pos, Math.max(0, d.el.duration - 1)); }, { once: true });
    P.playing = false;
    changed('track', 'queue', 'state');
  };
  // Older versions saved only 100 songs behind the cursor and 799 ahead.
  // Restore the omitted ends when the library still gives an unambiguous order.
  P.restoreLegacyOrder = (q, order) => {
    q.orig = order;
    if (q.shuffle || (q.at !== 100 && q.items.length !== q.at + 800)) return;
    const saved = q.items.filter(x => !x.m).map(x => x.id);
    if (!saved.length) return;
    const first = order.indexOf(saved[0]);
    if (first < 0 || order.indexOf(saved[0], first + 1) >= 0 || !saved.every((id, i) => order[first + i] === id)) return;
    const prefix = q.at === 100 ? order.slice(0, first) : [];
    const suffix = q.items.length === q.at + 800 ? order.slice(first + saved.length) : [];
    const items = ids => ids.map(id => ({ id, m: 0 }));
    q.items = items(prefix).concat(q.items, items(suffix));
    q.at += prefix.length;
  };
  P.volumeChanged = volume;

  /* ---------- Windows media controls (keyboard media keys, the volume flyout) ---------- */
  function session() {
    const ms = navigator.mediaSession;
    if (!ms) return;
    const t = P.current();
    try {
      if (!t) { ms.metadata = null; ms.playbackState = 'none'; return; }
      if (!ms.metadata || ms.metadata.title !== t.T || ms.metadata.artist !== (t.A || '')) {
        ms.metadata = new MediaMetadata({ title: t.T, artist: t.A || '', album: t.B || '' });
        N.art.url(t, 'md').then(u => { if (u && P.current() === t) ms.metadata = new MediaMetadata({ title: t.T, artist: t.A || '', album: t.B || '', artwork: [{ src: u, sizes: '420x420', type: 'image/jpeg' }] }); });
      }
      ms.playbackState = P.playing ? 'playing' : 'paused';
      const dur = P.duration();
      if (dur > 0) ms.setPositionState({ duration: dur, playbackRate: 1, position: Math.min(dur, P.time()) });
    } catch { }
  }
  if (navigator.mediaSession) {
    const ms = navigator.mediaSession, h = (a, f) => { try { ms.setActionHandler(a, f); } catch { } };
    h('play', () => P.play()); h('pause', () => P.pause()); h('stop', () => P.pause());
    h('previoustrack', () => P.prev()); h('nexttrack', () => P.next());
    h('seekto', e => P.seek(e.seekTime)); h('seekbackward', e => P.seek(P.time() - (e.seekOffset || 10))); h('seekforward', e => P.seek(P.time() + (e.seekOffset || 10)));
  }

  /* ---------- presence.json: what's playing, for a Discord "Listening to" helper later ---------- */
  let presTimer = 0, presDir = '';
  function presence(now) {
    clearTimeout(presTimer);
    presTimer = setTimeout(async () => {
      if (!belt.inBelt) return;
      try {
        presDir = presDir || (await belt.native.paths()).data;
        const t = P.current(), pos = P.time(), dur = P.duration(), at = Date.now(), set = N.S.set || {};
        const doc = t ? {
          app: 'String', state: P.playing ? 'playing' : 'paused', title: t.T, artist: t.A, album: t.B, duration: Math.round(dur * 1000) / 1000,
          position: Math.round(pos * 1000) / 1000, at, startedAt: P.playing ? Math.round(at - pos * 1000) : 0, endsAt: P.playing && dur ? Math.round(at + (dur - pos) * 1000) : 0,
          covers: !!set.dcCovers, display: set.dcShow || 'song',   // for the Discord helper: look covers up online? what the status line shows
        } : { app: 'String', state: 'stopped', at };
        P.lastPresence = doc;
        await belt.native.writeText(presDir + '\\presence.json', JSON.stringify(doc, null, 2));
      } catch { }
    }, now ? 0 : 600);
  }
  P.presence = presence;
  belt.onFlush(() => { if (P.playing) Q().pos = P.time(); });
})();
