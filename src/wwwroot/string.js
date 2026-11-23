/* String: the screens. Home, Songs, Albums, Artists, Liked, Stats, playlists, search and settings in the middle; the
   player bar, the queue drawer and Now Playing round them. Saved state (N.S) goes through belt.save (host.js: String's
   own window writes it); the library's tags live in the data folder (library.js), listening time in stats.js. */
(function () {
  'use strict';
  const N = window.N, L = N.lib, P = N.player, ART = N.art;
  const $ = (s, r = document) => r.querySelector(s), $$ = (s, r = document) => [...r.querySelectorAll(s)];
  const esc = s => String(s ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
  const attr = esc;

  /* ---------- icons the belt's set doesn't have ---------- */
  const ICON = {
    back: '<path d="M15.5 5l-7 7 7 7"/>',
    fwd: '<path d="M8.5 5l7 7-7 7"/>',
    expand: '<path d="M14 4h6v6M10 20H4v-6M20 4l-6.5 6.5M4 20l6.5-6.5"/>',
    down: '<path d="M6 9l6 6 6-6"/>',
    shuffle: '<path d="M3 7h3.5c2.5 0 4 1.5 5.5 5s3 5 5.5 5H21M3 17h3.5c1.2 0 2.2-.4 3-1.2M14.5 8.2c.8-.8 1.8-1.2 3-1.2H21"/><path d="M18.5 4.5L21 7l-2.5 2.5M18.5 14.5L21 17l-2.5 2.5"/>',
    prev: '<path d="M18 5.5v13L8.5 12z"/><path d="M6 5.5v13" stroke-width="2.2"/>',
    next: '<path d="M6 5.5v13l9.5-6.5z"/><path d="M18 5.5v13" stroke-width="2.2"/>',
    repeat: '<path d="M4 11V9a3 3 0 0 1 3-3h12M16 3l3 3-3 3M20 13v2a3 3 0 0 1-3 3H5M8 21l-3-3 3-3"/>',
    queue: '<path d="M4 5h12M4 10h12M4 15h7"/><path d="M15 13v6l5-3z" fill="currentColor"/>',
    vol: '<path d="M4 9.5h3.5L12 5.5v13l-4.5-4H4z"/><path d="M15.5 9a4 4 0 0 1 0 6M18 6.5a7.5 7.5 0 0 1 0 11"/>',
    vol1: '<path d="M4 9.5h3.5L12 5.5v13l-4.5-4H4z"/><path d="M15.5 9a4 4 0 0 1 0 6"/>',
    mute: '<path d="M4 9.5h3.5L12 5.5v13l-4.5-4H4z"/><path d="M16 9.5l5 5M21 9.5l-5 5"/>',
    more: '<path d="M5.5 12h.01M12 12h.01M18.5 12h.01" stroke-width="3"/>',
    disc: '<circle cx="12" cy="12" r="8.5"/><circle cx="12" cy="12" r="2.2"/><path d="M12 5.5a6.5 6.5 0 0 1 6.5 6.5"/>',
    swap: '<path d="M7 20V4M3 8l4-4 4 4M17 4v16M13 16l4 4 4-4"/>',
    playnext: '<path d="M4.25 5h10M4.25 10h7M4.25 15h7"/><path d="M14.25 12.5v7l5.5-3.5z" fill="currentColor"/>',
    playf: '<path d="M8.25 5.5v13l10.5-6.5z" fill="currentColor" stroke="none"/>',
    pausef: '<path d="M7 5h3.6v14H7zM13.4 5H17v14h-3.6z" fill="currentColor" stroke="none"/>',
  };
  const icon = n => ICON[n] ? `<svg viewBox="0 0 24 24" aria-hidden="true">${ICON[n]}</svg>` : belt.icon(n);
  N.icon = icon;
  const PP = '<svg class="pp" viewBox="0 0 24 24"><path class="a"/><path class="b"/></svg>';
  $$('[data-icon]').forEach(el => el.insertAdjacentHTML('afterbegin', icon(el.dataset.icon)));

  /* ---------- saved state ---------- */
  const DEFAULTS = () => ({
    v: 1, roots: [], files: [], playlists: [], liked: {}, plays: {}, edits: {}, hidden: {}, notDupes: {}, recent: [],
    queue: { items: [], at: -1, pos: 0, ctx: null, orig: [], listStart: 0, past: [], shuffle: false, repeat: 'off' },
    vol: .8, muted: false, set: { fade: 0, autoplay: true, timeLeft: false, artMono: true, viz: true, discord: false, discordId: '', dcCovers: false, dcShow: 'song' }, sort: { key: 'title', dir: 1 }, albumSort: 'artist', route: 'home',
  });
  N.S = DEFAULTS();
  // Store each path once. Adjacent sorted paths usually share both a directory and a file extension.
  const LIB_CTX = ['songs', 'album', 'artist', 'pl', 'liked'];
  function snapshot() {
    const s = N.S, q = s.queue;
    const queueItems = P.queueItems(), orig = q.orig || [], ids = queueItems.map(x => x.id);
    const unique = [...new Set([...ids, ...orig])].sort();
    const index = new Map(unique.map((id, i) => [id, i]));
    let prev = '';
    const paths = unique.map(id => {
      let front = 0, back = 0, limit = Math.min(prev.length, id.length);
      while (front < limit && prev[front] === id[front]) front++;
      while (back < limit - front && prev[prev.length - 1 - back] === id[id.length - 1 - back]) back++;
      const entry = [front, id.slice(front, id.length - back), back];
      prev = id;
      return entry;
    });
    const items = queueItems.map(x => x.m ? -1 - index.get(x.id) : index.get(x.id));
    const sameOrder = orig.length === ids.length && orig.every((id, i) => id === ids[i]);
    return { ...s, queue: { ...q, paths, items, orig: sameOrder ? null : orig.map(id => index.get(id)) } };
  }
  N.save = () => belt.save(snapshot);   // the snapshot is made when the save goes out, once per burst of changes
  function merge(base, saved) {
    const out = { ...base, ...saved };
    for (const k of ['queue', 'set', 'sort']) out[k] = { ...base[k], ...(saved[k] || {}) };
    const q = out.queue;
    if (Array.isArray(q.paths)) {
      let prev = '';
      const paths = q.paths.map(([front, middle, back]) => {
        prev = prev.slice(0, front) + middle + (back ? prev.slice(-back) : '');
        return prev;
      });
      q.items = q.items.map(n => ({ id: paths[n < 0 ? -1 - n : n], m: n < 0 ? 1 : 0 }));
      q.orig = q.orig ? q.orig.map(n => paths[n]) : q.items.map(x => x.id);
      delete q.paths;
    }
    if (!Array.isArray(q.items)) q.items = [];
    const man = new Set(Array.isArray(q.man) ? q.man : []);
    q.items = q.items.map((x, i) => typeof x === 'string' ? { id: x, m: man.has(i) ? 1 : 0 } : x).filter(x => x && x.id);
    delete q.man;
    if (q.at >= q.items.length) q.at = q.items.length - 1;
    return out;
  }

  /* ---------- small helpers ---------- */
  function mmss(s) {
    s = Math.max(0, Math.floor(s || 0));
    const h = Math.floor(s / 3600), m = Math.floor(s % 3600 / 60), x = String(s % 60).padStart(2, '0');
    return h ? `${h}:${String(m).padStart(2, '0')}:${x}` : `${m}:${x}`;
  }
  function long(s) {
    s = Math.round(s || 0);
    const h = Math.floor(s / 3600), m = Math.floor(s % 3600 / 60);
    return h ? `${h} hr ${m} min` : m ? `${m} min ${s % 60} sec` : `${s} sec`;
  }
  const plural = (n, w) => `${n.toLocaleString()} ${w}${n === 1 ? '' : 's'}`;
  function when(ts) {
    if (!ts) return '';
    const { calendarDay } = StringStats;
    const d = calendarDay(new Date(Date.now())) - calendarDay(new Date(ts));
    if (d < 1) return 'Today';
    if (d < 2) return 'Yesterday';
    if (d < 7) return `${d} days ago`;
    return new Date(ts).toLocaleDateString(undefined, { month: 'short', day: 'numeric', year: new Date(ts).getFullYear() === new Date().getFullYear() ? undefined : 'numeric' });
  }
  const calm = () => document.documentElement.classList.contains('calm') || matchMedia('(prefers-reduced-motion: reduce)').matches;
  const enc = encodeURIComponent;
  const S = () => N.S;
  const plIds = p => p.items.map(x => x.id);

  /* ---------- add-ons: extra features a person installs in String's data folder (<data>\addons\<name>\, with an
     addon.json naming its scripts and styles; host.js loads them before String starts). Only install add-ons from
     someone you trust: they run as part of String. An add-on can add a page (a route), a button by "Playlists", a
     section on the Library page, a line under Edit info and checks to the self-test, and it hears what String does
     through N.on: 'cacheLoaded', 'booted', 'edited' (a song's id), 'merged' ({ from, to }: a duplicate dropped for the
     copy kept) and 'removed' (the ids of song files moved to the Recycle Bin). ---------- */
  const ADD = { pages: {}, library: [], editNotes: [], buttons: [] };
  N.addons = {
    page(name, fn) { ADD.pages[name] = fn; },                   // fn(viewElement, argument) draws the page
    librarySection(fn) { ADD.library.push(fn); },               // fn() → HTML for the Library page
    editInfoNote(fn) { ADD.editNotes.push(fn); },               // fn(track) → a sentence for Edit info, or ''
    sidebarButton(b) { ADD.buttons.push(b); addonButtons(); },  // { id, title, icon, route }
    tests: [],                                                   // async fn(ctx) the self-test runs (selftest.js)
  };
  // the add-ons' buttons by "Playlists"; a button an older index.html still carries for an add-on that isn't installed
  // is hidden
  function addonButtons() {
    const bar = $('#newPl') && $('#newPl').parentElement;
    if (!bar) return;
    for (const b of ADD.buttons) {
      let el = document.getElementById(b.id);
      if (!el) {
        el = document.createElement('button');
        el.className = 'icon-btn';
        el.id = b.id;
        bar.insertBefore(el, $('#newPl'));
      }
      if (el.dataset.addonRoute === b.route) continue;
      el.hidden = false;
      el.title = b.title || '';
      el.dataset.addonRoute = b.route;
      el.innerHTML = belt.icon(b.icon || 'box');
      el.addEventListener('click', () => go(b.route));
    }
    $$('button', bar).forEach(el => { if (el.id !== 'newPl' && !el.dataset.addonRoute) el.hidden = true; });
  }
  const playlist = id => S().playlists.find(p => p.id === id);
  const likedIds = () => Object.entries(S().liked).sort((a, b) => b[1] - a[1]).map(([id]) => id).filter(id => L.tracks.has(id));
  const isLiked = id => !!S().liked[id];
  const curId = () => { const t = P.current(); return t ? t.id : ''; };
  const sameCtx = (a, b) => !!a && !!b && a.kind === b.kind && (a.key || '') === (b.key || '');

  /* ---------- what a context plays ---------- */
  function sortedSongs() {
    const { key, dir } = S().sort, c = L.coll;
    const t = id => L.get(id);
    const cmp = {
      title: (a, b) => c.compare(t(a).T, t(b).T),
      artist: (a, b) => c.compare(t(a).A || '￿', t(b).A || '￿') || c.compare(t(a).B, t(b).B) || (t(a).n || 999) - (t(b).n || 999),
      album: (a, b) => c.compare(t(a).B || '￿', t(b).B || '￿') || (t(a).disc || 0) - (t(b).disc || 0) || (t(a).n || 999) - (t(b).n || 999),
      added: (a, b) => (t(b).added || 0) - (t(a).added || 0),
      duration: (a, b) => (t(a).duration || 0) - (t(b).duration || 0),
    }[key] || ((a, b) => c.compare(t(a).T, t(b).T));
    const out = L.all.filter(id => L.tracks.has(id)).sort(cmp);
    return dir < 0 ? out.reverse() : out;
  }
  function sortedAlbums() {
    const mode = S().albumSort, c = L.coll;
    return [...L.albums.values()].sort(mode === 'recent' ? (a, b) => b.added - a.added : mode === 'year' ? (a, b) => (b.year || 0) - (a.year || 0) || c.compare(a.name, b.name)
      : mode === 'name' ? (a, b) => c.compare(a.name, b.name) : (a, b) => c.compare(a.artist, b.artist) || (a.year || 0) - (b.year || 0) || c.compare(a.name, b.name));
  }
  function sortedArtists() {
    return [...L.artists.values()].sort((a, b) => (!a.key) - (!b.key) || L.coll.compare(a.name, b.name));
  }
  function ctxIds(c) {
    if (!c) return [];
    if (c.kind === 'album') { const a = L.albums.get(c.key); return a ? a.ids : []; }
    if (c.kind === 'artist') { const a = L.artists.get(c.key); return a ? a.ids : []; }
    if (c.kind === 'pl') { const p = playlist(c.key); return p ? L.ids(plIds(p)) : []; }
    if (c.kind === 'liked') return likedIds();
    if (c.kind === 'songs') return sortedSongs();
    if (c.kind === 'track') return L.tracks.has(c.key) ? [c.key] : [];
    if (c.kind === 'search') return L.search(c.key).tracks;
    return [];
  }
  N.sortedAlbums = sortedAlbums;
  N.sortedArtists = sortedArtists;
  N.ctxIds = ctxIds;
  function ctxName(c) {
    if (!c) return '';
    if (c.kind === 'album') { const a = L.albums.get(c.key); return a ? a.name : c.name || ''; }
    if (c.kind === 'artist') { const a = L.artists.get(c.key); return a ? a.name : c.name || ''; }
    if (c.kind === 'pl') { const p = playlist(c.key); return p ? p.name : c.name || ''; }
    if (c.kind === 'liked') return 'Liked songs';
    if (c.kind === 'songs') return 'All songs';
    if (c.kind === 'search') return `“${c.key}”`;
    return c.name || '';
  }
  const ctxRoute = c => !c ? '' : c.kind === 'album' ? 'album/' + enc(c.key) : c.kind === 'artist' ? 'artist/' + enc(c.key) : c.kind === 'pl' ? 'pl/' + enc(c.key) : c.kind === 'liked' ? 'liked' : c.kind === 'songs' ? 'songs' : '';
  // the big play button on a page or card: pause/resume if it's what's playing, else start it
  function playCtx(c, startId, visible) {
    if (!startId && !visible && sameCtx(P.context(), c) && P.current()) { P.toggle(); return; }
    const ids = visible || ctxIds(c);
    if (!ids.length) { belt.toast('Nothing to play there yet'); return; }
    const i = startId ? Math.max(0, ids.indexOf(startId)) : S().queue.shuffle ? -1 : 0;
    P.playIds(ids, i, { kind: c.kind, key: c.key || '', name: ctxName(c) });
  }

  /* ---------- covers ---------- */
  function coverOf(c) {
    if (c.kind === 'album') { const a = L.albums.get(c.key); return a && a.cover; }
    if (c.kind === 'artist') { const a = L.artists.get(c.key); return a && a.cover; }
    if (c.kind === 'track') return L.get(c.key);
    const ids = ctxIds(c);
    return ids.map(L.get).find(t => ART.source(t)) || L.get(ids[0]);
  }
  // up to four different covers for a playlist; one if it has fewer
  function mosaic(ids, seed, label, size, cls = '') {
    const seen = new Set(), picks = [];
    for (const id of ids) {
      const t = L.get(id), src = ART.source(t);
      if (!t || !src) continue;
      const k = t.albumKey || src.key;
      if (seen.has(k)) continue;
      seen.add(k); picks.push(t);
      if (picks.length === 4) break;
    }
    if (picks.length < 4) return ART.html(picks[0] || null, seed, label, size, cls);
    return `<div class="art mosaic ${cls}">${picks.map(t => ART.html(t, t.id, t.T, 'sm')).join('')}</div>`;
  }
  const likedArt = (cls, name = '') => `<div class="art liked-art ${cls}"${name}><svg viewBox="0 0 24 24" style="width:42%;height:42%;fill:rgb(var(--hi));stroke:rgb(var(--hi));filter:drop-shadow(0 0 14px rgb(var(--hi) / .8))">${belt.icon('heart').replace(/^<svg[^>]*>|<\/svg>$/g, '')}</svg></div>`;
  async function glowFrom(el, t) {
    if (!el || !t) return;
    const u = await ART.url(t, 'md');
    if (u && el.isConnected) el.style.backgroundImage = `url("${u}")`;
  }

  /* ================================================================== routing */
  const H = { stack: ['home'], at: 0, scroll: {} };
  let lists = [], morphEl = null, renderToken = 0, filterRoute = '', listQuery = '';
  const route = () => H.stack[H.at];
  function focusTrack(id) {
    if (!id) return false;
    for (const l of lists) if (l.scrollTo(id)) return true;
    const filtered = lists.find(l => l.positions && l.o.ids.includes(id));
    if (filtered) {
      const inp = $('#view').querySelector('[data-lf]');
      inp.value = '';
      inp.dispatchEvent(new Event('input'));
      return filtered.scrollTo(id);
    }
    return false;
  }
  function go(r, { replace = false, focusId = '' } = {}) {
    closeMenu();
    if (np.open) closeNP();
    if (!replace && r === route()) {
      if (focusId && focusTrack(focusId)) return;
      $('#scroll').scrollTo({ top: 0, behavior: calm() ? 'auto' : 'smooth' });
      return;
    }
    H.scroll[route()] = $('#scroll').scrollTop;
    if (replace) H.stack[H.at] = r;
    else { H.stack = H.stack.slice(0, H.at + 1); H.stack.push(r); H.at++; if (H.stack.length > 60) { H.stack.shift(); H.at--; } }
    render({ transition: !replace, top: !replace, focusId });
  }
  function back() { if (H.at > 0) { H.scroll[route()] = $('#scroll').scrollTop; H.at--; render({ transition: true, restore: true }); } }
  function fwd() { if (H.at < H.stack.length - 1) { H.scroll[route()] = $('#scroll').scrollTop; H.at++; render({ transition: true, restore: true }); } }

  function render({ transition = false, restore = false, top = false, keep = false, focusId = '' } = {}) {
    const sc = $('#scroll'), keepAt = sc.scrollTop;
    const run = () => {
      const focused = filterRoute === route() && document.activeElement?.matches('[data-lf]') ? document.activeElement : null;
      draw();
      if (focused) {
        const input = $('#view').querySelector('[data-lf]');
        input?.focus({ preventScroll: true });
        input?.setSelectionRange(focused.selectionStart, focused.selectionEnd, focused.selectionDirection);
      }
      if (keep) sc.scrollTop = keepAt;
      else if (restore) sc.scrollTop = H.scroll[route()] || 0;
      else if (top) sc.scrollTop = 0;
      lists.forEach(l => l.paint(true));
      onScroll();
      if (focusId) focusTrack(focusId);
    };
    if (transition && document.startViewTransition && !calm() && belt.visible !== false) {
      const vt = document.startViewTransition(run);
      vt.finished.finally(() => { $$('[style*="view-transition-name"]').forEach(el => { el.style.viewTransitionName = ''; }); morphEl = null; });
    } else run();
  }
  function draw() {
    closeMenu();
    if (chartWatch) { chartWatch.disconnect(); chartWatch = null; }
    lists.forEach(l => l.destroy());
    lists = [];
    renderToken++;
    const r = route(), slash = r.indexOf('/'), kind = slash < 0 ? r : r.slice(0, slash), arg = slash < 0 ? '' : decodeURIComponent(r.slice(slash + 1));
    if (filterRoute !== r) { filterRoute = r; listQuery = ''; }
    const v = $('#view');
    v.className = 'v-' + kind;
    const views = { ...ADD.pages, home: vHome, songs: vSongs, albums: vAlbums, album: vAlbum, artists: vArtists, artist: vArtist, liked: vLiked, pl: vPlaylist, search: vSearch, settings: vSettings, stats: vStats, dupes: vDupes };
    (views[kind] || vHome)(v, arg);
    $$('#nav button').forEach(b => b.classList.toggle('on', b.dataset.go === kind));
    $$('#plList button').forEach(b => b.classList.toggle('on', kind === 'pl' && b.dataset.pl === arg));
    $('#openSet').classList.toggle('on', kind === 'settings');
    $$('[data-addon-route]').forEach(b => b.classList.toggle('on', b.dataset.addonRoute === kind));
    $('#back').disabled = H.at === 0;
    $('#fwd').disabled = H.at >= H.stack.length - 1;
    if (kind !== 'search' && document.activeElement !== $('#q')) $('#q').value = '';
    S().route = kind === 'search' ? 'home' : r;
    ART.fill(v);
    markNow();
  }
  // a card's cover flies into the page header (shared-element view transition)
  function morphFrom(el) {
    const a = el && el.querySelector('.art');
    if (!a || calm()) return;
    a.style.viewTransitionName = 'hero-art';
    morphEl = a;
  }
  const heroName = () => morphEl ? ' style="view-transition-name:hero-art"' : '';

  /* ================================================================== track lists (only the rows in view exist) */
  const ROW = 52;
  class TrackList {
    constructor(host, o) {
      this.o = Object.assign({ ids: [], ctx: null, numbered: false, album: true, extra: null, sortable: false, pl: null, extraOf: null }, o);
      this.ids = this.o.ids;
      this.sel = new Set();
      this.anchor = -1;
      this.rows = new Map();
      host.innerHTML = `<div class="tl${this.o.album ? '' : ' no-album'}">${this.head()}<div class="tl-body"></div></div>`;
      this.el = host.firstElementChild;
      this.body = $('.tl-body', this.el);
      this.body.style.height = this.ids.length * ROW + 'px';
      this.wire();
      lists.push(this);
    }
    head() {
      const s = S().sort, so = k => this.o.sortable ? ` class="sort${s.key === k ? ' on' : ''}" data-sort="${k}"` : '';
      const arrow = k => this.o.sortable && s.key === k ? (s.dir > 0 ? ' ↑' : ' ↓') : '';
      const extra = this.o.extra === 'added' ? 'Added' : this.o.extra === 'plays' ? 'Plays' : this.o.extra === 'liked' ? 'Liked' : '';
      return `<div class="tl-h"><span class="c-num">#</span><span${so('title')}>Title${arrow('title')}</span>` +
        `<span class="c-album"><span${so('album')}>Album${arrow('album')}</span></span>` +
        `<span class="c-extra"${this.o.extra === 'added' && this.o.sortable ? ` data-sort="added"` : ''}><span${this.o.extra === 'added' ? so('added') : ''}>${extra}${arrow('added')}</span></span>` +
        `<span></span><span class="c-dur"><span${so('duration')}>Time${arrow('duration')}</span></span><span></span></div>`;
    }
    extraText(t, i) {
      const x = this.o.extra;
      if (this.o.extraOf) return this.o.extraOf(t, i);
      if (x === 'added') return when(t.added);
      if (x === 'plays') { const p = S().plays[t.id]; return p ? plural(p[0], 'play') : ''; }
      return '';
    }
    rowHTML(i) {
      const id = this.ids[i], t = L.get(id);
      if (!t) return `<div class="tr missing" data-i="${i}" style="top:${i * ROW}px"><div class="c-num"><span class="n">${i + 1}</span></div><div class="c-title"><div class="tt"><span class="t-title ellip">Missing song</span><span class="t-sub ellip">${esc(id)}</span></div></div></div>`;
      const now = id === curId(), num = this.o.numbered ? (t.n || i + 1) : i + 1;
      const artist = t.A ? `<a class="link" data-artist="${attr(t.artistKey)}">${esc(t.A)}</a>` : '<span>Unknown artist</span>';
      return `<div class="tr${now ? ' now' : ''}${this.sel.has(i) ? ' sel' : ''}${t.unplayable ? ' missing' : ''}" data-i="${i}" draggable="true" style="top:${i * ROW}px">` +
        `<div class="c-num">${now ? '<span class="eq n"><i></i><i></i><i></i><i></i></span>' : `<span class="n">${num}</span>`}<button class="go" tabindex="-1">${icon(now && P.playing ? 'pausef' : 'playf')}</button></div>` +
        `<div class="c-title">${ART.html(t, t.albumKey || t.id, t.B || t.T, 'sm')}<div class="tt"><span class="t-title ellip">${esc(t.T)}</span><span class="t-sub ellip">${artist}</span></div></div>` +
        `<div class="c-album ellip">${t.B ? `<a class="link" data-album="${attr(t.albumKey)}">${esc(t.B)}</a>` : ''}</div>` +
        `<div class="c-extra ellip">${esc(this.extraText(t, i))}</div>` +
        `<div class="c-like"><button class="icon-btn like${isLiked(id) ? ' on' : ''}" data-like tabindex="-1" title="${isLiked(id) ? 'Remove from Liked' : 'Add to Liked'}">${belt.icon('heart')}</button></div>` +
        `<div class="c-dur">${t.duration ? mmss(t.duration) : '–'}</div>` +
        `<div class="c-more"><button class="icon-btn" data-more tabindex="-1" title="More">${icon('more')}</button></div></div>`;
    }
    paint(force) {
      if (!this.body.isConnected) return;
      const sc = $('#scroll'), top = this.body.getBoundingClientRect().top - sc.getBoundingClientRect().top, vh = sc.clientHeight;
      const a = Math.max(0, Math.floor(-top / ROW) - 8), z = Math.min(this.ids.length, Math.ceil((vh - top) / ROW) + 8);
      for (const [i, el] of this.rows) if (force || i < a || i >= z) { el.remove(); this.rows.delete(i); }
      let html = '';
      const add = [];
      for (let i = a; i < z; i++) if (!this.rows.has(i)) { html += this.rowHTML(i); add.push(i); }
      if (!html) return;
      const tmp = document.createElement('div');
      tmp.innerHTML = html;
      [...tmp.children].forEach((el, k) => { this.rows.set(add[k], el); this.body.appendChild(el); });
      ART.fill(this.body);
    }
    setIds(ids, positions = null) {
      this.ids = ids;
      this.positions = positions;
      this.sel.clear();
      this.anchor = -1;
      this.body.style.height = ids.length * ROW + 'px';
      this.paint(true);
    }
    scrollTo(id) {
      const i = this.ids.indexOf(id);
      if (i < 0 || !this.body.isConnected) return false;
      const sc = $('#scroll');
      const top = this.body.getBoundingClientRect().top - sc.getBoundingClientRect().top + sc.scrollTop;
      sc.scrollTo({ top: Math.max(0, top + i * ROW - sc.clientHeight / 2 + ROW / 2), behavior: calm() ? 'auto' : 'smooth' });
      this.paint(true);
      requestAnimationFrame(() => {
        this.paint(true);
        const row = this.rows.get(i);
        if (row) { row.classList.remove('flash'); void row.offsetWidth; row.classList.add('flash'); }
      });
      return true;
    }
    selected() { return [...this.sel].sort((a, b) => a - b).map(i => this.ids[i]).filter(id => L.tracks.has(id)); }
    playlistSelection() { return [...this.sel].map(i => this.o.entries[this.positions?.[i] ?? i]); }
    select(i, e) {
      if (e && (e.ctrlKey || e.metaKey)) { if (this.sel.has(i)) this.sel.delete(i); else this.sel.add(i); this.anchor = i; }
      else if (e && e.shiftKey && this.anchor >= 0) { const [a, b] = [Math.min(this.anchor, i), Math.max(this.anchor, i)]; this.sel.clear(); for (let k = a; k <= b; k++) this.sel.add(k); }
      else { this.sel.clear(); this.sel.add(i); this.anchor = i; }
      for (const [k, el] of this.rows) el.classList.toggle('sel', this.sel.has(k));
      activeList = this;
    }
    play(i) {
      const id = this.ids[i];
      if (!L.tracks.has(id)) return;
      if (id === curId() && sameCtx(P.context(), this.o.ctx)) { P.toggle(); return; }
      P.playIds(this.ids, i, this.o.ctx);
    }
    wire() {
      const b = this.body;
      b.addEventListener('mousedown', e => {
        if (this.o.ctx?.kind === 'liked' && e.target.closest('[data-like]') && document.activeElement?.matches('[data-lf]')) e.preventDefault();
      });
      b.addEventListener('click', e => {
        const row = e.target.closest('.tr'); if (!row) return;
        const i = +row.dataset.i;
        if (e.target.closest('.go')) { this.play(i); return; }
        if (e.target.closest('[data-like]')) { toggleLike([this.ids[i]], e.target.closest('[data-like]')); return; }
        if (e.target.closest('[data-more]')) { if (!this.sel.has(i)) this.select(i); const r = e.target.closest('[data-more]').getBoundingClientRect(); trackMenu(this.selected(), r.right, r.bottom, this, i); return; }
        const ar = e.target.closest('[data-artist]'); if (ar) { go('artist/' + enc(ar.dataset.artist)); return; }
        const al = e.target.closest('[data-album]'); if (al) { go('album/' + enc(al.dataset.album)); return; }
        this.select(i, e);
      });
      b.addEventListener('dblclick', e => { const row = e.target.closest('.tr'); if (row && !e.target.closest('button,a')) this.play(+row.dataset.i); });
      b.addEventListener('contextmenu', e => {
        const row = e.target.closest('.tr'); if (!row) return;
        e.preventDefault();
        const i = +row.dataset.i;
        if (!this.sel.has(i)) this.select(i);
        trackMenu(this.selected(), e.clientX, e.clientY, this, i);
      });
      b.addEventListener('dragstart', e => {
        const row = e.target.closest('.tr'); if (!row) return;
        const i = +row.dataset.i;
        if (!this.sel.has(i)) this.select(i);
        const ids = this.selected();
        if (!ids.length) { e.preventDefault(); return; }
        dragging = { ids, list: this, visible: this.ids, idx: [...this.sel].sort((a, c) => a - c) };
        e.dataTransfer.setData('application/x-string', JSON.stringify(ids));
        e.dataTransfer.effectAllowed = 'copyMove';
        const ghost = document.createElement('div');
        ghost.textContent = ids.length === 1 ? L.get(ids[0]).T : plural(ids.length, 'song');
        ghost.style.cssText = 'position:fixed;top:-100px;padding:8px 12px;background:#000;border:1px solid rgb(var(--hi)/.6);color:#fff;font:600 12px var(--ui)';
        document.body.appendChild(ghost);
        e.dataTransfer.setDragImage(ghost, 10, 10);
        setTimeout(() => ghost.remove());
      });
      b.addEventListener('dragend', () => { dragging = null; this.dropAt = null; $$('.drop-above,.drop-below', b).forEach(x => x.classList.remove('drop-above', 'drop-below')); });
      if (this.o.pl) {
        // reordering a playlist by dragging its rows
        b.addEventListener('dragover', e => {
          if (this.positions || dragging?.list !== this || dragging.visible !== this.ids) return;
          e.preventDefault();
          e.dataTransfer.dropEffect = 'move';
          $$('.drop-above,.drop-below', b).forEach(x => x.classList.remove('drop-above', 'drop-below'));
          const y = e.clientY - b.getBoundingClientRect().top, at = Math.max(0, Math.min(this.ids.length, Math.round(y / ROW)));
          const row = this.rows.get(at) || this.rows.get(at - 1);
          if (row) row.classList.add(this.rows.get(at) ? 'drop-above' : 'drop-below');
          this.dropAt = at;
        });
        b.addEventListener('drop', e => {
          if (this.positions || dragging?.list !== this || dragging.visible !== this.ids || !Number.isInteger(this.dropAt)) return;
          e.preventDefault();
          const p = playlist(this.o.pl); if (!p) return;
          if (p.items.length !== this.o.entries.length || p.items.some((item, i) => item !== this.o.entries[i])) return;
          const moving = dragging.idx.map(i => p.items[i]).filter(Boolean);
          const at = this.dropAt - dragging.idx.filter(i => i < this.dropAt).length;
          p.items = p.items.filter((_, i) => !dragging.idx.includes(i));
          p.items.splice(Math.max(0, at), 0, ...moving);
          p.updated = Date.now();
          N.save();
          this.o.entries = p.items.slice();
          this.o.ids = plIds(p);
          this.setIds(this.o.ids);
          for (let k = 0; k < moving.length; k++) this.sel.add(Math.max(0, at) + k);
          this.paint(true);
        });
      }
      const h = $('.tl-h', this.el);
      h.addEventListener('mousedown', e => {
        if (e.target.closest('[data-sort]') && document.activeElement?.matches('[data-lf]')) e.preventDefault();
      });
      h.addEventListener('click', e => {
        const s = e.target.closest('[data-sort]'); if (!s || !this.o.sortable) return;
        const k = s.dataset.sort, cur = S().sort;
        S().sort = { key: k, dir: cur.key === k ? -cur.dir : 1 };
        N.save();
        render({ keep: true });
      });
    }
    destroy() { this.rows.clear(); if (activeList === this) activeList = null; }
  }
  let activeList = null, dragging = null;

  /* ================================================================== cards and tiles */
  function card(o) {
    const now = o.ctx && sameCtx(P.context(), o.ctx) && P.current();
    return `<div class="card${o.round ? ' round' : ''}${now ? ' now' : ''}" data-open="${attr(o.open || '')}" data-ctx="${attr(JSON.stringify(o.ctx || null))}" style="animation-delay:${Math.min(o.i || 0, 14) * 28}ms">` +
      `<div class="cover">${o.art}<button class="play-big fab" data-play title="Play">${PP}</button></div>` +
      `<div class="t" title="${attr(o.title)}">${esc(o.title)}</div><div class="s">${esc(o.sub || '')}</div></div>`;
  }
  const albumCard = (a, i) => card({ i, open: 'album/' + enc(a.key), ctx: { kind: 'album', key: a.key }, art: ART.html(a.cover, a.key, a.name, 'md', '', i >= 24), title: a.name, sub: [a.artist, a.year || ''].filter(Boolean).join(' · ') });
  const artistCard = (r, i) => card({ i, round: true, open: 'artist/' + enc(r.key), ctx: { kind: 'artist', key: r.key }, art: ART.html(r.cover, 'artist:' + r.key, r.name, 'md', '', i >= 24), title: r.name, sub: plural(r.ids.length, 'song') });
  const trackCard = (t, i, rowIds, ctx) => card({ i, ctx: { kind: 'track', key: t.id, list: rowIds, from: ctx }, art: ART.html(t, t.albumKey || t.id, t.B || t.T, 'md'), title: t.T, sub: t.A || 'Unknown artist' });
  function wireCards(root) {
    root.addEventListener('click', e => {
      const c = e.target.closest('.card, .tile'); if (!c) return;
      const ctx = JSON.parse(c.dataset.ctx || 'null');
      if (e.target.closest('[data-play]') || (ctx && ctx.kind === 'track')) {
        e.stopPropagation();
        if (ctx && ctx.kind === 'track') {
          if (ctx.key === curId()) { P.toggle(); return; }
          const ids = (ctx.list || [ctx.key]).filter(id => L.tracks.has(id));
          P.playIds(ids, Math.max(0, ids.indexOf(ctx.key)), ctx.from || { kind: 'track', key: ctx.key, name: L.get(ctx.key).T });
        } else if (ctx && ctx.kind === 'shuffle') P.shuffleIds(L.all, { kind: 'songs', key: '', name: 'All songs' });
        else if (ctx) playCtx(ctx);
        return;
      }
      if (c.dataset.open) { morphFrom(c); go(c.dataset.open); }
    });
    root.addEventListener('contextmenu', e => {
      const c = e.target.closest('.card, .tile'); if (!c) return;
      const ctx = JSON.parse(c.dataset.ctx || 'null'); if (!ctx || ctx.kind === 'shuffle') return;
      e.preventDefault();
      const ids = ctx.kind === 'track' ? [ctx.key] : ctxIds(ctx);
      if (ids.length) trackMenu(ids, e.clientX, e.clientY, null, -1, ctx);
    });
  }
  function markNow() {
    const from = P.context(), cur = P.current();
    $$('.card, .tile').forEach(c => {
      const ctx = JSON.parse(c.dataset.ctx || 'null');
      const now = !!cur && !!ctx && (ctx.kind === 'track' ? ctx.key === cur.id : sameCtx(from, ctx));
      c.classList.toggle('now', now);
      c.classList.toggle('is-playing', now && P.playing);
    });
    $$('[data-hero-ctx]').forEach(h => {
      const ctx = JSON.parse(h.dataset.heroCtx);
      h.classList.toggle('is-playing', !!cur && sameCtx(from, ctx) && P.playing && !visibleIds(ctx));
    });
  }

  /* ================================================================== views */
  function pageTitle(h1, sub, extra = '') {
    return `<div class="page-title"><h1>${esc(h1)}</h1><span class="sub">${esc(sub)}</span><span class="grow"></span>${extra}</div>`;
  }
  function emptyLib(v) {
    if (!S().roots.length && !S().files.length) { welcome(v); return true; }
    if (!L.all.length) {
      const p = L.progress;
      v.innerHTML = `<div class="welcome"><div><div class="disc-hero"></div><h1>${p ? 'Reading your songs' : 'No songs here yet'}</h1>` +
        `<p>${p ? (p.phase === 'look' ? `Looking through your folders… ${p.done ? plural(p.done, 'song') + ' so far' : ''}` : `Reading tags: ${p.done.toLocaleString()} of ${p.total.toLocaleString()}`) : 'String found no songs in the folders you picked. It plays MP3, FLAC, M4A, OGG, Opus and WAV.'}</p>` +
        `${p ? '' : '<div class="row"><button class="btn primary" data-act="addFolder">' + belt.icon('folder') + 'Add a folder</button><button class="btn" data-go-to="settings">Library settings</button></div>'}</div></div>`;
      return true;
    }
    return false;
  }
  function welcome(v) {
    v.innerHTML = `<div class="welcome"><div><div class="disc-hero"></div><h1>Your records go here</h1>` +
      `<p>Pick the folders your songs are in. String plays them where they are: nothing is moved, copied or changed. Add more folders any time.</p>` +
      `<div class="row"><button class="btn primary big" data-act="addFolder">${belt.icon('folder')}Add a folder</button><button class="btn big" data-act="addFiles">${belt.icon('music')}Add songs</button></div>` +
      `<p style="margin-top:22px" class="scan-note">Or drop folders and songs anywhere on this window</p></div></div>`;
  }

  function vHome(v) {
    if (emptyLib(v)) return;
    const hr = new Date().getHours();
    const hello = hr < 5 ? 'Late night' : hr < 12 ? 'Good morning' : hr < 18 ? 'Good afternoon' : 'Good evening';
    const idsHere = L.ids(L.all), total = idsHere.reduce((s, id) => s + (L.get(id).duration || 0), 0);
    // shortcut tiles: liked, shuffle everything, then what was played lately
    const tiles = [], tiled = new Set();
    const tile = (ctx, art, name, open) => `<div class="tile" data-ctx="${attr(JSON.stringify(ctx))}" data-open="${attr(open || '')}">${art}<b class="ellip">${esc(name)}</b><button class="play-big fab" data-play title="Play">${PP}</button></div>`;
    tiles.push(tile({ kind: 'shuffle' }, `<div class="art">${ART.gen('shuffle-all', '', true)}<span class="tile-ic">${icon('shuffle')}</span></div>`, 'Shuffle everything', ''));
    if (likedIds().length) tiles.push(tile({ kind: 'liked', key: '' }, likedArt(''), 'Liked songs', 'liked'));
    for (const r of S().recent || []) {
      if (tiles.length >= 8) break;
      if (!['album', 'artist', 'pl'].includes(r.kind) || tiled.has(r.kind + ':' + r.key)) continue;
      const ids = ctxIds(r);
      if (!ids.length) continue;
      const name = ctxName(r);
      const art = r.kind === 'pl' ? mosaic(ids, 'pl:' + r.key, name, 'sm') : ART.html(coverOf(r), r.key, name, 'sm');
      tiles.push(tile({ kind: r.kind, key: r.key }, art, name, ctxRoute(r)));
      tiled.add(r.kind + ':' + r.key);
    }
    for (const p of S().playlists) { if (tiles.length >= 8) break; if (tiled.has('pl:' + p.id)) continue; tiled.add('pl:' + p.id); tiles.push(tile({ kind: 'pl', key: p.id }, mosaic(plIds(p), 'pl:' + p.id, p.name, 'sm'), p.name, 'pl/' + enc(p.id))); }
    let html = `<div class="page-title"><h1>${hello}</h1><span class="sub">${plural(idsHere.length, 'song')} · ${long(total)}</span></div><div class="tiles">${tiles.join('')}</div>`;
    const albums = [...L.albums.values()];
    const recentAlbums = albums.slice().sort((a, b) => b.added - a.added).slice(0, 14);
    const recentTracks = idsHere.sort((a, b) => (L.get(b).added || 0) - (L.get(a).added || 0)).slice(0, 14);
    const top = Object.entries(S().plays).filter(([id]) => L.tracks.has(id)).sort((a, b) => b[1][0] - a[1][0] || b[1][1] - a[1][1]).slice(0, 14).map(([id]) => id);
    const lately = Object.entries(S().plays).filter(([id]) => L.tracks.has(id)).sort((a, b) => b[1][1] - a[1][1]).slice(0, 14).map(([id]) => id);
    const row = (title, cards, link) => cards.length ? `<div class="sec-h"><h2>${title}</h2>${link ? `<a class="link scan-note" data-go-to="${link}">Show all</a>` : ''}</div><div class="row-scroll">${cards.join('')}</div>` : '';
    if (lately.length) html += row('Played lately', lately.map((id, i) => trackCard(L.get(id), i, lately, { kind: 'recent', key: 'lately', name: 'Played lately' })));
    if (recentAlbums.length >= 3) html += row('Recently added albums', recentAlbums.map(albumCard), 'albums');
    html += row('Recently added songs', recentTracks.map((id, i) => trackCard(L.get(id), i, recentTracks, { kind: 'recent', key: 'added', name: 'Recently added' })), 'songs');
    if (top.length >= 3) html += row('On repeat', top.map((id, i) => trackCard(L.get(id), i, top, { kind: 'recent', key: 'top', name: 'On repeat' })));
    const artists = [...L.artists.values()].filter(r => r.key).sort((a, b) => b.ids.length - a.ids.length).slice(0, 14);
    if (artists.length >= 2) html += row('Artists', artists.map(artistCard), 'artists');
    v.innerHTML = html;
  }

  const listFilterHTML = ph => `<label class="list-filter">${belt.icon('search')}<input class="field" data-lf placeholder="${attr(ph)}" spellcheck="false" autocomplete="off"><button class="icon-btn lf-x" data-lf-x hidden title="Clear">${belt.icon('x')}</button></label>`;
  const visibleIds = ctx => lists.find(l => l.positions && sameCtx(l.o.ctx, ctx))?.ids;
  function wireListFilter(v, tl) {
    const inp = $('[data-lf]', v), clr = $('[data-lf-x]', v);
    if (!inp || !tl) return;
    let emptyEl = null;
    const apply = () => {
      closeMenu();
      listQuery = inp.value;
      const q = listQuery.trim(), terms = L.norm(q).split(' ').filter(Boolean), allIds = tl.o.ids;
      if (clr) clr.hidden = !q;
      const positions = terms.length ? [] : null;
      if (positions) allIds.forEach((id, i) => {
        const t = L.get(id);
        if (t && terms.every(w => t.q.includes(w))) positions.push(i);
      });
      const matched = positions ? positions.map(i => allIds[i]) : allIds;
      tl.setIds(matched, positions);
      if (!matched.length && terms.length) {
        if (!emptyEl) { emptyEl = document.createElement('div'); emptyEl.className = 'empty-note tl-empty'; tl.el.appendChild(emptyEl); }
        emptyEl.textContent = 'No songs match';
      } else if (emptyEl) { emptyEl.remove(); emptyEl = null; }
      markNow();
    };
    inp.addEventListener('input', apply);
    inp.addEventListener('keydown', e => { if (e.key === 'Escape') { inp.value = ''; apply(); inp.blur(); } });
    if (clr) clr.addEventListener('click', e => { e.preventDefault(); inp.value = ''; apply(); inp.focus(); });
    if (listQuery) { inp.value = listQuery; apply(); }
  }

  function vSongs(v) {
    if (!listQuery && emptyLib(v)) return;
    const ids = sortedSongs(), total = ids.reduce((s, id) => s + (L.get(id).duration || 0), 0);
    const ctx = { kind: 'songs', key: '' };
    v.innerHTML = pageTitle('Songs', `${plural(ids.length, 'song')} · ${long(total)}`,
      `<div class="actions" data-hero-ctx="${attr(JSON.stringify(ctx))}">${listFilterHTML('Filter songs')}<button class="icon-btn" data-act="shuffleCtx" title="Shuffle">${icon('shuffle')}</button><button class="play-big" data-act="playCtx" title="Play">${PP}</button></div>`) + '<div id="list"></div>';
    v.dataset.ctx = JSON.stringify(ctx);
    const tl = new TrackList($('#list', v), { ids, ctx: { ...ctx, name: 'All songs' }, extra: 'added', sortable: true });
    wireListFilter(v, tl);
  }

  function vAlbums(v) {
    if (emptyLib(v)) return;
    const mode = S().albumSort, all = sortedAlbums();
    const seg = `<div class="seg" id="albumSort">${[['artist', 'Artist'], ['name', 'A–Z'], ['recent', 'Recent'], ['year', 'Year']].map(([k, n]) => `<button data-k="${k}" class="${mode === k ? 'on' : ''}">${n}</button>`).join('')}</div>`;
    v.innerHTML = pageTitle('Albums', plural(all.length, 'album'), seg) + (all.length ? `<div class="grid">${all.map(albumCard).join('')}</div>` : '<div class="empty-note">No albums: songs with an album tag show up here</div>');
    $('#albumSort', v).addEventListener('click', e => { const b = e.target.closest('[data-k]'); if (b) { S().albumSort = b.dataset.k; N.save(); render({ keep: true }); } });
  }

  function vArtists(v) {
    if (emptyLib(v)) return;
    const all = sortedArtists();
    v.innerHTML = pageTitle('Artists', plural(all.filter(a => a.key).length, 'artist')) + `<div class="grid">${all.map(artistCard).join('')}</div>`;
  }

  function hero({ kind, art, label, title, meta, ctx, round, editable, more, filter }) {
    return `<div class="hero${round ? ' round' : ''}"><div class="glow" id="glow"></div>${art}<div class="hero-t"><span class="lbl">${esc(label)}</span>` +
      `<h1${editable ? ' class="edit" data-act="renamePl" title="Rename"' : ''}>${esc(title)}</h1><div class="meta">${meta}</div></div></div>` +
      `<div class="actions" data-hero-ctx="${attr(JSON.stringify(ctx))}"><button class="play-big" data-act="playCtx" title="Play">${PP}</button>` +
      `<button class="icon-btn" data-act="shuffleCtx" title="Shuffle">${icon('shuffle')}</button>${more ? `<button class="icon-btn" data-act="ctxMenu" title="More">${icon('more')}</button>` : ''}${filter ? listFilterHTML(filter) : ''}</div>`;
  }

  function vAlbum(v, key) {
    const a = L.albums.get(key);
    if (!a) { v.innerHTML = '<div class="empty-note" style="margin-top:20px">That album isn\'t in the library any more</div>'; return; }
    const ctx = { kind: 'album', key };
    v.dataset.ctx = JSON.stringify(ctx);
    const meta = `<a class="link" data-artist="${attr(a.artistKey)}"><b>${esc(a.artist)}</b></a>${a.year ? ' · ' + a.year : ''} · ${plural(a.ids.length, 'song')}, ${long(a.duration)}`;
    v.innerHTML = hero({ art: ART.html(a.cover, a.key, a.name, 'md', 'big').replace('<div class="art', `<div${heroName()} class="art`), label: 'Album', title: a.name, meta, ctx, more: true }) + '<div id="list"></div>';
    glowFrom($('#glow', v), a.cover);
    new TrackList($('#list', v), { ids: a.ids, ctx: { ...ctx, name: a.name }, numbered: true, album: false });
  }

  function vArtist(v, key) {
    const r = L.artists.get(key);
    if (!r) { v.innerHTML = '<div class="empty-note" style="margin-top:20px">That artist isn\'t in the library any more</div>'; return; }
    const ctx = { kind: 'artist', key };
    v.dataset.ctx = JSON.stringify(ctx);
    const plays = id => (S().plays[id] || [0])[0];
    const popular = r.ids.slice().sort((x, y) => plays(y) - plays(x)).slice(0, 5);
    const albums = [...r.albums].map(k => L.albums.get(k)).filter(Boolean).sort((x, y) => (y.year || 0) - (x.year || 0));
    const meta = `${plural(r.ids.length, 'song')}${albums.length ? ' · ' + plural(albums.length, 'album') : ''}`;
    v.innerHTML = hero({ art: ART.html(r.cover, 'artist:' + key, r.name, 'md', 'big').replace('<div class="art', `<div${heroName()} class="art`), label: 'Artist', title: r.name, meta, ctx, round: true, more: true }) +
      `<div class="sec-h"><h2>${popular.some(id => plays(id)) ? 'Popular' : 'Songs'}</h2></div><div id="pop"></div>` +
      (albums.length ? `<div class="sec-h"><h2>Albums</h2></div><div class="grid">${albums.map(albumCard).join('')}</div>` : '') +
      (r.ids.length > 5 ? `<div class="sec-h"><h2>All songs</h2></div><div id="list"></div>` : '');
    glowFrom($('#glow', v), r.cover);
    new TrackList($('#pop', v), { ids: popular, ctx: { ...ctx, name: r.name }, extra: 'plays' });
    if (r.ids.length > 5) new TrackList($('#list', v), { ids: r.ids, ctx: { ...ctx, name: r.name } });
  }

  function vLiked(v) {
    const ids = likedIds(), ctx = { kind: 'liked', key: '' };
    const showList = ids.length || listQuery;
    v.dataset.ctx = JSON.stringify(ctx);
    const total = ids.reduce((s, id) => s + (L.get(id).duration || 0), 0);
    v.innerHTML = hero({ art: likedArt('big'), label: 'Playlist', title: 'Liked songs', meta: ids.length ? `${plural(ids.length, 'song')}, ${long(total)}` : 'Songs you like show up here', ctx, filter: showList ? 'Filter liked songs' : '' }) +
      (showList ? '<div id="list"></div>' : `<div class="empty-note">Press ${belt.icon('heart').replace('<svg', '<svg style="width:13px;height:13px;vertical-align:-2px"')} on a song (or L while it plays) to keep it here</div>`);
    if (showList) wireListFilter(v, new TrackList($('#list', v), { ids, ctx: { ...ctx, name: 'Liked songs' }, extra: 'liked', extraOf: t => when(S().liked[t.id]) }));
  }

  function vPlaylist(v, id) {
    const p = playlist(id);
    if (!p) { v.innerHTML = '<div class="empty-note" style="margin-top:20px">That playlist was deleted</div>'; return; }
    const ctx = { kind: 'pl', key: id };
    v.dataset.ctx = JSON.stringify(ctx);
    const ids = plIds(p), have = L.ids(ids), total = have.reduce((s, x) => s + (L.get(x).duration || 0), 0);
    const showList = ids.length || listQuery;
    const meta = ids.length ? `${plural(ids.length, 'song')}, ${long(total)}${have.length < ids.length ? ` · ${ids.length - have.length} not found` : ''}` : 'Empty: add songs from their ⋯ menu, or drag them onto the playlist';
    v.innerHTML = hero({ art: mosaic(ids, 'pl:' + id, p.name, 'md', 'big').replace('<div class="art', `<div${heroName()} class="art`), label: 'Playlist', title: p.name, meta, ctx, editable: true, more: true, filter: showList ? 'Filter playlist' : '' }) + '<div id="list"></div>';
    glowFrom($('#glow', v), have.map(L.get).find(t => ART.source(t)));
    const at = new Map(p.items.map(x => [x.id, x.at]));
    if (showList) wireListFilter(v, new TrackList($('#list', v), { ids, ctx: { ...ctx, name: p.name }, pl: id, entries: p.items.slice(), extra: 'added', extraOf: t => when(at.get(t.id)) }));
  }

  function vSearch(v) {
    const q = $('#q').value.trim();
    if (!q) { v.innerHTML = pageTitle('Search', 'Type above to find songs, albums, artists and playlists'); return; }
    const r = L.search(q), ctx = { kind: 'search', key: q };
    let html = pageTitle(`“${q}”`, r.tracks.length + r.albums.length + r.artists.length + r.playlists.length ? `${plural(r.tracks.length, 'song')} · ${plural(r.albums.length, 'album')} · ${plural(r.artists.length, 'artist')}` : 'Nothing matches');
    if (r.artists.length) html += `<div class="sec-h"><h2>Artists</h2></div><div class="row-scroll">${r.artists.slice(0, 14).map(k => artistCard(L.artists.get(k))).join('')}</div>`;
    if (r.albums.length) html += `<div class="sec-h"><h2>Albums</h2></div><div class="row-scroll">${r.albums.slice(0, 14).map(k => albumCard(L.albums.get(k))).join('')}</div>`;
    if (r.playlists.length) html += `<div class="sec-h"><h2>Playlists</h2></div><div class="row-scroll">${r.playlists.map((id, i) => { const p = playlist(id); return card({ i, open: 'pl/' + enc(id), ctx: { kind: 'pl', key: id }, art: mosaic(plIds(p), 'pl:' + id, p.name, 'md'), title: p.name, sub: plural(p.items.length, 'song') }); }).join('')}</div>`;
    if (r.tracks.length) html += `<div class="sec-h"><h2>Songs</h2></div><div id="list"></div>`;
    v.innerHTML = html;
    if (r.tracks.length) new TrackList($('#list', v), { ids: r.tracks, ctx: { ...ctx, name: `“${q}”` } });
  }

  /* ================================================================== Stats: listening time and top artists */
  const STAT_KINDS = [['day', 'Today', 'Yesterday'], ['week', 'This week', 'Last week'], ['month', 'This month', 'Last month'], ['year', 'This year', 'Last year'], ['all', 'All time', '']];
  // 3,725 s -> "1 hr 2 min"; short: "1h 2m"
  function dur(s, short) {
    s = Math.round(s || 0);
    if (s < 60) return s ? (short ? '<1m' : '< 1 min') : (short ? '0m' : '0 min');
    const h = Math.floor(s / 3600), m = Math.floor(s % 3600 / 60);
    if (short) return h ? (m ? `${h}h ${m}m` : `${h}h`) : `${m}m`;
    return h ? (m ? `${h} hr ${m} min` : `${h} hr`) : `${m} min`;
  }
  // where phone listening comes from (Navidrome's history, phone.js), and a way to stop counting it
  function phoneNote() {
    const ph = S().phone || {};
    if (S().set.phoneStats === false) return `<p class="stat-phone">Phone listening isn't counted · <a class="link" data-phone="on">count it</a></p>`;
    if (!ph.found) return '';
    const users = N.phone.users;
    const chosen = S().set.phoneUser || '', accounts = Object.entries(users);
    const picker = accounts.length > 1 || (accounts.length && !Object.hasOwn(users, chosen))
      ? `<label class="stat-phone">Count phone listening for <select class="field sel" id="phoneUser"><option value="" disabled${Object.hasOwn(users, chosen) ? '' : ' selected'}>Choose your Navidrome account</option>${accounts.map(([id, name]) => `<option value="${attr(id)}"${id === chosen ? ' selected' : ''}>${esc(name)}</option>`).join('')}</select></label>` : '';
    if (picker && !Object.hasOwn(users, chosen)) return picker;
    const min = ph.at ? Math.max(0, Math.round((Date.now() - ph.at) / 60000)) : -1;
    const ago = min < 0 ? '' : ` · checked ${min < 1 ? 'just now' : min < 60 ? min + ' min ago' : Math.round(min / 60) + ' h ago'}`;
    return picker + `<p class="stat-phone" title="Navidrome only hears about a song once it's been played about halfway, so a phone play counts as the whole song">` +
      `Phone listening comes from Navidrome on this PC: ${plural(ph.plays || 0, 'play')} counted, as whole songs${ago}${ph.error ? ' · couldn\'t read it last time' : ''} · <a class="link" data-phone="off">stop counting</a></p>`;
  }
  document.addEventListener('click', e => {
    const b = e.target.closest('[data-phone]');
    if (!b) return;
    S().set.phoneStats = b.dataset.phone === 'on';
    N.save();
    if (S().set.phoneStats) N.phone.sync();
    render({ keep: true });
  });
  document.addEventListener('change', e => {
    if (e.target.id !== 'phoneUser' || !e.target.value) return;
    N.phone.choose(e.target.value);
  });
  N.on('phone', () => { statsToday(); if (route() === 'stats') render({ keep: true }); });
  function vStats(v) {
    const M = N.stats.model, now = new Date(), kind = S().statsKind || 'week';
    const first = M.firstDay();
    const tiles = STAT_KINDS.slice(0, 4).map(([k, name, prevName]) => {
      const p = M.period(k, now), cur = M.range(p.from, p.to).total, prev = M.range(p.prevFrom, p.prevTo).total;
      return `<button class="kpi stat-tile${k === kind ? ' on' : ''}" data-k="${k}"><span class="lbl">${name}</span><div class="v">${dur(cur)}</div><div class="s">${prevName} ${dur(prev)}</div></button>`;
    }).join('');
    const streak = M.streak(now);
    const sub = first ? `Since ${first.toLocaleDateString([], { month: 'short', day: 'numeric', year: 'numeric' })}${streak > 1 ? ` · ${streak}-day streak` : ''}` : 'Time spent listening, counted as you play';
    const p = M.period(kind, now), r = M.range(p.from, p.to), vals = M.series(p);
    const chartTitle = { day: 'Today, by hour', week: 'This week, by day', month: 'This month, by day', year: 'This year, by month', all: 'All time, by month' }[kind];
    let html = pageTitle('Stats', sub) + `<div class="stat-tiles">${tiles}</div>` +
      `<div class="seg stat-seg" id="statSeg">${STAT_KINDS.map(([k, name]) => `<button data-k="${k}" class="${k === kind ? 'on' : ''}">${name}</button>`).join('')}</div>`;
    if (!first) {
      v.innerHTML = html + `<div class="empty-note" style="margin-top:18px">Play something: the time you spend listening adds up here, with your top artists and songs</div>`;
      wireStats(v);
      return;
    }
    html += `<div class="panel chart-panel"><div class="panel-h"><span class="lbl">${chartTitle}</span><span class="note">${dur(r.total)} in all${r.phone ? ` · ${dur(r.phone)} on your phone` : ''}</span><button class="btn sm ghost" id="chartAsTable">${S().statsTable ? 'Chart' : 'Table'}</button></div><div class="chart" id="chart"></div></div>`;
    const topA = r.artists.slice(0, 10), topT = r.tracks.slice(0, 10);
    const maxA = topA.length ? topA[0].secs : 1, maxT = topT.length ? topT[0].secs : 1;
    const artistRow = (a, i) => {
      const key = L.norm(a.name), lib = L.artists.get(key);
      const art = ART.html(lib ? lib.cover : null, 'artist:' + key, a.name || 'Unknown', 'sm', 'round');
      return `<div class="rank${lib && a.name ? ' link-row' : ''}"${lib && a.name ? ` data-artist="${attr(key)}"` : ''}><span class="n">${i + 1}</span>${art}<div class="tt"><b class="ellip">${esc(a.name || 'Unknown artist')}</b><div class="rbar"><i style="width:${(a.secs / maxA * 100).toFixed(1)}%"></i></div></div><span class="rv">${dur(a.secs, true)}</span></div>`;
    };
    const topIds = topT.map(x => x.id).filter(id => L.tracks.has(id));
    const songRow = (x, i) => {
      const t = L.get(x.id);
      const art = t ? ART.html(t, t.albumKey || t.id, t.B || t.T, 'sm') : ART.html(null, x.id, x.title, 'sm');
      return `<div class="rank${t ? ' link-row' : ' gone'}"${t ? ` data-play-top="${attr(x.id)}"` : ''} title="${t ? 'Play' : 'Not in your library any more'}"><span class="n">${i + 1}</span>${art}<div class="tt"><b class="ellip">${esc(t ? t.T : x.title)}</b><span class="t-sub ellip">${esc((t ? t.A : x.artist) || 'Unknown artist')}</span><div class="rbar"><i style="width:${(x.secs / maxT * 100).toFixed(1)}%"></i></div></div><span class="rv">${dur(x.secs, true)}</span></div>`;
    };
    const periodName = STAT_KINDS.find(k => k[0] === kind)[1].toLowerCase();
    html += `<div class="stat-cols"><div class="panel"><div class="panel-h"><span class="lbl">Top artists</span><span class="note">${periodName}</span></div>${topA.length ? topA.map(artistRow).join('') : '<div class="empty-note">Nothing yet</div>'}</div>` +
      `<div class="panel"><div class="panel-h"><span class="lbl">Top songs</span><span class="note">${periodName}</span></div>${topT.length ? topT.map(songRow).join('') : '<div class="empty-note">Nothing yet</div>'}</div></div>` + phoneNote();
    v.innerHTML = html;
    wireStats(v);
    const chart = $('#chart', v);
    if (S().statsTable) chartTable(chart, p, vals); else chartBars(chart, p, vals);
    $('#chartAsTable', v).addEventListener('click', () => { S().statsTable = !S().statsTable; N.save(); render({ keep: true }); });
    $$('[data-play-top]', v).forEach(el => el.addEventListener('click', () => {
      const id = el.dataset.playTop;
      if (id === curId()) { P.toggle(); return; }
      P.playIds(topIds, Math.max(0, topIds.indexOf(id)), { kind: 'recent', key: 'top-' + kind, name: 'Top songs, ' + periodName });
    }));
    if (window.ResizeObserver) { let w = chart.clientWidth; chartWatch = new ResizeObserver(() => { if (chart.isConnected && Math.abs(chart.clientWidth - w) > 4 && !S().statsTable) { w = chart.clientWidth; chartBars(chart, p, vals); } }); chartWatch.observe(chart); }
    statsDrawn = Date.now();
  }
  let chartWatch = null, statsDrawn = 0;
  function wireStats(v) {
    const pick = k => { if (k && k !== S().statsKind) { S().statsKind = k; N.save(); render({ keep: true }); } };
    $('#statSeg', v).addEventListener('click', e => { const b = e.target.closest('[data-k]'); if (b) pick(b.dataset.k); });
    $$('.stat-tile', v).forEach(b => b.addEventListener('click', () => pick(b.dataset.k)));
  }
  // one series of bars in the tint colour: <= 24 px wide, 4 px rounded tops on a shared baseline, hairline grid,
  // the peak labelled, every bar with a tooltip; the current hour / day / month stands out
  function chartBars(el, p, vals) {
    const W = Math.max(320, el.clientWidth || 800), H = 230, L0 = 52, R0 = 10, T0 = 24, B0 = 28;
    const iw = W - L0 - R0, ih = H - T0 - B0, n = vals.length, max = Math.max(0, ...vals);
    // round ticks: minutes under 90 min, hours above
    const mins = max < 5400;
    const steps = mins ? [60, 120, 300, 600, 900, 1800] : [3600, 7200, 10800, 21600, 43200, 86400, 172800, 432000, 864000];
    const step = steps.find(s => max / s <= 4) || steps[steps.length - 1];
    const top = Math.max(step, Math.ceil(max / step) * step);
    const y = s => T0 + ih - s / top * ih;
    const slot = iw / n, bw = Math.min(24, Math.max(3, slot * .62));
    const tickLabel = s => s === 0 ? '0' : mins ? `${s / 60}m` : `${s / 3600}h`;
    let svg = `<svg class="bars" viewBox="0 0 ${W} ${H}" width="${W}" height="${H}" role="img" aria-label="${attr(`${p.units.length} bars; the most is ${dur(max)}`)}">`;
    for (let s = 0; s <= top + 1; s += step) svg += `<line class="grid" x1="${L0}" x2="${W - R0}" y1="${y(s).toFixed(1)}" y2="${y(s).toFixed(1)}"/><text class="tick" x="${L0 - 10}" y="${(y(s) + 4).toFixed(1)}" text-anchor="end">${tickLabel(s)}</text>`;
    const peak = vals.indexOf(max);
    vals.forEach((v, i) => {
      const x = L0 + i * slot + (slot - bw) / 2, h = v / top * ih, base = T0 + ih;
      const cls = i === p.current ? 'b cur' : i > p.current && p.kind !== 'all' ? 'b later' : 'b';
      if (h >= .5) {
        const r = Math.min(4, bw / 2, h), yt = base - h;
        svg += `<path class="${cls}" style="animation-delay:${Math.min(i, 30) * 14}ms" d="M${x.toFixed(1)} ${base} V${(yt + r).toFixed(1)} A${r} ${r} 0 0 1 ${(x + r).toFixed(1)} ${yt.toFixed(1)} H${(x + bw - r).toFixed(1)} A${r} ${r} 0 0 1 ${(x + bw).toFixed(1)} ${(yt + r).toFixed(1)} V${base} Z"/>`;
      }
      if (p.units[i].label) svg += `<text class="xl${i === p.current ? ' cur' : ''}" x="${(L0 + i * slot + slot / 2).toFixed(1)}" y="${H - 8}" text-anchor="middle">${esc(p.units[i].label)}</text>`;
      svg += `<rect class="hit" data-i="${i}" x="${(L0 + i * slot).toFixed(1)}" y="${T0}" width="${slot.toFixed(1)}" height="${ih}"/>`;
    });
    if (max > 0) svg += `<text class="peak" x="${(L0 + peak * slot + slot / 2).toFixed(1)}" y="${(y(max) - 8).toFixed(1)}" text-anchor="middle">${dur(max, true)}</text>`;
    svg += `<line class="base" x1="${L0}" x2="${W - R0}" y1="${T0 + ih}" y2="${T0 + ih}"/></svg><div class="chart-tip" hidden></div>`;
    el.innerHTML = svg;
    const tip = $('.chart-tip', el), svgEl = $('svg', el);
    svgEl.addEventListener('pointermove', e => {
      const hit = e.target.closest('.hit');
      if (!hit) { tip.hidden = true; $$('.hit.on', el).forEach(x => x.classList.remove('on')); return; }
      const i = +hit.dataset.i;
      $$('.hit.on', el).forEach(x => x !== hit && x.classList.remove('on'));
      hit.classList.add('on');
      tip.innerHTML = `<span>${esc(p.units[i].long)}</span><b>${dur(vals[i])}</b>`;
      tip.hidden = false;
      const er = el.getBoundingClientRect(), hr = hit.getBoundingClientRect();
      tip.style.left = Math.max(70, Math.min(er.width - 70, hr.left - er.left + hr.width / 2)) + 'px';
      tip.style.top = Math.max(0, y(vals[i]) - 46) + 'px';
    });
    svgEl.addEventListener('pointerleave', () => { tip.hidden = true; $$('.hit.on', el).forEach(x => x.classList.remove('on')); });
  }
  function chartTable(el, p, vals) {
    el.innerHTML = `<table class="tbl stat-tbl"><tr><th>${p.kind === 'day' ? 'Hour' : p.kind === 'year' || p.kind === 'all' ? 'Month' : 'Day'}</th><th>Listened</th></tr>${p.units.map((u, i) => `<tr${i === p.current ? ' class="cur"' : ''}><td>${esc(u.long)}</td><td>${dur(vals[i])}</td></tr>`).join('')}</table>`;
  }

  /* ================================================================== duplicates: songs the library has more than once
     The Duplicates page (route `dupes`, from Library → Duplicates). N.dupes.duplicates (dupes.js) finds them, best copy first;
     you can play each copy, pick the one to keep, and tick the ones to drop (copies surely the same are ticked
     for them; "maybe" ones aren't), then hide those in String or move them to the Recycle Bin. Either way the kept
     copy takes over their likes, plays, loops, listening time and playlist places. */
  const DUP = { groups: null, ver: -1, keep: [], drop: new Set() };
  const copies = n => n === 1 ? '1 copy' : `${n.toLocaleString()} copies`;
  function findDupes() {
    const s = S(), hidden = s.hidden || {}, all = [...L.tracks.values()].filter(t => !hidden[t.id]);
    const inPl = new Set(s.playlists.flatMap(p => p.items.map(x => x.id)));
    const per = new Map();
    for (const t of all) per.set(t.dir, (per.get(t.dir) || 0) + 1);
    // after the sound itself: the copy in the folder that holds most songs; likes, playlists and plays only break a
    // tie (they move over to whichever copy stays)
    const prefer = t => 4 * (per.get(t.dir) || 0) / Math.max(1, all.length) + (s.liked[t.id] ? 1 : 0) + (inPl.has(t.id) ? 1 : 0) + ((s.plays[t.id] || [0])[0] ? 0.5 : 0);
    const apart = (a, b) => !!(s.notDupes || {})[pairKey(a.id, b.id)];
    DUP.groups = N.dupes.duplicates(all, { prefer, apart });
    DUP.ver = L.version;
    DUP.keep = DUP.groups.map(g => g[0].id);
    DUP.drop = new Set();
    DUP.groups.forEach((g, gi) => tickGroup(gi));
  }
  const pairKey = (a, b) => a < b ? a + '\u0001' + b : b + '\u0001' + a;
  // "Not the same" on a maybe group: its copies that aren't surely the same stop being linked, for good
  function notSame(gi) {
    const g = DUP.groups[gi], s = S();
    s.notDupes = s.notDupes || {};
    for (let i = 0; i < g.length; i++) for (let j = i + 1; j < g.length; j++) if (g.sets[i] !== g.sets[j]) s.notDupes[pairKey(g[i].id, g[j].id)] = Date.now();
    N.save(); findDupes(); render({ keep: true });
  }
  // the copies surely the same as the kept one are ticked; "maybe" ones wait for you
  function tickGroup(gi) {
    const g = DUP.groups[gi], k = g.findIndex(t => t.id === DUP.keep[gi]);
    g.forEach((t, i) => { if (i !== k && g.sets[i] === g.sets[k]) DUP.drop.add(t.id); else DUP.drop.delete(t.id); });
  }
  function hiddenNote() {
    const n = Object.keys(S().hidden || {}).length;
    return n ? `<p class="dup-hidden">${plural(n, 'song')} hidden from String · <a class="link" id="dupUnhide">show them again</a></p>` : '';
  }
  function apartNote() {
    const n = Object.keys(S().notDupes || {}).length;
    return n ? `<p class="dup-hidden">${n === 1 ? '1 pair' : n + ' pairs'} you marked as not the same · <a class="link" id="dpForget">list them again</a></p>` : '';
  }
  const kbps = t => t.duration > 0 && t.size ? Math.round(t.size * 8 / t.duration / 1000) : 0;
  function usedFor(t) {
    const s = S(), bits = [], pls = s.playlists.filter(p => p.items.some(x => x.id === t.id)).length, n = (s.plays[t.id] || [0])[0];
    if (s.liked[t.id]) bits.push('liked');
    if (pls) bits.push(`in ${plural(pls, 'playlist')}`);
    if (n) bits.push(plural(n, 'play'));
    return bits.join(' · ');
  }
  function vDupes(v) {
    if (!DUP.groups || DUP.ver !== L.version) findDupes();
    const G = DUP.groups, cur = (P.current() || {}).id;
    const drops = [...DUP.drop].map(L.get).filter(Boolean), mb = drops.reduce((n, t) => n + (t.size || 0), 0) / 1048576;
    const head = pageTitle('Duplicates', 'Songs you have more than once', `<button class="btn sm" id="dpAgain">${belt.icon('reload')}Look again</button>`);
    if (!G.length) {
      v.innerHTML = head + `<div class="empty-note">No duplicates: every song is here once</div>` + hiddenNote() + apartNote();
      $('#dpAgain', v).addEventListener('click', () => { findDupes(); render({ keep: true }); });
      return;
    }
    const extra = G.reduce((n, g) => n + g.length - 1, 0), maybes = G.filter(g => !g.sure).length;
    const row = (t, i, g, gi) => {
      const keep = t.id === DUP.keep[gi], drop = DUP.drop.has(t.id);
      const credit = g.sure ? '' : `${t.T}${t.A ? ' · ' + t.A : ''}`;
      return `<div class="dp-row${keep ? ' keep' : ''}${drop ? ' drop' : ''}${t.id === cur ? ' playing' : ''}">` +
        `<button class="icon-btn dp-play" data-dp-play="${attr(t.id)}" title="Play this copy">${icon(t.id === cur && P.playing ? 'pausef' : 'playf')}</button>` +
        `<div class="dp-tt"><b class="ellip" title="${attr(t.path)}">${esc(t.name)}</b><span class="ellip" title="${attr(t.path)}">${esc(t.dir)}</span></div>` +
        `<div class="dp-meta"><span>${esc((t.format || '?').toUpperCase())}${kbps(t) ? ` · ${kbps(t)} kbps` : ''} · ${(t.size / 1048576).toFixed(1)} MB · ${mmss(t.duration)}</span>` +
        `<span class="ellip">${esc([credit, usedFor(t)].filter(Boolean).join(' · '))}</span></div>` +
        `<label class="dp-pick"><input type="radio" name="dpk${gi}" data-dp-keep="${gi}" value="${attr(t.id)}"${keep ? ' checked' : ''}><span>${keep ? 'Keeping' : 'Keep'}</span></label>` +
        `<label class="dp-pick${keep ? ' off' : ''}"><input type="checkbox" data-dp-drop="${attr(t.id)}"${drop ? ' checked' : ''}${keep ? ' disabled' : ''}><span>Remove</span></label>` +
        `<button class="icon-btn" data-dup-show="${attr(t.id)}" title="Show in Explorer">${belt.icon('folder')}</button></div>`;
    };
    v.innerHTML = head +
      `<div class="panel dp-sum"><div class="dp-count"><b>${G.length.toLocaleString()}</b><span>${G.length === 1 ? 'song has' : 'songs have'} ${copies(extra)} too many</span></div>` +
      `<p>For each song, String picks the copy to keep: the best sound, then the one in the folder with most of your songs. Copies that are surely the same song are ticked.${maybes ? ` ${maybes === 1 ? 'One song is' : maybes + ' songs are'} only <em>maybe</em> the same: the same name and length, but another credit or version. Play them to compare: tick what you want gone, or press Not the same and they stop being listed.` : ''}</p>` +
      `<div class="dp-act"><span class="scan-note">${drops.length ? `${copies(drops.length)} ticked · ${mb.toFixed(1)} MB` : 'nothing ticked'}</span><span class="grow"></span>` +
      `<button class="btn sm" data-dp-fix="hide"${drops.length ? '' : ' disabled'} title="The files stay where they are; String leaves them out">Hide in String</button>` +
      `<button class="btn sm danger" data-dp-fix="recycle"${drops.length ? '' : ' disabled'}>${belt.icon('trash')}Move to the Recycle Bin</button></div>` +
      `<p class="dp-fine">Either way, the copy you keep takes over their likes, plays, loops, listening time and playlist places.</p></div>` +
      G.map((g, gi) => `<div class="dp-group${g.sure ? '' : ' maybe'}"><div class="dp-h"><b class="ellip">${esc(g[0].T)}</b><span class="ellip">${esc([...new Set(g.map(t => t.A).filter(Boolean))].join(' / '))}</span>` +
        `<em>${g.sure ? `${g.length} copies` : 'Maybe the same'}</em>${g.sure ? '' : `<button class="btn sm ghost" data-dp-apart="${gi}" title="They're different songs or versions: stop listing them">Not the same</button>`}</div>${g.map((t, i) => row(t, i, g, gi)).join('')}</div>`).join('') + hiddenNote() + apartNote();
    $('#dpAgain', v).addEventListener('click', () => { findDupes(); render({ keep: true }); });
    $$('[data-dp-keep]', v).forEach(r => r.addEventListener('change', () => { const gi = +r.dataset.dpKeep; DUP.keep[gi] = r.value; tickGroup(gi); render({ keep: true }); }));
    $$('[data-dp-drop]', v).forEach(c => c.addEventListener('change', () => { if (c.checked) DUP.drop.add(c.dataset.dpDrop); else DUP.drop.delete(c.dataset.dpDrop); render({ keep: true }); }));
    $$('[data-dp-play]', v).forEach(b => b.addEventListener('click', () => { const id = b.dataset.dpPlay; if ((P.current() || {}).id === id) P.toggle(); else P.playIds([id], 0, null); }));
    $$('[data-dp-fix]', v).forEach(b => b.addEventListener('click', () => fixDupes(b.dataset.dpFix).catch(e => belt.toast(e.message))));
    $$('[data-dp-apart]', v).forEach(b => b.addEventListener('click', () => notSame(+b.dataset.dpApart)));
  }
  // the play buttons follow the player without redrawing the page
  function dupesPlaying() {
    if (route() !== 'dupes') return;
    const cur = (P.current() || {}).id;
    $$('[data-dp-play]').forEach(b => {
      const me = b.dataset.dpPlay === cur;
      b.innerHTML = icon(me && P.playing ? 'pausef' : 'playf');
      b.closest('.dp-row').classList.toggle('playing', me);
    });
  }
  N.on('track', dupesPlaying);
  N.on('state', dupesPlaying);

  // the copy that stays takes over what the dropped one had
  function mergeCopy(k, d) {
    const s = S();
    if (s.liked[d.id]) { if (!s.liked[k.id]) s.liked[k.id] = s.liked[d.id]; delete s.liked[d.id]; }
    if (s.plays[d.id]) { const a = s.plays[k.id] || [0, 0], b = s.plays[d.id]; s.plays[k.id] = [a[0] + b[0], Math.max(a[1] || 0, b[1] || 0)]; delete s.plays[d.id]; }
    for (const p of s.playlists) {
      if (!p.items.some(x => x.id === d.id)) continue;
      const seen = new Set();
      p.items = p.items.map(x => x.id === d.id ? { ...x, id: k.id } : x).filter(x => !seen.has(x.id) && seen.add(x.id));
      p.updated = Date.now();
    }
    if (s.loops && s.loops[d.id]) { if (!s.loops[k.id]) s.loops[k.id] = s.loops[d.id]; delete s.loops[d.id]; }
    const q = s.queue;
    q.items.forEach((x, i) => { if (x.id === d.id && i !== q.at) x.id = k.id; });
    if (Array.isArray(q.orig)) q.orig = q.orig.map(id => id === d.id ? k.id : id);
    N.stats.merge(d.id, k.id);
    N.emit('merged', { from: d, to: k });   // for add-ons that keep records of their own about songs
  }
  async function fixDupes(mode) {
    const plan = [];
    DUP.groups.forEach((g, gi) => { const k = L.get(DUP.keep[gi]); if (k) for (const t of g) if (t.id !== k.id && DUP.drop.has(t.id)) plan.push([k, t]); });
    if (!plan.length) return;
    const mb = (plan.reduce((n, [, d]) => n + (d.size || 0), 0) / 1048576).toFixed(1);
    if (mode === 'recycle' && !await ask({ title: `Move ${copies(plan.length)} to the Recycle Bin?`, text: `${mb} MB of song files. You can put them back from the Recycle Bin; the copies you keep take over their likes, plays and playlist places.`, ok: 'Move to the Recycle Bin', danger: true })) return;
    // a copy that's playing right now hands over to the kept one, at the same moment of the song
    const res = mode === 'recycle' ? await belt.native.recycle(plan.map(([, d]) => d.path)) : null;
    const moved = res && new Set(res.done.map(p => p.toLowerCase()));
    const completed = moved ? plan.filter(([, d]) => moved.has(d.id)) : plan;
    const cur = P.current(), swap = moved && cur && completed.find(([, d]) => d.id === cur.id);
    for (const [k, d] of completed) mergeCopy(k, d);
    if (moved) N.emit('removed', [...moved]);   // song files that went to the Recycle Bin (their ids)
    if (swap) {
      const q = S().queue, pos = P.time(), was = P.playing;
      q.items[q.at].id = swap[0].id;
      P.jump(q.at);
      if (!was) P.pause();
      P.seek(pos);
    }
    if (mode === 'hide') {
      S().hidden = S().hidden || {};
      for (const [, d] of plan) S().hidden[d.id] = Date.now();
      N.save(); L.refresh();
      belt.toast(`${copies(plan.length)} hidden from String: the files are still there`);
    } else {
      for (const p of res.done) delete (S().hidden || {})[p.toLowerCase()];
      N.save();
      await L.scan().catch(() => { });
      belt.toast(res.failed.length ? `${copies(res.done.length)} moved to the Recycle Bin; ${res.failed.length} couldn't be: ${res.failed[0].error}` : `${copies(res.done.length)} moved to the Recycle Bin`);
    }
    findDupes();
    if (route() === 'dupes' || route() === 'settings') render({ keep: true });
  }
  document.addEventListener('click', e => {
    const show = e.target.closest('[data-dup-show]');
    if (show) { const t = L.get(show.dataset.dupShow); if (t) belt.native.reveal(t.path).catch(err => belt.toast(err.message)); return; }
    if (e.target.closest('#dpForget')) { S().notDupes = {}; N.save(); DUP.groups = null; render({ keep: true }); return; }
    if (e.target.closest('#dupUnhide')) { S().hidden = {}; N.save(); L.refresh(); DUP.groups = null; render({ keep: true }); belt.toast('Hidden songs are back'); }
  });

  function vSettings(v) {
    const s = S().set;
    const folders = S().roots.map(p => {
      const n = [...L.tracks.values()].filter(t => t.id.startsWith(p.toLowerCase().replace(/[\\/]+$/, '') + '\\')).length;
      return `<div class="folder">${belt.icon('folder')}<span class="p">${esc(p)}</span><small>${plural(n, 'song')}</small><button class="icon-btn" data-act="revealPath" data-path="${attr(p)}" title="Show in Explorer">${belt.icon('external')}</button><button class="icon-btn" data-act="removeRoot" data-path="${attr(p)}" title="Stop using this folder">${belt.icon('x')}</button></div>`;
    }).join('');
    const ids = L.ids(L.all), total = ids.reduce((x, id) => x + (L.get(id).duration || 0), 0);
    const plays = Object.values(S().plays).reduce((x, p) => x + p[0], 0);
    const fmts = {};
    for (const id of ids) { const t = L.get(id); fmts[t.format || '?'] = (fmts[t.format || '?'] || 0) + 1; }
    v.innerHTML = pageTitle('Library', 'Folders, playback and look') + `<div class="set-grid">` +
      `<div class="panel"><div class="panel-h"><span class="lbl">Music folders</span><span class="note">read in place, never changed</span></div>` +
      `<div class="folders">${folders || '<div class="empty-note">No folders yet</div>'}</div>` +
      `<div class="row"><button class="btn primary sm" data-act="addFolder">${belt.icon('plus')}Add folder</button><button class="btn sm" data-act="rescan">${belt.icon('reload')}Rescan</button></div>` +
      `<div class="panel-h" style="margin-top:22px"><span class="lbl">Single songs</span><span class="note">${plural(S().files.length, 'song')} added one by one</span></div>` +
      `<div class="row"><button class="btn sm" data-act="addFiles">${belt.icon('music')}Add songs</button>${S().files.length ? '<button class="btn sm ghost" data-act="clearFiles">Forget them</button>' : ''}</div>` +
      ADD.library.map(fn => { try { return fn() || ''; } catch (e) { console.error(e); return ''; } }).join('') +
      `<div class="panel-h" style="margin-top:22px"><span class="lbl">Duplicates</span><span class="note">the same song twice</span></div>` +
      `<div id="dupBody"><div class="row"><button class="btn sm" data-go-to="dupes">${belt.icon('search')}Find duplicates</button></div>${hiddenNote()}</div></div>` +
      `<div class="panel"><div class="panel-h"><span class="lbl">Playback</span></div>` +
      `<div class="set-row"><span>Crossfade between songs</span><span class="row"><input type="range" id="fade" min="0" max="12" step="1" value="${s.fade}"><span class="v" id="fadeV">${s.fade ? s.fade + ' s' : 'Off'}</span></span></div>` +
      `<p class="scan-note" style="margin:0 10px 12px;text-transform:none;letter-spacing:0">Off plays songs back to back with no gap.</p>` +
      `<label class="switch"><span>Autoplay when a list ends</span><input type="checkbox" id="autoOn"${s.autoplay !== false ? ' checked' : ''}></label>` +
      `<p class="scan-note" style="margin:0 10px 12px;text-transform:none;letter-spacing:0">Skips to the next album or songs from your library instead of pausing.</p>` +
      `<label class="switch"><span>Find lyrics online (LRCLIB)</span><input type="checkbox" id="lyOn"${s.lyricsOnline ? ' checked' : ''}></label>` +
      `<div class="set-row"><span>Translate lyrics into</span><select class="field sel" id="trLang">${N.translate.LANGS.map(l => `<option${(s.trLang || 'English') === l ? ' selected' : ''}>${esc(l)}</option>`).join('')}</select></div>` +
      `<div class="set-row"><span>Translation model</span><select class="field sel" id="trModel"><option value="">${s.trModel ? 'Best downloaded' : 'Best downloaded (automatic)'}</option>${s.trModel ? `<option selected>${esc(s.trModel)}</option>` : ''}</select></div>` +
      `<p class="scan-note" style="margin:0 10px 12px;text-transform:none;letter-spacing:0">Translations run on this PC with Ollama: lyrics never leave your computer. Bigger models translate better and take longer.</p>` +
      `<p class="scan-note" style="margin:0 10px 12px;text-transform:none;letter-spacing:0">For songs without lyrics of their own: sends the title, artist, album and length to lrclib.net, a free open lyrics library.</p>` +
      `<div class="panel-h" style="margin-top:18px"><span class="lbl">Look</span></div>` +
      `<div class="set-row"><span>Tint</span><div class="seg" id="tintSeg">${[['mono', 'Mono'], ['arc', 'Arc'], ['ember', 'Ember']].map(([k, n]) => `<button data-k="${k}" class="${(s.tint || 'mono') === k ? 'on' : ''}">${n}</button>`).join('')}</div></div>` +
      `<div class="set-row"><span>Cover art</span><div class="seg" id="artSeg"><button data-k="mono" class="${s.artMono ? 'on' : ''}">Tinted</button><button data-k="colour" class="${s.artMono ? '' : 'on'}">Colour</button></div></div>` +
      `<label class="switch"><span>Spectrum in the player bar</span><input type="checkbox" id="vizOn"${s.viz ? ' checked' : ''}></label>` +
      `<div class="set-row"><span>Your own look</span><button class="btn sm" data-act="lookFolder">${belt.icon('folder')}Open folder</button></div>` +
      `<label class="switch"><span>Use my look.css</span><input type="checkbox" id="lookOn"${s.look !== false ? ' checked' : ''}></label>` +
      `<p class="scan-note" style="margin:0 10px 12px;text-transform:none;letter-spacing:0">A look.css in your look folder (with pictures or fonts beside it) loads after String's own look, so it can change colours, the logo or the background. Updates never touch it, and String shows each saved change straight away. The first time, the folder gets a look.css full of examples.</p></div>` +
      `<div class="panel" id="dcPanel"><div class="panel-h"><span class="lbl">Discord</span><span class="note">“Listening to” on your profile</span></div>` +
      `<label class="switch"><span>Show what I'm playing on Discord</span><input type="checkbox" id="dcOn"${s.discord ? ' checked' : ''}></label>` +
      `<label class="f dc-id"><span class="lbl">Application ID</span><input class="field mono" id="dcId" value="${attr(s.discordId || '')}" placeholder="a long number from Discord's developer site" spellcheck="false" autocomplete="off" inputmode="numeric"></label>` +
      `<div class="set-row"><span>Your status line shows</span><div class="seg" id="dcShow">${[['song', 'Song'], ['artist', 'Artist'], ['app', 'App name']].map(([k, n]) => `<button data-k="${k}" class="${(s.dcShow || 'song') === k ? 'on' : ''}">${n}</button>`).join('')}</div></div>` +
      `<label class="switch"><span>Covers from Apple's iTunes search</span><input type="checkbox" id="dcCovers"${s.dcCovers ? ' checked' : ''}></label>` +
      `<p class="dc-note">Discord only shows pictures that are on the web, so your own cover files can't appear there. With this on, the song and artist name go to Apple's iTunes search to find the cover; off, there's no picture.</p>` +
      `<div class="dc-status" id="dcStatus"></div>` +
      `<details class="dc-how"${s.discordId ? '' : ' open'}><summary>Getting an application ID (once)</summary><ol>` +
      `<li>Open <a class="link" data-ext="https://discord.com/developers/applications">Discord's developer site</a> and sign in with your Discord account.</li>` +
      `<li>Press <b>New Application</b>. Its name is what shows after “Listening to” on your profile, so call it <b>String</b> (or anything you like). You can give it an icon there too.</li>` +
      `<li>On its <b>General Information</b> page, copy the <b>Application ID</b>, paste it above and turn the switch on.</li>` +
      `<li>In Discord, <b>Settings → Activity Privacy</b>: sharing your activity has to be on.</li></ol>` +
      `<p class="dc-note">Discord has to be open on this PC (the app, not the website). Your status clears when you pause or close String.</p></details></div>` +
      `<div class="panel" id="updPanel"><div class="panel-h"><span class="lbl">Updates</span><span class="note" id="updVer"></span></div><div id="updBody"></div></div>` +
      `<div class="panel"><div class="panel-h"><span class="lbl">Numbers</span></div><div class="stats">` +
      `<div class="kpi"><span class="lbl">Songs</span><div class="v">${ids.length.toLocaleString()}</div><div class="s">${long(total)}</div></div>` +
      `<div class="kpi"><span class="lbl">Albums</span><div class="v">${L.albums.size.toLocaleString()}</div><div class="s">${plural([...L.artists.values()].filter(a => a.key).length, 'artist')}</div></div>` +
      `<div class="kpi"><span class="lbl">Plays</span><div class="v">${plays.toLocaleString()}</div><div class="s">${plural(Object.keys(S().liked).length, 'liked song')}</div></div>` +
      `<div class="kpi"><span class="lbl">Formats</span><div class="v" style="font-size:15px">${Object.entries(fmts).sort((a, b) => b[1] - a[1]).map(([k, n]) => `${esc(k.toUpperCase())} ${n}`).join(' · ') || '–'}</div><div class="s">${L.lastScan ? 'checked ' + new Date(L.lastScan).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' }) : ''}</div></div>` +
      `</div><p class="scan-note" style="margin:16px 0 0;text-transform:none;letter-spacing:0;line-height:1.6">Keys: Space play/pause · Ctrl+←/→ previous/next · ←/→ seek · Ctrl+↑/↓ volume · M mute · S shuffle · R repeat · L like · Q queue · F now playing · Y lyrics · [ ] loop start/end · \\ stop the loop · / search</p></div>` +
      `</div>`;
    drawUpdates();
    if (!UPD.info) belt.native.updateInfo().then(i => { UPD.info = UPD.info || i; drawUpdates(); }).catch(() => { });
    $('#fade', v).addEventListener('input', e => { s.fade = +e.target.value; $('#fadeV', v).textContent = s.fade ? s.fade + ' s' : 'Off'; N.save(); });
    $('#autoOn', v).addEventListener('change', e => P.setAutoplay(e.target.checked));
    $('#artSeg', v).addEventListener('click', e => { const b = e.target.closest('[data-k]'); if (!b) return; s.artMono = b.dataset.k === 'mono'; applyLook(); N.save(); $$('#artSeg button', v).forEach(x => x.classList.toggle('on', x === b)); });
    $('#vizOn', v).addEventListener('change', e => { s.viz = e.target.checked; applyLook(); N.save(); });
    $('#lookOn', v).addEventListener('change', e => { s.look = e.target.checked; applyLook(); N.save(); });
    $('#lyOn', v).addEventListener('change', e => { s.lyricsOnline = e.target.checked; N.save(); N.lyrics.forget(P.current()); LY.id = null; });
    $('#trLang', v).addEventListener('change', e => { s.trLang = e.target.value; N.save(); LY.id = null; });
    // the model list loads when the list is opened, so a visit to the settings never starts Ollama
    const trModel = $('#trModel', v);
    let modelsAsked = false;
    const fillModels = async () => {
      if (modelsAsked) return;
      modelsAsked = true;
      try {
        const list = await N.translate.models();
        trModel.innerHTML = `<option value="">Best downloaded (${esc(N.translate.pickModel(list) || 'none yet')})</option>` + list.map(m => `<option${s.trModel === m ? ' selected' : ''}>${esc(m)}</option>`).join('');
      } catch (err) { trModel.innerHTML = `<option value="">${esc('Ollama: ' + err.message)}</option>`; }
    };
    trModel.addEventListener('pointerdown', fillModels);
    trModel.addEventListener('focus', fillModels);
    trModel.addEventListener('change', e => { s.trModel = e.target.value; N.save(); LY.id = null; });
    $('#tintSeg', v).addEventListener('click', e => { const b = e.target.closest('[data-k]'); if (!b) return; s.tint = b.dataset.k; applyLook(); N.save(); $$('#tintSeg button', v).forEach(x => x.classList.toggle('on', x === b)); });
    $('#dcOn', v).addEventListener('change', e => { s.discord = e.target.checked; N.save(); discordSync(); });
    let idTimer = 0;
    $('#dcId', v).addEventListener('input', e => { clearTimeout(idTimer); idTimer = setTimeout(() => { s.discordId = e.target.value.replace(/\s+/g, ''); N.save(); discordSync(); }, 500); });
    $('#dcShow', v).addEventListener('click', e => { const b = e.target.closest('[data-k]'); if (!b) return; s.dcShow = b.dataset.k; N.save(); P.presence(true); $$('#dcShow button', v).forEach(x => x.classList.toggle('on', x === b)); });
    $('#dcCovers', v).addEventListener('change', e => { s.dcCovers = e.target.checked; N.save(); P.presence(true); });
    discordUI();
  }

  /* ================================================================== Discord: the helper behind "Listening to"
     presence\string-presence.exe (source presence\ in the project) follows presence.json and talks to Discord's pipe.
     String starts it while the setting is on and stops it when it closes; Discord then clears the status. */
  const D = { proc: null, cur: null, id: '', status: 'off', user: '', error: '', last: '', retry: 0 };
  const validId = id => /^\d{15,22}$/.test(id || '');
  async function discordSync() {
    const s = S().set;
    const want = belt.inBelt && !!s.discord && validId(s.discordId);
    if (D.cur && (!want || D.id !== s.discordId)) { const p = D.proc; D.proc = null; D.cur = null; if (p) p.kill(); }
    clearTimeout(D.retry);
    if (!want) {
      D.status = !s.discord ? 'off' : !s.discordId ? 'noid' : 'badid';
      D.user = ''; D.last = '';
      discordUI();
      return;
    }
    if (D.cur) return;
    const me = {};
    D.cur = me; D.id = s.discordId; D.status = 'starting'; D.error = ''; D.last = '';
    discordUI();
    try {
      const data = (await belt.native.paths()).data;
      const args = [s.discordId, data + '\\presence.json'];
      if (s.discordPipe) args.push('--pipe', s.discordPipe);   // the self-test's stand-in for Discord
      const proc = await belt.native.run('presence\\string-presence.exe', args, {
        onLine: line => {
          if (D.cur !== me) return;
          const sp = line.indexOf(' '), k = sp < 0 ? line : line.slice(0, sp), rest = sp < 0 ? '' : line.slice(sp + 1);
          if (k === 'status' && rest === 'waiting') { D.status = 'waiting'; D.user = ''; }
          else if (k === 'status' && rest.startsWith('connected')) { D.status = 'connected'; D.user = rest.slice(10); D.error = ''; }
          else if (k === 'sent') D.last = rest;
          else if (k === 'error' || k === 'warn') D.error = rest;
          discordUI();
        },
        onExit: code => {
          if (D.cur !== me) return;   // stopped on purpose
          D.cur = null; D.proc = null;
          D.status = code === 3 ? 'refused' : code === 2 ? 'badid' : 'stopped';
          discordUI();
          if (code !== 2 && code !== 3) D.retry = setTimeout(discordSync, 15000);
        },
      });
      if (D.cur === me) D.proc = proc; else proc.kill();
    } catch (e) {
      if (D.cur === me) { D.cur = null; D.status = 'error'; D.error = e.message; discordUI(); }
    }
    P.presence(true);
  }
  function discordUI() {
    const el = $('#dcStatus');
    if (!el) return;
    const playing = D.last.startsWith('playing ') ? D.last.slice(8) : '';
    const text = {
      off: 'Off',
      noid: 'Paste your application ID to start',
      badid: 'That doesn\'t look like an application ID: it\'s a long number',
      starting: 'Starting…',
      waiting: 'Waiting for Discord to open on this PC',
      connected: `Connected as ${D.user}` + (playing ? ` · showing “${playing}”` : ' · nothing showing while paused'),
      refused: 'Discord doesn\'t know that application ID: check it on the developer site',
      stopped: 'The helper stopped: trying again shortly',
      error: 'Couldn\'t start the helper: ' + D.error,
    }[D.status] || D.status;
    el.innerHTML = `<i class="dot ${D.status === 'connected' ? 'on' : D.status === 'waiting' || D.status === 'starting' ? 'wait' : ''}"></i><span>${esc(text)}</span>` +
      (D.error && D.status !== 'error' && D.status !== 'refused' ? `<small>${esc(D.error)}</small>` : '');
  }
  function applyLook() {
    const root = document.documentElement, tint = S().set.tint || 'mono';
    root.classList.toggle('tint-arc', tint === 'arc');
    root.classList.toggle('tint-ember', tint === 'ember');
    root.classList.toggle('art-mono', S().set.artMono !== false);
    $('#mini').hidden = !S().set.viz;
    N.viz.show('mini', !!S().set.viz);
    belt.setLook(S().set.look !== false);
  }

  /* ---------- your own look: <data>\look\look.css, loaded after String's styles (host.js adds it), never touched by
     updates. The host watches the folder; a saved change loads it again here. */
  function reloadLook() { belt.reloadLook(); }
  async function openLook() {
    try {
      const r = await belt.native.lookFolder();
      if (r.made) belt.toast('Your look folder is ready: look.css has examples to start from');
      reloadLook();
    } catch (err) { belt.toast(err.message); }
  }
  belt.onLook(reloadLook);

  /* ================================================================== updates
     An installed String asks GitHub (where it's published) for its newest release once a day, unless that's turned
     off in Settings → Updates, and offers it. Installing downloads it, checks it was signed with String's release key,
     and runs the release's installer (src\Updater.cs), which closes String and opens the new one; the library,
     playlists, settings and the person's own look stay. A copy built from source doesn't update itself. */
  const UPD = { info: null, checking: false, busy: false, msg: '' };
  async function checkUpdates(quiet) {
    if (UPD.checking || UPD.busy) return;
    UPD.checking = true; UPD.msg = ''; drawUpdates();
    try {
      UPD.info = await belt.native.updateCheck();
      const u = S().update = { ...(S().update || {}), at: Date.now() };
      if (UPD.info.newer && quiet && u.told !== UPD.info.latest) { u.told = UPD.info.latest; belt.toast(`String ${UPD.info.latest} is out: Settings → Updates`); }
      if (!quiet && UPD.info.installed && UPD.info.published && !UPD.info.newer) belt.toast('String is up to date');
      N.save();
    } catch (e) { UPD.msg = 'Couldn’t check: ' + e.message; if (!quiet) belt.toast(UPD.msg); }
    UPD.checking = false; drawUpdates();
  }
  async function installUpdate() {
    const i = UPD.info;
    if (!i || !i.newer || UPD.busy) return;
    if (!await ask({ title: `Install String ${i.latest}?`, text: 'String downloads it, checks it, then closes and opens again, updated. Your library, playlists, settings and your own look stay as they are.', ok: 'Install' })) return;
    UPD.busy = true; UPD.msg = 'Downloading…'; drawUpdates();
    try {
      UPD.done = await belt.native.updateInstall({ version: i.latest });
      UPD.msg = UPD.done.started ? 'Installing: String closes and opens again in a moment' : 'Downloaded and checked';
      // still here a while later: the installer gave up (String stayed open, say to keep unsaved changes); try again
      if (UPD.done.started) setTimeout(() => {
        if (!UPD.busy) return;
        UPD.busy = false;
        UPD.msg = 'The update didn’t finish. If its window said String was still open, click Install again.';
        drawUpdates();
      }, 90e3);
    } catch (e) { UPD.busy = false; UPD.msg = 'Not installed: ' + e.message; belt.toast(UPD.msg); }
    drawUpdates();
  }
  belt.onUpdate(m => {
    if (m.ev === 'progress') UPD.msg = `Downloading ${Math.min(100, Math.round(m.got / Math.max(1, m.total) * 100))}%`;
    else if (m.ev === 'verifying') UPD.msg = 'Checking the download…';
    const b = $('#updGo');
    if (b) b.lastChild.textContent = UPD.msg; else drawUpdates();
  });
  function drawUpdates() {
    const box = $('#updBody'), i = UPD.info;
    if (!box) return;
    $('#updVer').textContent = i ? 'String ' + i.current : '';
    const note = t => `<p class="scan-note" style="margin:0 10px 12px;text-transform:none;letter-spacing:0">${t}</p>`;
    let html = '';
    if (i && !i.installed) html = note('This copy of String was built from its source code, so it doesn’t update itself: build it again for a newer one.');
    else if (i && !i.published) html = note('This copy of String has nowhere to get updates from.');
    else if (i) {
      html += `<label class="switch"><span>Check for updates once a day</span><input type="checkbox" id="updAuto"${S().set.updates !== false ? ' checked' : ''}></label>`;
      if (i.newer) html += `<div class="upd-new"><b>String ${esc(i.latest)} is out</b>${i.notes ? `<p class="upd-notes">${esc(i.notes)}</p>` : ''}` +
        `<div class="row"><button class="btn primary sm" id="updGo"${UPD.busy ? ' disabled' : ''}>${belt.icon('download')}<span>${UPD.busy ? esc(UPD.msg) : 'Install ' + esc(i.latest)}</span></button></div></div>`;
      html += `<div class="row"><button class="btn sm" id="updCheck"${UPD.checking || UPD.busy ? ' disabled' : ''}>${belt.icon('reload')}${UPD.checking ? 'Checking…' : 'Check now'}</button>${i.latest && !i.newer ? '<span class="scan-note">Up to date</span>' : ''}</div>`;
      html += note('Checking asks GitHub, where String is published, for its newest release: nothing about you or your music goes with it. Updates are signed, so String only installs ones from its publisher.');
    }
    if (UPD.msg && !UPD.busy) html += note(esc(UPD.msg));
    box.innerHTML = html;
    const auto = $('#updAuto', box), go = $('#updGo', box), chk = $('#updCheck', box);
    if (auto) auto.addEventListener('change', e => { S().set.updates = e.target.checked; N.save(); });
    if (go) go.addEventListener('click', installUpdate);
    if (chk) chk.addEventListener('click', () => checkUpdates(false));
  }
  // the daily check: a minute after start, then every few hours if String stays open (not in a self-test)
  function maybeCheckUpdates() {
    if (S().set.updates === false || Date.now() - ((S().update || {}).at || 0) < 864e5) return;
    belt.native.updateInfo().then(i => { if (i.installed && i.published) checkUpdates(true); }).catch(() => { });
  }
  N.updates = { check: checkUpdates, install: installUpdate, state: UPD };

  /* ================================================================== actions (buttons with data-act anywhere) */
  async function addFolders() {
    const picked = await belt.native.pickFolder({ title: 'Choose folders with your songs', multi: true }).catch(e => { belt.toast(e.message); return null; });
    const list = Array.isArray(picked) ? picked : picked ? [picked] : [];
    addRoots(list);
  }
  function addRoots(list) {
    const S0 = S(), have = new Set(S0.roots.map(p => p.toLowerCase()));
    let added = 0;
    for (const p of list) {
      const k = p.toLowerCase();
      if (have.has(k) || S0.roots.some(r => k.startsWith(r.toLowerCase().replace(/[\\/]+$/, '') + '\\'))) continue;
      S0.roots = S0.roots.filter(r => !r.toLowerCase().startsWith(k.replace(/[\\/]+$/, '') + '\\'));   // a parent replaces its subfolders
      S0.roots.push(p); have.add(k); added++;
    }
    if (!added) { if (list.length) belt.toast('Already in your library'); return; }
    N.save();
    belt.toast(added === 1 ? 'Folder added: reading its songs' : `${added} folders added: reading their songs`);
    rescan();
    render({ keep: true });
  }
  async function addFiles() {
    const picked = await belt.native.pickFile({ title: 'Choose songs', multi: true, filter: 'Songs|*.mp3;*.flac;*.m4a;*.m4b;*.aac;*.ogg;*.oga;*.opus;*.wav;*.weba;*.webm|All files|*.*' }).catch(e => { belt.toast(e.message); return null; });
    const list = Array.isArray(picked) ? picked : picked ? [picked] : [];
    addSongs(list);
  }
  function addSongs(list) {
    const have = new Set(S().files.map(p => p.toLowerCase()));
    const fresh = list.filter(p => L.AUDIO.test(p) && !have.has(p.toLowerCase()) && !L.tracks.has(p.toLowerCase()));
    if (!fresh.length) { if (list.length) belt.toast('Those are already in your library'); return; }
    S().files.push(...fresh);
    N.save();
    belt.toast(`${plural(fresh.length, 'song')} added`);
    rescan();
  }
  function rescan() { L.scan().then(r => { if (r && route() === 'settings') render({ keep: true }); }).catch(e => belt.toast('Could not read the library: ' + e.message)); }

  document.addEventListener('click', e => {
    const ext = e.target.closest('[data-ext]');
    if (ext) { e.preventDefault(); belt.openExternal(ext.dataset.ext); return; }
    const g = e.target.closest('[data-go-to]');
    if (g) { go(g.dataset.goTo, { focusId: g.dataset.focusId }); return; }
    const b = e.target.closest('[data-act]');
    if (!b) return;
    const act = b.dataset.act, v = $('#view'), ctx = v.dataset.ctx ? JSON.parse(v.dataset.ctx) : null;
    if (act === 'addFolder') addFolders();
    else if (act === 'addFiles') addFiles();
    else if (act === 'rescan') { rescan(); belt.toast('Checking your folders'); }
    else if (act === 'removeRoot') { S().roots = S().roots.filter(p => p !== b.dataset.path); N.save(); rescan(); render({ keep: true }); }
    else if (act === 'revealPath') belt.native.reveal(b.dataset.path).catch(err => belt.toast(err.message));
    else if (act === 'lookFolder') openLook();
    else if (act === 'clearFiles') { S().files = []; N.save(); rescan(); render({ keep: true }); }
    else if (act === 'playCtx' && ctx) playCtx(ctx, null, visibleIds(ctx));
    else if (act === 'shuffleCtx' && ctx) { const ids = visibleIds(ctx) || ctxIds(ctx); if (ids.length) P.shuffleIds(ids, { ...ctx, name: ctxName(ctx) }); }
    else if (act === 'ctxMenu' && ctx) { const r = b.getBoundingClientRect(); ctxMenu(ctx, r.left, r.bottom + 4, visibleIds(ctx)); }
    else if (act === 'renamePl' && ctx) renamePl(ctx.key);
  });

  /* ================================================================== likes, playlists */
  function toggleLike(ids, btn) {
    const on = !ids.every(isLiked);
    for (const id of ids) { if (on) S().liked[id] = Date.now(); else delete S().liked[id]; }
    N.save();
    if (btn) { btn.classList.toggle('on', on); btn.classList.remove('pop'); void btn.offsetWidth; if (on) btn.classList.add('pop'); }
    if (ids.length > 1 || !btn) belt.toast(on ? `Added to Liked (${ids.length})` : 'Removed from Liked');
    refreshLikes();
  }
  function refreshLikes() {
    $('#cLiked').textContent = likedIds().length || '';
    const t = P.current();
    $('#nowLike').classList.toggle('on', !!t && isLiked(t.id));
    lists.forEach(l => { for (const [i, el] of l.rows) { const b = el.querySelector('[data-like]'); if (b) b.classList.toggle('on', isLiked(l.ids[i])); } });
    if (np.open) npInfo(false);
    if (route() === 'liked') render({ keep: true });
  }
  const newId = () => Date.now().toString(36) + Math.random().toString(36).slice(2, 6);
  async function newPlaylist(ids = []) {
    const name = await ask({ title: 'New playlist', value: `Playlist ${S().playlists.length + 1}`, ok: 'Create' });
    if (!name) return null;
    const p = { id: newId(), name, items: ids.map(id => ({ id, at: Date.now() })), created: Date.now(), updated: Date.now() };
    S().playlists.push(p);
    N.save();
    sidebar();
    if (ids.length) belt.toast(`${plural(ids.length, 'song')} added to ${name}`);
    return p;
  }
  function addToPlaylist(p, ids) {
    const have = new Set(plIds(p));
    const fresh = ids.filter(id => !have.has(id));
    if (!fresh.length) { belt.toast(`Already in ${p.name}`); return; }
    p.items.push(...fresh.map(id => ({ id, at: Date.now() })));
    p.updated = Date.now();
    N.save();
    belt.toast(`${fresh.length === 1 ? L.get(fresh[0]).T : plural(fresh.length, 'song')} added to ${p.name}`);
    sidebar();
    if (route() === 'pl/' + enc(p.id)) render({ keep: true });
  }
  async function renamePl(id) {
    const p = playlist(id); if (!p) return;
    const name = await ask({ title: 'Rename playlist', value: p.name, ok: 'Rename' });
    if (!name) return;
    p.name = name; p.updated = Date.now();
    N.save(); sidebar(); render({ keep: true });
  }
  async function deletePl(id) {
    const p = playlist(id); if (!p) return;
    const ok = await ask({ title: 'Delete playlist', text: `Delete “${p.name}”? The songs stay in your library.`, ok: 'Delete', danger: true });
    if (!ok) return;
    S().playlists = S().playlists.filter(x => x.id !== id);
    N.save(); sidebar();
    belt.toast(`Deleted ${p.name}`);
    if (route() === 'pl/' + enc(id)) go('home', { replace: true });
  }
  function removeFromPlaylist(id, idx) {
    const p = playlist(id); if (!p) return;
    const remove = new Set(idx);
    p.items = p.items.filter((_, i) => !remove.has(i));
    p.updated = Date.now();
    N.save(); sidebar(); render({ keep: true });
  }
  function removePlaylistEntries(id, entries) {
    const p = playlist(id); if (!p) return;
    const selected = new Set(entries), idx = [];
    p.items.forEach((entry, i) => { if (selected.has(entry)) idx.push(i); });
    if (idx.length) removeFromPlaylist(id, idx);
  }

  function sidebar() {
    const pls = S().playlists, from = P.context(), cur = P.current();
    $('#plList').innerHTML = pls.length ? pls.map(p => {
      const playing = cur && sameCtx(from, { kind: 'pl', key: p.id });
      return `<button data-pl="${attr(p.id)}" title="${attr(p.name)}">${mosaic(plIds(p), 'pl:' + p.id, p.name, 'sm')}<span><b class="ellip" style="font-weight:500">${esc(p.name)}</b><small>${plural(p.items.length, 'song')}</small></span>${playing ? '<span class="eq playing-dot"><i></i><i></i><i></i></span>' : ''}</button>`;
    }).join('') : '<p class="scan-note" style="padding:6px 12px;text-transform:none;letter-spacing:0;line-height:1.5">Make one with +, then drag songs onto it.</p>';
    const r = route();
    $$('#plList button').forEach(b => b.classList.toggle('on', r === 'pl/' + enc(b.dataset.pl)));
    ART.fill($('#plList'));
    const ids = L.ids(L.all);
    $('#cSongs').textContent = ids.length || '';
    $('#cAlbums').textContent = L.albums.size || '';
    $('#cArtists').textContent = [...L.artists.values()].filter(a => a.key).length || '';
    $('#cLiked').textContent = likedIds().length || '';
    const total = ids.reduce((s, id) => s + (L.get(id).duration || 0), 0);
    $('#sideFoot').textContent = ids.length ? `${plural(ids.length, 'song')} · ${long(total)}` : '';
  }
  $('#plList').addEventListener('click', e => { const b = e.target.closest('[data-pl]'); if (b) go('pl/' + enc(b.dataset.pl)); });
  $('#plList').addEventListener('contextmenu', e => { const b = e.target.closest('[data-pl]'); if (!b) return; e.preventDefault(); ctxMenu({ kind: 'pl', key: b.dataset.pl }, e.clientX, e.clientY); });
  // songs dragged onto a playlist in the sidebar
  $('#plList').addEventListener('dragover', e => {
    const b = e.target.closest('[data-pl]');
    if (!b || !dragging) return;
    e.preventDefault(); e.dataTransfer.dropEffect = 'copy';
    $$('#plList .drop').forEach(x => x !== b && x.classList.remove('drop'));
    b.classList.add('drop');
  });
  $('#plList').addEventListener('dragleave', e => { const b = e.target.closest('[data-pl]'); if (b && !b.contains(e.relatedTarget)) b.classList.remove('drop'); });
  $('#plList').addEventListener('drop', e => {
    const b = e.target.closest('[data-pl]');
    $$('#plList .drop').forEach(x => x.classList.remove('drop'));
    if (!b || !dragging) return;
    e.preventDefault();
    const p = playlist(b.dataset.pl);
    if (p) addToPlaylist(p, dragging.ids);
  });
  $('#newPl').addEventListener('click', async () => { const p = await newPlaylist(); if (p) go('pl/' + enc(p.id)); });

  /* ================================================================== menus and dialogs */
  let menuEl = null;
  function closeMenu() { if (menuEl) { menuEl.remove(); menuEl = null; } }
  function menu(items, x, y) {
    closeMenu();
    const el = document.createElement('div');
    el.id = 'menu';
    const draw = list => {
      el.innerHTML = list.filter(Boolean).map((it, i) => it.sep ? '<hr>' : it.head ? `<div class="mh ellip">${esc(it.head)}</div>` : `<button data-i="${i}">${it.icon ? icon(it.icon) : ''}<span class="ellip">${esc(it.label)}</span>${it.k ? `<span class="k">${esc(it.k)}</span>` : it.sub ? '<span class="k">›</span>' : ''}</button>`).join('');
      el.onclick = e => {
        const b = e.target.closest('button'); if (!b) return;
        const it = list.filter(Boolean)[+b.dataset.i];
        if (it.sub) { draw([{ label: 'Back', icon: 'back', sub: null, act: () => draw(items) }, { sep: 1 }, ...it.sub()]); return; }
        closeMenu();
        if (it.act) it.act();
      };
    };
    draw(items);
    document.body.appendChild(el);
    const r = el.getBoundingClientRect(), W = innerWidth, Hh = innerHeight;
    const left = Math.max(6, Math.min(x, W - r.width - 6)), top = Math.max(6, Math.min(y, Hh - r.height - 6));
    el.style.left = left + 'px'; el.style.top = top + 'px';
    el.style.setProperty('--ox', (x - left) + 'px'); el.style.setProperty('--oy', (y - top) + 'px');
    menuEl = el;
  }
  addEventListener('pointerdown', e => { if (menuEl && !menuEl.contains(e.target)) closeMenu(); }, true);
  addEventListener('blur', closeMenu);

  function playlistPicker(ids) {
    return [{ label: 'New playlist…', icon: 'plus', act: () => newPlaylist(ids) },
      ...(S().playlists.length ? [{ sep: 1 }] : []),
      ...S().playlists.slice().sort((a, b) => b.updated - a.updated).map(p => ({ label: p.name, icon: 'list', act: () => addToPlaylist(p, ids) }))];
  }
  function trackMenu(ids, x, y, list, idx, ctx) {
    if (!ids.length) return;
    const t = L.get(ids[0]), one = ids.length === 1, liked = ids.every(isLiked);
    const plRemove = list && list.o.pl ? list.playlistSelection() : null;
    const playAt = list && !one ? list.ids.findIndex((id, i) => id === ids[0] && list.sel.has(i)) : idx;
    menu([
      { head: one ? t.T : plural(ids.length, 'song') },
      { label: 'Play', icon: 'playf', act: () => list && playAt >= 0 ? list.play(playAt) : P.playIds(ids, 0, ctx ? { ...ctx, name: ctxName(ctx) } : null) },
      { label: 'Play next', icon: 'playnext', act: () => { P.playNext(ids); belt.toast(one ? `${t.T} plays next` : `${ids.length} songs play next`); } },
      { label: 'Add to queue', icon: 'queue', act: () => queueTracks(ids) },
      { sep: 1 },
      { label: 'Add to playlist', icon: 'plus', sub: () => playlistPicker(ids) },
      { label: liked ? 'Remove from Liked' : 'Add to Liked', icon: 'heart', k: 'L', act: () => toggleLike(ids) },
      plRemove ? { label: 'Remove from this playlist', icon: 'trash', act: () => removePlaylistEntries(list.o.pl, plRemove) } : null,
      { sep: 1 },
      one && t.albumKey ? { label: 'Go to album', icon: 'disc', act: () => go('album/' + enc(t.albumKey)) } : null,
      one && t.A ? { label: 'Go to artist', icon: 'user', act: () => go('artist/' + enc(t.artistKey)) } : null,
      one ? { label: 'Edit info', icon: 'pencil', act: () => editInfo(t) } : null,
      one && t.A ? { label: 'Swap title and artist', icon: 'swap', act: () => swapNames(t) } : null,
      one ? { label: 'Show in Explorer', icon: 'folder', act: () => belt.native.reveal(t.path).catch(e => belt.toast(e.message)) } : null,
    ], x, y);
  }
  function ctxMenu(ctx, x, y, visible) {
    const ids = visible || ctxIds(ctx), p = ctx.kind === 'pl' ? playlist(ctx.key) : null;
    menu([
      { head: ctxName(ctx) },
      { label: 'Play', icon: 'playf', act: () => playCtx(ctx, null, visible) },
      { label: 'Shuffle', icon: 'shuffle', act: () => ids.length && P.shuffleIds(ids, { ...ctx, name: ctxName(ctx) }) },
      { label: 'Play next', icon: 'playnext', act: () => { P.playNext(ids); belt.toast(`${plural(ids.length, 'song')} play next`); } },
      { label: 'Add to queue', icon: 'queue', act: () => queueTracks(ids) },
      { sep: 1 },
      { label: 'Add to playlist', icon: 'plus', sub: () => playlistPicker(ids) },
      ...(p ? [{ sep: 1 }, { label: 'Rename', icon: 'pencil', act: () => renamePl(p.id) }, { label: 'Delete playlist', icon: 'trash', act: () => deletePl(p.id) }] : []),
      ...(ctx.kind === 'album' && ids[0] ? [{ sep: 1 }, { label: 'Show in Explorer', icon: 'folder', act: () => belt.native.reveal(L.get(ids[0]).dir).catch(e => belt.toast(e.message)) }] : []),
    ], x, y);
  }

  // a small dialog: a text box (resolves to the text) or a question (resolves true/false)
  function ask({ title, value, text, ok = 'Save', cancel = 'Cancel', danger = false }) {
    return new Promise(res => {
      const scrim = document.createElement('div');
      scrim.className = 'scrim';
      scrim.innerHTML = `<div class="dialog"><h2>${esc(title)}</h2>${text ? `<p style="margin:0;color:var(--text2)">${esc(text)}</p>` : `<input class="field" value="${attr(value || '')}" spellcheck="false" maxlength="120">`}` +
        `<div class="row end"><button class="btn ghost" data-k="no">${esc(cancel)}</button><button class="btn ${danger ? 'danger' : 'primary'}" data-k="ok">${esc(ok)}</button></div></div>`;
      document.body.appendChild(scrim);
      const input = $('input', scrim);
      const done = v => { scrim.remove(); res(v); };
      if (input) { input.focus(); input.select(); }
      scrim.addEventListener('click', e => {
        if (e.target === scrim) done(null);
        const k = e.target.closest('[data-k]');
        if (k) done(k.dataset.k === 'ok' ? (input ? input.value.trim() || null : true) : null);
      });
      scrim.addEventListener('keydown', e => {
        e.stopPropagation();
        if (e.key === 'Escape') done(null);
        if (e.key === 'Enter') done(input ? input.value.trim() || null : true);
      });
      if (!input) $('[data-k="ok"]', scrim).focus();
    });
  }
  // Edit info: String's own names for a song (the file isn't touched)
  // a song named the wrong way round ("Title - Artist" read as "Artist - Title"): one click, an edit of your own
  function swapNames(t) {
    const title = t.A, artist = t.T, e = { ...(S().edits[t.id] || {}), title, artist };
    delete e.auto;
    S().edits[t.id] = e;
    N.save(); L.refresh();
    belt.toast(`Now “${title}” by ${artist}`);
    N.emit('edited', t.id);
  }
  function editInfo(t) {
    const e = S().edits[t.id] || {};
    let art = e.art || '';
    const scrim = document.createElement('div');
    scrim.className = 'scrim';
    const f = (k, label, val, ph) => `<label class="f"><span class="lbl">${label}</span><input class="field" data-f="${k}" value="${attr(val)}" placeholder="${attr(ph)}" spellcheck="false"></label>`;
    scrim.innerHTML = `<div class="dialog"><h2>Edit info</h2>` +
      f('title', 'Title', e.title || '', t.title || L.nameTitle(t.name)) + f('artist', 'Artist', e.artist || '', t.artist || 'Unknown artist') + f('album', 'Album', e.album || '', t.album || 'No album') +
      `<div class="art-pick"><div class="art" id="eiArt"></div><div><button class="btn sm" data-k="pick">${belt.icon('image')}Choose a cover</button> <button class="btn sm ghost" data-k="noart">Use the song's own</button><div class="note" id="eiNote" style="margin-top:6px"></div></div></div>` +
      `<p class="note" style="margin:16px 0 0">${esc(ADD.editNotes.map(fn => { try { return fn(t); } catch { return ''; } }).find(Boolean) || 'These edits are saved in String.')} Leave a box empty to use the song's own tag.</p>` +
      `<div class="row end"><button class="btn ghost" data-k="reset">Reset all</button><span class="grow"></span><button class="btn ghost" data-k="no">Cancel</button><button class="btn primary" data-k="ok">Save</button></div></div>`;
    document.body.appendChild(scrim);
    const showArt = () => {
      const tmp = { ...t };
      const before = S().edits[t.id];
      S().edits[t.id] = { ...(before || {}), art };
      ART.set($('#eiArt', scrim), tmp, 'sm', t.albumKey || t.id, t.T);
      S().edits[t.id] = before; if (!before) delete S().edits[t.id];
      $('#eiNote', scrim).textContent = art ? L.base(art) : '';
    };
    showArt();
    $('input', scrim).focus();
    const close = () => scrim.remove();
    scrim.addEventListener('keydown', ev => { ev.stopPropagation(); if (ev.key === 'Escape') close(); if (ev.key === 'Enter') save(); });
    function save() {
      const out = {};
      $$('[data-f]', scrim).forEach(i => { const v = i.value.trim(); if (v) out[i.dataset.f] = v; });
      if (art) out.art = art;
      ART.forget(t);
      if (Object.keys(out).length) S().edits[t.id] = out; else delete S().edits[t.id];
      ART.forget(t);
      N.save();
      close();
      L.refresh();
      N.emit('track');
      belt.toast('Saved');
      N.emit('edited', t.id);
    }
    scrim.addEventListener('click', async ev => {
      if (ev.target === scrim) { close(); return; }
      const k = ev.target.closest('[data-k]'); if (!k) return;
      const key = k.dataset.k;
      if (key === 'no') close();
      else if (key === 'ok') save();
      else if (key === 'reset') { $$('[data-f]', scrim).forEach(i => { i.value = ''; }); art = ''; showArt(); }
      else if (key === 'noart') { art = ''; showArt(); }
      else if (key === 'pick') {
        const p = await belt.native.pickFile({ title: 'Choose a cover picture', filter: 'Pictures|*.jpg;*.jpeg;*.png;*.webp;*.gif;*.bmp', start: t.dir }).catch(err => { belt.toast(err.message); return null; });
        if (p) { art = Array.isArray(p) ? p[0] : p; showArt(); }
      }
    });
  }

  /* ================================================================== player bar */
  let lastBarId = null;
  function barTrack() {
    const t = P.current();
    ART.set($('#nowArt'), t, 'sm', t ? t.albumKey || t.id : 'string', t ? t.B || t.T : '');
    const title = $('#nowTitle'), sub = $('#nowSub');
    const changed = (t ? t.id : '') !== lastBarId;
    lastBarId = t ? t.id : '';
    title.innerHTML = `<span>${esc(t ? t.T : 'Nothing playing')}</span>`;
    sub.innerHTML = t ? (t.A ? `<a class="link" data-artist="${attr(t.artistKey)}">${esc(t.A)}</a>` : 'Unknown artist') + (t.B ? ` · <a class="link" data-album="${attr(t.albumKey)}">${esc(t.B)}</a>` : '') : (L.tracks.size ? 'Pick a song, or press Space to shuffle everything' : '');
    if (changed && t && !calm()) { title.classList.remove('swap-in'); sub.classList.remove('swap-in'); void title.offsetWidth; title.classList.add('swap-in'); sub.classList.add('swap-in'); }
    requestAnimationFrame(() => {
      const span = title.firstElementChild, over = span.scrollWidth - title.clientWidth;
      title.classList.toggle('long', over > 4);
      title.style.setProperty('--mx', -(over + 16) + 'px');
      title.style.setProperty('--md', Math.max(5, (over + 16) / 28) + 's');
    });
    $('#nowLike').classList.toggle('on', !!t && isLiked(t.id));
    $('#nowLike').disabled = !t;
    belt.setTitle(t && P.playing ? t.T : '');
    lists.forEach(l => l.paint(true));
    markNow();
    sidebar();
    if (np.open) npRender();
    if (qOpen) drawQueue();
    tickNow(true);
  }
  function barState() {
    const on = P.playing;
    belt.setPlaying(on);   // the taskbar's middle button: pause while playing, play while not
    document.body.classList.toggle('playing', on);
    $('#bar').classList.toggle('is-playing', on);
    $('#np').classList.toggle('is-playing', on);
    $('#pp').title = on ? 'Pause (Space)' : 'Play (Space)';
    const t = P.current();
    belt.setTitle(t && on ? t.T : '');
    for (const l of lists) { const i = l.ids.indexOf(curId()); const el = l.rows.get(i); if (el) { const g = el.querySelector('.go'); if (g) g.innerHTML = icon(on ? 'pausef' : 'playf'); } }
    markNow();
    N.viz.wake();
    tickNow(true);
  }
  function modes() {
    const q = S().queue;
    $('#shuffle').classList.toggle('on', q.shuffle);
    $('#repeat').classList.toggle('on', q.repeat !== 'off');
    $('#repeat').innerHTML = icon('repeat') + (q.repeat === 'one' ? '<span class="one">1</span>' : '');
    $('#repeat').title = q.repeat === 'off' ? 'Repeat (R)' : q.repeat === 'all' ? 'Repeating everything (R)' : 'Repeating this song (R)';
    $('#shuffle').title = q.shuffle ? 'Shuffle is on (S)' : 'Shuffle (S)';
  }
  function volUI() {
    const v = S().muted ? 0 : S().vol;
    $('#volFill').style.width = v * 100 + '%';
    $('#volKnob').style.left = v * 100 + '%';
    $('#mute').innerHTML = icon(v === 0 ? 'mute' : v < .5 ? 'vol1' : 'vol');
    $('#volTip').textContent = Math.round(v * 100) + '%';
  }
  // the seek bar and times: every frame while visible and playing, else on demand
  let seekDrag = null, tickRaf = 0;
  function tickNow(once) {
    if (!canDraw()) return;
    const dur = P.duration(), cur = seekDrag ? seekDrag.frac * dur : P.time(), f = dur ? Math.min(1, cur / dur) : 0;
    $('#seekFill').style.width = f * 100 + '%';
    $('#seekKnob').style.left = f * 100 + '%';
    $('#seekBuf').style.width = (dur ? Math.min(1, P.buffered() / dur) : 0) * 100 + '%';
    setTimeText($('#tCur'), mmss(cur));
    const left = dur ? Math.max(0, dur - cur) : 0;
    const timeLeft = !!S().set.timeLeft, tDur = $('#tDur'), pressed = String(timeLeft);
    setTimeText(tDur, dur ? (timeLeft ? '-' + mmss(left) : mmss(dur)) : '0:00');
    if (tDur.getAttribute('aria-pressed') !== pressed) tDur.setAttribute('aria-pressed', pressed);
    $('#bar').style.setProperty('--prog', (f * 100).toFixed(2) + '%');
    if (np.open) { const s = $('#npSeek'); if (s) setTimeText(s, `${mmss(cur)} / ${mmss(dur)}`); }
    if (!once) tickRaf = 0;
    if (!tickRaf && P.playing && belt.visible !== false) tickRaf = requestAnimationFrame(() => { tickRaf = 0; tickNow(false); });
  }
  function setTimeText(el, text) { if (el.textContent !== text) el.textContent = text; }
  function canDraw() { return belt.visible !== false && !document.hidden; }
  function slider(el, { onMove, onDone, tip }) {
    const frac = e => { const r = el.getBoundingClientRect(); return Math.max(0, Math.min(1, (e.clientX - r.left) / r.width)); };
    el.addEventListener('pointerdown', e => {
      if (e.button !== 0) return;
      el.setPointerCapture(e.pointerId);
      el.classList.add('drag');
      onMove(frac(e), true);
      const move = ev => onMove(frac(ev), true);
      const up = ev => { el.releasePointerCapture(e.pointerId); el.classList.remove('drag'); el.removeEventListener('pointermove', move); el.removeEventListener('pointerup', up); el.removeEventListener('pointercancel', up); onDone(frac(ev)); };
      el.addEventListener('pointermove', move);
      el.addEventListener('pointerup', up);
      el.addEventListener('pointercancel', up);
    });
    el.addEventListener('pointermove', e => { const f = frac(e), t = el.querySelector('.tip'); if (t) { t.style.left = f * 100 + '%'; if (tip) t.textContent = tip(f); } });
  }
  slider($('#seek'), {
    onMove: f => { if (!P.current()) return; seekDrag = { frac: f }; tickNow(true); },
    onDone: f => { if (!P.current()) return; seekDrag = null; P.seek(f * P.duration()); tickNow(true); },
    tip: f => mmss(f * P.duration()),
  });
  slider($('#vol'), { onMove: f => { S().vol = f; S().muted = false; P.volumeChanged(); volUI(); }, onDone: f => P.setVolume(f), tip: f => Math.round(f * 100) + '%' });
  $('#vol').addEventListener('wheel', e => { e.preventDefault(); P.setVolume((S().muted ? 0 : S().vol) - Math.sign(e.deltaY) * .05); }, { passive: false });
  $('#tDur').addEventListener('click', () => { S().set.timeLeft = !S().set.timeLeft; N.save(); tickNow(true); });
  $('#pp').addEventListener('click', () => P.toggle());

  /* ---------- the loop: right-click the seek bar to start or end it there ---------- */
  function loopUI() {
    const lp = P.loop(), dur = P.duration(), band = $('#loopBand'), chip = $('#loopChip');
    if (!lp || !dur) { band.hidden = true; chip.hidden = true; return; }
    const a = lp.a || 0, b = lp.b != null ? lp.b : dur;
    band.hidden = false;
    band.style.left = (a / dur * 100).toFixed(3) + '%';
    band.style.width = (Math.max(0, b - a) / dur * 100).toFixed(3) + '%';
    band.classList.toggle('from-start', lp.a == null);
    band.classList.toggle('to-end', lp.b == null);
    chip.hidden = false;
    chip.innerHTML = icon('repeat') + `<span>${lp.a == null ? 'start' : mmss(a)}–${lp.b == null ? 'end' : mmss(b)}</span>`;
  }
  function loopAt(which, at) {
    if (!P.current() || !P.duration()) return;
    const lp = P.loop() || {};
    const nl = which === 'a' ? P.setLoop(at, lp.b) : P.setLoop(lp.a, at);
    if (!nl) return;
    belt.toast(which === 'a' && nl.b == null ? `Loop starts at ${mmss(nl.a || 0)}: right-click where it should end` : `Looping ${mmss(nl.a || 0)}–${nl.b == null ? 'the end' : mmss(nl.b)}`);
  }
  $('#seek').addEventListener('contextmenu', e => {
    e.preventDefault();
    const dur = P.duration();
    if (!P.current() || !dur) return;
    const r = $('#seek').getBoundingClientRect(), at = Math.max(0, Math.min(1, (e.clientX - r.left) / r.width)) * dur;
    const lp = P.loop();
    menu([
      { head: `Loop · at ${mmss(at)}` },
      { label: 'Start the loop here', icon: 'flag', k: '[', act: () => loopAt('a', at) },
      { label: 'End the loop here', icon: 'flag', k: ']', act: () => loopAt('b', at) },
      lp ? { sep: 1 } : null,
      lp ? { label: 'Stop looping', icon: 'x', k: '\\', act: () => { P.clearLoop(); belt.toast('Loop off'); } } : null,
    ], e.clientX, e.clientY - 12);
  });
  $('#loopChip').addEventListener('click', () => { P.clearLoop(); belt.toast('Loop off'); });
  N.on('loop', loopUI);
  N.on('track', loopUI);
  N.on('time', loopUI);
  $('#next').addEventListener('click', () => P.next());
  $('#prev').addEventListener('click', () => P.prev());
  $('#shuffle').addEventListener('click', () => P.setShuffle(!S().queue.shuffle));
  $('#repeat').addEventListener('click', () => P.cycleRepeat());
  $('#mute').addEventListener('click', () => P.toggleMute());
  $('#nowLike').addEventListener('click', e => { const t = P.current(); if (t) toggleLike([t.id], e.currentTarget); });
  $('#nowArt').addEventListener('click', () => P.current() && openNP());
  $('#nowArt').addEventListener('contextmenu', e => { const t = P.current(); if (t) { e.preventDefault(); trackMenu([t.id], e.clientX, e.clientY); } });
  $('#npBtn').addEventListener('click', () => np.open ? closeNP() : openNP());
  $('#qBtn').addEventListener('click', () => qOpen ? closeQueue() : openQueue());
  $('#qBtn').addEventListener('dragover', e => { if (!dragging) return; e.preventDefault(); e.dataTransfer.dropEffect = 'copy'; $('#qBtn').classList.add('drop'); });
  $('#qBtn').addEventListener('dragleave', () => $('#qBtn').classList.remove('drop'));
  function queueTracks(ids) {
    const added = P.addToQueue(ids);
    if (!added.length) return;
    const name = added.length === 1 ? L.get(added[0])?.T : '';
    belt.toast(`${name || plural(added.length, 'song')} added to the queue`);
  }
  $('#qBtn').addEventListener('drop', e => {
    $('#qBtn').classList.remove('drop');
    if (!dragging) return;
    e.preventDefault();
    queueTracks(dragging.ids);
  });
  $('#nowT').addEventListener('click', e => {
    const ar = e.target.closest('[data-artist]'); if (ar) { go('artist/' + enc(ar.dataset.artist)); return; }
    const al = e.target.closest('[data-album]'); if (al) { go('album/' + enc(al.dataset.album)); return; }
    const t = P.current(); if (!t) return;
    const from = P.context(), r = ctxRoute(from);
    const home = t.albumKey && L.albums.get(t.albumKey)?.ids.includes(t.id) ? 'album/' + enc(t.albumKey) : 'songs';
    go(r && ctxIds(from).includes(t.id) ? r : home, { focusId: t.id });
  });
  $('#nowT').addEventListener('contextmenu', e => { const t = P.current(); if (t) { e.preventDefault(); trackMenu([t.id], e.clientX, e.clientY); } });

  /* ================================================================== queue drawer */
  let qOpen = false, qDrag = -1, qMode = 'queue';
  function openQueue() { qOpen = true; drawQueue(); $('#queue').classList.add('open'); $('#qBtn').classList.add('on'); }
  function closeQueue() { qOpen = false; $('#queue').classList.remove('open'); $('#qBtn').classList.remove('on'); }
  function qItem(t, i, cur) {
    return `<div class="qi${cur ? ' cur' : ''}" data-q="${i}" draggable="${cur ? 'false' : 'true'}" style="animation-delay:${Math.min(i, 12) * 22}ms">${ART.html(t, t.albumKey || t.id, t.B || t.T, 'sm')}` +
      `<div class="tt"><span class="t-title ellip">${esc(t.T)}</span><span class="t-sub ellip">${esc(t.A || 'Unknown artist')}</span></div>` +
      `${cur ? (P.playing ? '<span class="eq"><i></i><i></i><i></i><i></i></span>' : '') : `<button class="icon-btn" data-qx="${i}" title="Remove">${belt.icon('x')}</button>`}</div>`;
  }
  function recentIds() {
    const out = [], seen = new Set();
    const add = id => { if (id && !seen.has(id) && L.tracks.has(id) && !S().hidden[id]) { seen.add(id); out.push(id); } };
    for (const id of P.history || []) add(id);
    const played = Object.entries(S().plays).filter(([id]) => L.tracks.has(id)).sort((a, b) => b[1][1] - a[1][1]).map(([id]) => id);
    for (const id of played) { if (out.length >= 60) break; add(id); }
    return out;
  }
  function drawQueue() {
    $$('#qTab button').forEach(b => b.classList.toggle('on', b.dataset.k === qMode));
    if (qMode === 'recent') {
      const ids = recentIds(), cur = curId();
      const html = ids.length ? `<div class="q-sec lbl">Recently played</div>` + ids.map((id, idx) => {
        const t = L.get(id), now = id === cur;
        return `<div class="qi${now ? ' cur' : ''}" data-rec="${attr(id)}" style="animation-delay:${Math.min(idx, 12) * 22}ms">${ART.html(t, t.albumKey || t.id, t.B || t.T, 'sm')}` +
          `<div class="tt"><span class="t-title ellip">${esc(t.T)}</span><span class="t-sub ellip">${esc(t.A || 'Unknown artist')}</span></div>` +
          `${now && P.playing ? '<span class="eq"><i></i><i></i><i></i><i></i></span>' : ''}</div>`;
      }).join('') : '<div class="empty-note" style="margin:20px 10px">Songs you play show up here</div>';
      $('#qBody').innerHTML = html;
      $('#qClear').hidden = true;
      ART.fill($('#qBody'));
      return;
    }
    const q = S().queue, cur = P.current(), up = P.upcoming().filter(x => L.tracks.has(x.id));
    const manual = up.filter(x => x.m), rest = up.filter(x => !x.m).slice(0, 120);
    let html = '';
    if (cur) html += `<div class="q-sec lbl">Now playing</div>${qItem(cur, q.at, true)}`;
    if (manual.length) html += `<div class="q-sec lbl">Next in queue</div>${manual.map(x => qItem(L.get(x.id), x.i)).join('')}`;
    if (rest.length) {
      const from = P.context(rest[0].i), r = ctxRoute(from), nm = esc(ctxName(from) || 'the queue');
      html += `<div class="q-sec lbl">Next from ${r ? `<a class="link" data-go-to="${attr(r)}" data-focus-id="${attr(rest[0].id)}">${nm}</a>` : nm}</div>${rest.map(x => qItem(L.get(x.id), x.i)).join('')}`;
    }
    if (!cur && !up.length) html = '<div class="empty-note" style="margin:20px 10px">The queue is empty. Use “Add to queue” on any song.</div>';
    $('#qBody').innerHTML = html;
    $('#qClear').hidden = !manual.length;
    ART.fill($('#qBody'));
  }
  $('#qTab').addEventListener('click', e => { const b = e.target.closest('[data-k]'); if (b && b.dataset.k !== qMode) { qMode = b.dataset.k; drawQueue(); } });
  $('#qClose').addEventListener('click', closeQueue);
  $('#qClear').addEventListener('click', () => P.clearQueue());
  $('#qBody').addEventListener('click', e => { const x = e.target.closest('[data-qx]'); if (x) { e.stopPropagation(); P.removeAt(+x.dataset.qx); } });
  $('#qBody').addEventListener('dblclick', e => {
    const rec = e.target.closest('[data-rec]');
    if (rec) {
      const ids = recentIds(), i = ids.indexOf(rec.dataset.rec);
      if (i >= 0) P.playIds(ids, i, { kind: 'recent', key: 'lately', name: 'Recently played' });
      return;
    }
    const r = e.target.closest('[data-q]'); if (r && !e.target.closest('button')) P.jump(+r.dataset.q);
  });
  $('#qBody').addEventListener('contextmenu', e => {
    const rec = e.target.closest('[data-rec]');
    if (rec) { e.preventDefault(); trackMenu([rec.dataset.rec], e.clientX, e.clientY); return; }
    const r = e.target.closest('[data-q]'); if (!r) return; e.preventDefault(); const it = S().queue.items[+r.dataset.q]; if (it) trackMenu([it.id], e.clientX, e.clientY);
  });
  $('#qBody').addEventListener('dragstart', e => { const r = e.target.closest('[data-q]'); if (!r) return; qDrag = +r.dataset.q; r.classList.add('dragging'); e.dataTransfer.effectAllowed = 'move'; e.dataTransfer.setData('text/plain', ''); });
  $('#qBody').addEventListener('dragover', e => {
    if (qDrag < 0) return;
    const r = e.target.closest('[data-q]');
    e.preventDefault();
    $$('#qBody .drop-above').forEach(x => x.classList.remove('drop-above'));
    if (r && +r.dataset.q > S().queue.at) r.classList.add('drop-above');
  });
  $('#qBody').addEventListener('drop', e => {
    const r = e.target.closest('[data-q]');
    e.preventDefault();
    if (qDrag >= 0 && r) P.move(qDrag, +r.dataset.q);
    qDrag = -1;
  });
  $('#qBody').addEventListener('dragend', () => { qDrag = -1; drawQueue(); });

  /* ================================================================== now playing */
  // two faces: the record (art, ring, what's next) and the lyrics; the player bar's lyrics button opens the second
  const np = { open: false, lastId: null, mode: 'record' };
  function openNP(mode) {
    if (mode) setNpMode(mode);
    if (np.open) return;
    np.open = true;
    closeMenu();
    npRender();
    $('#np').classList.add('open');
    $('#npBtn').classList.toggle('on', np.mode === 'record');
    $('#lyrBtn').classList.toggle('on', np.mode === 'lyrics');
    N.viz.show('big', true);
    lyWake();
  }
  function closeNP() {
    if (!np.open) return;
    np.open = false;
    $('#np').classList.remove('open');
    $('#npBtn').classList.remove('on');
    $('#lyrBtn').classList.remove('on');
    N.viz.show('big', false);
  }
  function setNpMode(mode) {
    np.mode = mode === 'lyrics' ? 'lyrics' : 'record';
    $('#np').classList.toggle('lyr', np.mode === 'lyrics');
    $$('#npMode button').forEach(b => b.classList.toggle('on', b.dataset.k === np.mode));
    if (np.open) { $('#npBtn').classList.toggle('on', np.mode === 'record'); $('#lyrBtn').classList.toggle('on', np.mode === 'lyrics'); }
    if (np.mode === 'lyrics' && np.open && LY.id !== curId()) loadLyrics();
    lyWake();
  }
  function npRender() {
    const t = P.current();
    ART.set($('#npArt'), t, 'lg', t ? t.albumKey || t.id : 'string', t ? t.B || t.T : '');
    const amb = $('#npAmb');
    if (t) ART.url(t, 'md').then(u => { if (P.current() === t) amb.style.backgroundImage = u ? `url("${u}")` : 'none'; });
    else amb.style.backgroundImage = 'none';
    npInfo((t ? t.id : '') !== np.lastId);
    np.lastId = t ? t.id : '';
    if (np.mode === 'lyrics' && LY.id !== (t ? t.id : '')) loadLyrics();
  }

  /* ---------- lyrics: synced lines light up and scroll along; click one to go there ---------- */
  const LY = { id: null, res: null, idx: -2, userAt: 0, raf: 0 };
  function lyHead(t, r) {
    const src = !r ? '' : r.source === 'lrc' ? `From ${esc(L.base(t.path).replace(/\.[^.]+$/, ''))}.lrc` : r.source === 'file' ? 'From the song file'
      : r.source === 'lrclib' ? 'Lyrics from <a class="link" data-ext="https://lrclib.net">LRCLIB</a>' : '';
    const has = r && r.source !== 'none' && !r.instrumental;
    const kind = has ? (r.synced ? '' : ' · not synced') : '';
    const tr = has ? `<button class="ly-tbtn${S().set.translate ? ' on' : ''}" data-ly="tr" title="Translate on this PC (T)">${belt.icon('globe')}<span>Translate</span></button>` : '';
    return `<div class="ly-h"><div class="ly-t"><b class="ellip">${esc(t.T)}</b><span class="ellip">${esc(t.A || 'Unknown artist')}</span></div><div class="ly-r"><span class="ly-src">${src}${kind}</span>${tr}</div></div>` +
      (has ? '<div class="ly-status" id="lyStatus"></div>' : '');
  }
  async function loadLyrics(force) {
    const t = P.current(), box = $('#npLyrics');
    N.translate.stop();
    box.classList.remove('show-tr');
    LY.id = t ? t.id : ''; LY.res = null; LY.idx = -2;
    if (!t) { box.innerHTML = '<div class="ly-empty"><b>Nothing playing</b></div>'; return; }
    box.innerHTML = lyHead(t, null) + `<div class="ly-scroll"><div class="ly-wait">${'<i></i>'.repeat(7)}</div></div>`;
    // online, LRCLIB is asked only once the song has stayed a moment (skipping through songs costs them nothing)
    const r = await N.lyrics.get(t, { force, settle: { ms: 1200, stillWanted: () => LY.id === t.id && curId() === t.id && np.open } });
    if (r.skipped) return;
    if (LY.id !== t.id) return;
    LY.res = r;
    drawLyrics(t, r);
  }
  function drawLyrics(t, r) {
    const box = $('#npLyrics'), s = S().set;
    let body;
    if (r.source === 'none' && s.lyricsOnline == null) {
      body = `<div class="ly-empty"><b>Look for lyrics online?</b><p>This song has no lyrics of its own. String can ask <b>LRCLIB</b> (lrclib.net), a free, open lyrics library, which often has them synced to the music. It sends the song's title, artist, album and length, nothing else, and keeps what it finds on this PC.</p>` +
        `<div class="row"><button class="btn primary sm" data-ly="yes">Look them up</button><button class="btn sm ghost" data-ly="no">Only songs' own lyrics</button></div><p class="ly-fine">You can change this in Library → Playback.</p></div>`;
    } else if (r.source === 'none') {
      body = `<div class="ly-empty"><b>${r.busy ? 'LRCLIB is busy' : 'No lyrics for this one'}</b><p>${r.busy ? 'Try again in a minute.' : s.lyricsOnline ? "LRCLIB doesn't have this song (or not under this title)." : "Looking online is off (Library → Playback)."}</p>` +
        `<p class="ly-fine">Lyrics of your own: put a .lrc file named like the song next to it.</p><div class="row"><button class="btn sm" data-ly="again">${belt.icon('reload')}Look again</button></div></div>`;
    } else if (r.instrumental) {
      body = `<div class="ly-empty"><b>Instrumental</b><p>No words in this one.</p></div>`;
    } else {
      body = `<div class="ly-scroll${r.synced ? ' synced' : ''}" id="lyScroll"><div class="ly-pad"></div>` +
        r.lines.map((l, i) => `<p class="ll${l.text ? '' : ' gap'}" data-i="${i}">${l.text ? esc(l.text) : '♪'}</p>`).join('') + `<div class="ly-pad"></div></div>`;
    }
    box.innerHTML = lyHead(t, r) + body;
    const sc = $('#lyScroll');
    if (sc) ['wheel', 'touchstart', 'pointerdown'].forEach(ev => sc.addEventListener(ev, e => { if (ev !== 'pointerdown' || e.button !== 0 || !e.target.closest('.ll')) LY.userAt = Date.now(); }, { passive: true }));
    LY.idx = -2;
    lyWake();
    if (s.translate && sc) translateLyrics(t, r);
  }

  /* ---------- translation: a model on this PC (Ollama) writes each line's translation under it, as it goes ---------- */
  function trStatus(text) { const el = $('#lyStatus'); if (el) el.textContent = text; }
  function trApply(per) {
    $$('#npLyrics .ll').forEach((el, i) => {
      const v = per[i];
      let sp = el.querySelector('.ll-tr');
      if (!v) { if (sp) sp.textContent = ''; return; }
      if (!sp) { sp = document.createElement('span'); sp.className = 'll-tr'; el.appendChild(sp); }
      if (sp.textContent !== v) { sp.textContent = v; sp.classList.remove('in'); void sp.offsetWidth; sp.classList.add('in'); }
    });
  }
  async function translateLyrics(t, r) {
    const s = S().set, lang = s.trLang || 'English';
    $('#npLyrics').classList.add('show-tr');
    trStatus(`Translating into ${lang}…`);
    try {
      let model = s.trModel;
      if (!model) {
        model = N.translate.pickModel(await N.translate.models());
        if (!model) throw new Error('Ollama has no models yet. Download one first, for example: ollama pull qwen3:14b');
      }
      if (LY.id !== t.id) return;
      trStatus(`Translating into ${lang} with ${model}, on this PC…`);
      const res = await N.translate.run(t, r.lines.map(l => l.text), { lang, model, onUpdate: per => { if (LY.id === t.id) trApply(per); } });
      if (LY.id !== t.id || res.stopped) return;
      trApply(res.lines);
      trStatus(res.error && !res.lines.some(Boolean) ? `Couldn't translate: ${res.error}` : res.same ? `Already in ${lang}` : `Translated into ${lang} by ${model}, on this PC` + (res.error ? ` (${res.error})` : ''));
    } catch (e) {
      if (LY.id === t.id) trStatus(`Couldn't translate: ${e.message}`);
    }
  }
  function toggleTranslate() {
    const s = S().set, t = P.current();
    s.translate = !s.translate;
    N.save();
    $$('#npLyrics .ly-tbtn').forEach(b => b.classList.toggle('on', s.translate));
    if (s.translate && t && LY.res && LY.id === t.id && $('#lyScroll')) translateLyrics(t, LY.res);
    else { N.translate.stop(); $('#npLyrics').classList.remove('show-tr'); trStatus(''); }
  }
  // which line is being sung, every frame while the lyrics are in view and the music plays
  function lyWake() { if (canDraw() && !LY.raf && np.open && np.mode === 'lyrics' && LY.res && LY.res.synced) LY.raf = requestAnimationFrame(lyTick); }
  function lyTick() {
    LY.raf = 0;
    if (!canDraw() || !np.open || np.mode !== 'lyrics' || !LY.res || !LY.res.synced || LY.id !== curId()) return;
    const i = N.lyrics.lineAt(LY.res.lines, P.time() + .15);
    if (i !== LY.idx) lySet(i);
    if (P.playing && belt.visible !== false) LY.raf = requestAnimationFrame(lyTick);
  }
  function lySet(i) {
    LY.idx = i;
    const sc = $('#lyScroll');
    if (!sc) return;
    $$('.ll', sc).forEach((el, k) => { el.classList.toggle('on', k === i); el.classList.toggle('past', k < i); });
    if (Date.now() - LY.userAt < 3500) return;   // you're reading ahead: don't pull the page away
    const el = i >= 0 ? sc.children[i + 1] : sc.firstElementChild;
    if (el) sc.scrollTo({ top: Math.max(0, el.offsetTop - sc.clientHeight * .38 + el.offsetHeight / 2), behavior: calm() ? 'auto' : 'smooth' });
  }
  $('#npLyrics').addEventListener('click', e => {
    const b = e.target.closest('[data-ly]');
    if (b) {
      const k = b.dataset.ly;
      if (k === 'tr') { toggleTranslate(); return; }
      if (k === 'yes' || k === 'no') { S().set.lyricsOnline = k === 'yes'; N.save(); }
      loadLyrics(k !== 'no');
      return;
    }
    const line = e.target.closest('.ll');
    if (line && LY.res && LY.res.synced) { LY.userAt = 0; P.seek(LY.res.lines[+line.dataset.i].t); lyWake(); }
  });
  $('#npMode').addEventListener('click', e => { const b = e.target.closest('[data-k]'); if (b) setNpMode(b.dataset.k); });
  $('#lyrBtn').addEventListener('click', () => { if (np.open && np.mode === 'lyrics') closeNP(); else { if (!P.current()) return; openNP('lyrics'); } });
  N.on('time', () => { LY.idx = -2; lyWake(); });
  N.on('state', lyWake);
  function npInfo(animate) {
    const t = P.current(), from = P.context(), sw = animate && !calm() ? ' swap-in' : '';
    if (!t) { $('#npInfo').innerHTML = '<span class="lbl">Nothing playing</span>'; return; }
    const up = P.upcoming().filter(x => L.tracks.has(x.id)).slice(0, 3);
    const cr = ctxRoute(from), cn = esc(ctxName(from) || 'your queue');
    $('#npInfo').innerHTML = `<span class="lbl${sw}">Playing from ${cr ? `<a class="link" data-np-ctx="${attr(cr)}">${cn}</a>` : cn}</span>` +
      `<h1 class="${sw}">${esc(t.T)}</h1>` +
      `<div class="np-a${sw}">${t.A ? `<a class="link" data-artist="${attr(t.artistKey)}">${esc(t.A)}</a>` : 'Unknown artist'}</div>` +
      `<div class="np-b${sw}">${t.B ? `<a class="link" data-album="${attr(t.albumKey)}">${esc(t.B)}</a>${t.year ? ' · ' + t.year : ''} · ` : ''}<span id="npSeek">${mmss(P.time())} / ${mmss(P.duration())}</span>${t.format ? ' · ' + esc(t.format.toUpperCase()) : ''}</div>` +
      `<div class="np-act"><button class="icon-btn like${isLiked(t.id) ? ' on' : ''}" data-np="like" title="Like (L)">${belt.icon('heart')}</button><button class="icon-btn" data-np="add" title="Add to playlist">${belt.icon('plus')}</button><button class="icon-btn" data-np="queue" title="Queue (Q)">${icon('queue')}</button><button class="icon-btn" data-np="more" title="More">${icon('more')}</button></div>` +
      (up.length ? `<div class="np-next"><div class="lbl" style="margin-bottom:8px">Up next</div>${up.map(x => qItem(L.get(x.id), x.i)).join('')}</div>` : '');
    ART.fill($('#npInfo'));
  }
  $('#npClose').addEventListener('click', closeNP);
  $('#npInfo').addEventListener('click', e => {
    const cx = e.target.closest('[data-np-ctx]'); if (cx) { closeNP(); go(cx.dataset.npCtx, { focusId: curId() }); return; }
    const ar = e.target.closest('[data-artist]'); if (ar) { closeNP(); go('artist/' + enc(ar.dataset.artist)); return; }
    const al = e.target.closest('[data-album]'); if (al) { closeNP(); go('album/' + enc(al.dataset.album)); return; }
    const x = e.target.closest('[data-qx]'); if (x) { P.removeAt(+x.dataset.qx); return; }
    const b = e.target.closest('[data-np]'), t = P.current(); if (!b || !t) return;
    const r = b.getBoundingClientRect();
    if (b.dataset.np === 'like') toggleLike([t.id], b);
    else if (b.dataset.np === 'add') menu([{ head: 'Add to playlist' }, ...playlistPicker([t.id])], r.left, r.bottom + 4);
    else if (b.dataset.np === 'queue') openQueue();
    else if (b.dataset.np === 'more') trackMenu([t.id], r.left, r.bottom + 4);
  });
  $('#npInfo').addEventListener('dblclick', e => { const r = e.target.closest('[data-q]'); if (r && !e.target.closest('button')) P.jump(+r.dataset.q); });
  $('#npStage').addEventListener('click', () => P.toggle());

  /* ================================================================== search box, top bar, scrolling */
  let searchTimer = 0;
  $('#q').addEventListener('input', () => {
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => { if (route() === 'search') render({ top: true }); else go('search'); }, 140);
  });
  $('#q').addEventListener('keydown', e => {
    if (e.key === 'Escape') { $('#q').value = ''; $('#q').blur(); if (route() === 'search') back(); }
    if (e.key === 'Enter' && route() === 'search') { const r = L.search($('#q').value); if (r.tracks.length) P.playIds(r.tracks, 0, { kind: 'search', key: $('#q').value.trim(), name: `“${$('#q').value.trim()}”` }); }
  });
  $('#q').addEventListener('focus', () => { if ($('#q').value.trim() && route() !== 'search') go('search'); });
  $('#back').addEventListener('click', back);
  $('#fwd').addEventListener('click', fwd);
  $('#openSet').addEventListener('click', () => go('settings'));
  $('#nav').addEventListener('click', e => { const b = e.target.closest('[data-go]'); if (b) go(b.dataset.go); });
  wireCards($('#view'));
  let scrollRaf = 0;
  function onScroll() {
    const sc = $('#scroll');
    $('#top').classList.toggle('solid', sc.scrollTop > 40);
    const h1 = $('#view .hero-t h1, #view .page-title h1');
    $('#topTitle').textContent = h1 && sc.scrollTop > 140 ? h1.textContent : '';
    lists.forEach(l => l.paint());
  }
  $('#scroll').addEventListener('scroll', () => { if (!scrollRaf) scrollRaf = requestAnimationFrame(() => { scrollRaf = 0; onScroll(); }); }, { passive: true });
  addEventListener('resize', () => lists.forEach(l => l.paint()));
  $('#view').addEventListener('click', e => {
    const ar = e.target.closest('.hero [data-artist], .meta [data-artist], .rank[data-artist]'); if (ar) { e.stopPropagation(); go('artist/' + enc(ar.dataset.artist)); }
  });

  /* ================================================================== songs and folders dropped from Explorer */
  let dragDepth = 0;
  const external = e => e.dataTransfer && [...e.dataTransfer.types].includes('Files') && !dragging;
  addEventListener('dragenter', e => { if (!external(e)) return; dragDepth++; $('#dropzone').classList.add('on'); });
  addEventListener('dragleave', e => { if (!external(e)) return; if (--dragDepth <= 0) { dragDepth = 0; $('#dropzone').classList.remove('on'); } });
  addEventListener('dragover', e => { if (external(e)) { e.preventDefault(); e.dataTransfer.dropEffect = 'copy'; } });
  addEventListener('drop', async e => {
    if (!external(e)) return;
    e.preventDefault();
    dragDepth = 0;
    $('#dropzone').classList.remove('on');
    let paths = [];
    try { paths = await belt.native.dropped(e.dataTransfer.files); } catch (err) { belt.toast(err.message); return; }
    if (!paths.length) return;
    const st = await belt.native.stats(paths).catch(() => []);
    const dirs = paths.filter((_, i) => st[i] && st[i].dir), files = paths.filter((p, i) => st[i] && !st[i].dir && L.AUDIO.test(p));
    if (dirs.length) addRoots(dirs);
    if (files.length) addSongs(files);
    if (!dirs.length && !files.length) belt.toast('Those aren\'t songs String can play');
  });

  /* ================================================================== keys */
  addEventListener('keydown', e => {
    const typing = e.target.closest && e.target.closest('input, textarea, [contenteditable]');
    if (e.key === 'Escape') {
      if (menuEl) { closeMenu(); return; }
      if (np.open) { closeNP(); return; }
      if (qOpen) { closeQueue(); return; }
    }
    if (typing) return;
    const k = e.key, ctrl = e.ctrlKey || e.metaKey;
    if ((k === ' ' || k === 'Enter') && e.target.id === 'tDur') return;
    if (e.altKey && k === 'ArrowLeft') { e.preventDefault(); back(); return; }
    if (e.altKey && k === 'ArrowRight') { e.preventDefault(); fwd(); return; }
    if (e.altKey) return;
    if (k === ' ') { e.preventDefault(); P.toggle(); }
    else if (ctrl && k === 'ArrowRight') { e.preventDefault(); P.next(); }
    else if (ctrl && k === 'ArrowLeft') { e.preventDefault(); P.prev(); }
    else if (ctrl && k === 'ArrowUp') { e.preventDefault(); P.setVolume((S().muted ? 0 : S().vol) + .05); }
    else if (ctrl && k === 'ArrowDown') { e.preventDefault(); P.setVolume((S().muted ? 0 : S().vol) - .05); }
    else if (k === 'ArrowRight' && !ctrl) { e.preventDefault(); P.seek(P.time() + 5); }
    else if (k === 'ArrowLeft' && !ctrl) { e.preventDefault(); P.seek(P.time() - 5); }
    else if (ctrl && (k === 'f' || k === 'F')) { e.preventDefault(); $('#q').focus(); $('#q').select(); }
    else if (ctrl && (k === 'a' || k === 'A') && activeList) { e.preventDefault(); activeList.sel = new Set(activeList.ids.map((_, i) => i)); activeList.paint(true); }
    else if (ctrl) return;
    else if (k === '/') { e.preventDefault(); $('#q').focus(); $('#q').select(); }
    else if (k === 'm' || k === 'M') P.toggleMute();
    else if (k === 's' || k === 'S') P.setShuffle(!S().queue.shuffle);
    else if (k === 'r' || k === 'R') P.cycleRepeat();
    else if (k === 'l' || k === 'L') { const t = P.current(); if (t) toggleLike([t.id], $('#nowLike')); }
    else if (k === 'q' || k === 'Q') qOpen ? closeQueue() : openQueue();
    else if (k === 'f' || k === 'F') np.open && np.mode === 'record' ? closeNP() : P.current() && openNP('record');
    else if (k === 'y' || k === 'Y') np.open && np.mode === 'lyrics' ? closeNP() : P.current() && openNP('lyrics');
    else if ((k === 't' || k === 'T') && np.open && np.mode === 'lyrics' && $('#npLyrics .ly-tbtn')) toggleTranslate();
    else if (k === 'Enter' && activeList && activeList.sel.size) activeList.play(Math.min(...activeList.sel));
    else if (k === 'Delete' && activeList && activeList.o.pl && activeList.sel.size) removePlaylistEntries(activeList.o.pl, activeList.playlistSelection());
    else if (k === '[') loopAt('a', P.time());
    else if (k === ']') loopAt('b', P.time());
    else if (k === '\\' && P.loop()) { P.clearLoop(); belt.toast('Loop off'); }
  });
  addEventListener('mouseup', e => { if (e.button === 3) { e.preventDefault(); back(); } else if (e.button === 4) { e.preventDefault(); fwd(); } });

  /* ================================================================== events from the library and the player */
  let libTimer = 0;
  N.on('lib', () => {
    clearTimeout(libTimer);
    libTimer = setTimeout(() => {
      sidebar();
      const r = route();
      // a page made from the library is drawn again; a search keeps its results as typed
      if (r !== 'settings' || !document.activeElement.closest('#view')) render({ keep: true });
      if (!lastBarId && P.current()) barTrack();
      if (qOpen) drawQueue();
    }, 120);
  });
  N.on('scan', () => {
    const p = L.progress;
    $('#scanLine').classList.toggle('on', !!p);
    $('#scanNote').textContent = !p ? '' : p.phase === 'look' ? `Looking… ${p.done ? p.done.toLocaleString() : ''}` : p.total ? `Reading tags ${p.done.toLocaleString()} / ${p.total.toLocaleString()}` : '';
    // the "reading your songs" screen counts along
    const w = $('#view .welcome p');
    if (p && w && !L.all.length) w.textContent = p.phase === 'look' ? `Looking through your folders… ${p.done ? plural(p.done, 'song') + ' so far' : ''}` : `Reading tags: ${p.done.toLocaleString()} of ${p.total.toLocaleString()}`;
    else if (!p && $('#view .welcome') && L.all.length) render({ keep: true });
  });
  N.on('track', barTrack);
  N.on('state', barState);
  N.on('time', () => tickNow(true));
  N.on('queue', () => { if (qOpen) drawQueue(); if (np.open) npInfo(false); sidebar(); markNow(); });
  N.on('history', () => { if (qOpen && qMode === 'recent') drawQueue(); });
  N.on('modes', modes);
  N.on('volume', volUI);
  N.on('error', t => belt.toast(t ? `Can't play “${t.T}”: skipping it` : 'Can\'t play that song'));
  N.on('blocked', () => belt.toast('Press play to start the music'));
  // listening time: today's total by Stats in the sidebar; the Stats page itself follows along (not while hovering it)
  function statsToday() {
    const M = N.stats.model, p = M.period('day');
    $('#cToday').textContent = N.stats.model.firstDay() ? dur(M.range(p.from, p.to).total, true) : '';
  }
  // listening time ticks in every few seconds while a song plays; the Stats page is drawn again at most every 20 s, and
  // not while the pointer is on it or one of its controls (the phone account list) is in use
  function refreshStats() {
    if (!canDraw()) return;
    statsToday();
    if (route() === 'stats' && Date.now() - statsDrawn > 20000 && !$('#view .chart:hover, #view .stat-cols:hover, #view select:focus, #view .stat-phone:hover')) render({ keep: true });
  }
  N.on('stats', refreshStats);
  function syncDrawing() {
    document.documentElement.classList.toggle('string-hidden', !canDraw());
    if (canDraw()) { tickNow(true); lyWake(); refreshStats(); }
    else {
      cancelAnimationFrame(tickRaf); tickRaf = 0;
      cancelAnimationFrame(LY.raf); LY.raf = 0;
    }
  }
  belt.onHide(syncDrawing);
  belt.onShow(syncDrawing);
  document.addEventListener('visibilitychange', syncDrawing);
  belt.onShow(() => { N.phone.sync(true); if (Date.now() - L.lastScan > 10 * 60e3 && (S().roots.length || S().files.length) && !L.scanning) rescan(); });
  belt.onFlush(() => N.save());
  // closing found changes that couldn't be saved (a full or locked disk): say so, and let you choose
  belt.onCloseBlocked(async why => {
    const yes = await ask({
      title: 'Your latest changes aren’t saved',
      text: `String couldn’t write them to the disk (${why || 'no reason given'}). While it’s open it keeps trying. Close anyway, and lose what changed since the last save?`,
      ok: 'Close anyway', cancel: 'Keep String open', danger: true,
    });
    if (yes) belt.closeAnyway(); else belt.retrySaves();
  });
  // the buttons under String's taskbar thumbnail (hover its taskbar icon)
  belt.onMedia(a => { if (a === 'prev') P.prev(); else if (a === 'next') P.next(); else P.toggle(); });

  /* ================================================================== start */
  N.boot = (async () => {
    // add-ons (if any are installed) load meanwhile, so their pages and buttons are there before the first page
    const [{ state: saved, listening }] = await Promise.all([belt.load(), belt.addonsReady]);
    if (saved && saved.v === 1) N.S = merge(DEFAULTS(), saved);
    applyLook();
    N.stats.load(listening);
    statsToday();
    volUI();
    modes();
    await L.loadCache();
    N.emit('cacheLoaded');
    if (L.tracks.size) { L.index(); L.loaded = true; }
    addonButtons();
    const q = S().queue;
    if (!Array.isArray(q.orig)) {
      if (q.ctx && LIB_CTX.includes(q.ctx.kind)) P.restoreLegacyOrder(q, ctxIds(q.ctx));
      else q.orig = q.items.map(x => x.id);
    }
    const r = S().route || 'home';
    H.stack = [r === 'search' ? 'home' : r];
    H.at = 0;
    sidebar();
    render();
    P.restore();
    barTrack();
    barState();
    belt.profileReady();
    N.emit('booted');
    // songs played on the phone (through Navidrome) join Stats: a moment after start, then every 5 minutes
    setTimeout(() => N.phone.sync(true), 8000);
    setInterval(() => N.phone.sync(true), 5 * 60e3);
    if (S().roots.length || S().files.length) rescan();
    discordSync();
    belt.ready.then(hello => { if (hello && hello.smoke) return; setTimeout(maybeCheckUpdates, 60e3); setInterval(maybeCheckUpdates, 6 * 3600e3); iconCredit(); });
    return true;
  })().catch(error => {
    const dialog = document.createElement('dialog');
    dialog.innerHTML = '<h2>Your library could not open</h2><p>String has paused saving to protect your playlists and listening history.</p><p></p><button class="btn primary">Try again</button>';
    dialog.querySelectorAll('p')[1].textContent = error.message;
    dialog.querySelector('button').onclick = () => location.reload();
    dialog.addEventListener('cancel', e => e.preventDefault());
    document.body.appendChild(dialog);
    dialog.showModal();
    return false;
  });

  /* A thank-you to the icon's maker: the first time String starts with this icon, a small card above the player bar,
     until it's closed. set.iconCreditSeen is shared with String 2.0 (the same data), so it's shown once in either. */
  function iconCredit() {
    if (S().set.iconCreditSeen || document.querySelector('.credit-card')) return;
    const card = document.createElement('div');
    card.className = 'credit-card';
    card.innerHTML = `<img src="logo.png" alt=""><span>Credits to Discord — awepollyon for the icon!</span><button class="icon-btn" title="Close">${icon('x')}</button>`;
    card.querySelector('button').onclick = () => { S().set.iconCreditSeen = true; N.save(); card.remove(); };
    document.body.appendChild(card);
  }

  N.discord = D;
  N.snapshot = snapshot;
  N.mergeState = merge;
  N.ui = { discordSync, go, back, fwd, render, route, openNP, closeNP, openQueue, closeQueue, trackMenu, ctxMenu, closeMenu, editInfo, newPlaylist, addToPlaylist, addRoots, addSongs, toggleLike, lists: () => lists, mmss, ctxIds, playCtx, deletePl, removeFromPlaylist, ask, iconCredit,
    h: { $, $$, esc, attr, plural, enc, pageTitle } };   // small helpers, for add-ons drawing pages of their own
})();
