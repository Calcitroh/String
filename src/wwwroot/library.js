/* String library: the songs in your chosen folders (and any single songs added), their tags, albums and artists.
   Tags are read once and kept in the data folder's library.json (rebuilt from the songs if it goes missing); a song is
   read again only when its size or date changes. Never writes to the music folders. */
(function () {
  'use strict';
  const N = window.N = window.N || {};
  const AUDIO = /\.(mp3|flac|m4a|m4b|aac|ogg|oga|opus|wav|weba|webm)$/i;
  const COVER = /^(cover|folder|front|album|albumart(_\{[^}]*\})?(large|small)?|art)\.(jpe?g|png|webp)$/i;
  const SKIP_DIR = /^(\$|\.)|^(node_modules|System Volume Information|__MACOSX)$/i;
  // readError: the tags couldn't be read last time (the file busy or locked); read again at the next scan
  const KEEP = ['path', 'size', 'mtime', 'added', 'title', 'artist', 'album', 'albumArtist', 'track', 'disc', 'year', 'genre', 'duration', 'art', 'format', 'cover', 'readError'];
  const coll = new Intl.Collator(undefined, { numeric: true, sensitivity: 'base' });
  const norm = s => String(s || '').normalize('NFKD').replace(/[̀-ͯ]/g, '').toLowerCase().replace(/\s+/g, ' ').trim();

  const L = N.lib = {
    tracks: new Map(), albums: new Map(), artists: new Map(), all: [], version: 0,
    loaded: false, scanning: null, progress: null, lastScan: 0, coll, norm,
  };

  /* ---------- tiny event hub shared by the modules ---------- */
  const subs = new Map();
  N.on = (ev, fn) => { if (!subs.has(ev)) subs.set(ev, []); subs.get(ev).push(fn); };
  N.emit = (ev, arg) => (subs.get(ev) || []).forEach(fn => { try { fn(arg); } catch (e) { console.error(e); } });

  // read(start, length) for tags.js, over the host's seekable /_media/ address (App.cs)
  N.rangeReader = url => async (o, n) => {
    const r = await fetch(url, { headers: { Range: `bytes=${o}-${o + n - 1}` }, cache: 'no-store' });
    if (!r.ok) throw new Error('could not read the file (' + r.status + ')');
    const b = new Uint8Array(await r.arrayBuffer());
    return r.status === 206 ? b : b.subarray(o, o + n);
  };

  /* ---------- names ---------- */
  const base = p => p.slice(Math.max(p.lastIndexOf('\\'), p.lastIndexOf('/')) + 1);
  const dirOf = p => p.slice(0, Math.max(p.lastIndexOf('\\'), p.lastIndexOf('/')));
  // a title from a file name: no extension, no "[video id]" a downloader added, no leading track number
  function nameTitle(name) {
    let s = name.replace(/\.[^.]+$/, '');
    s = s.replace(/\s*[\[(](?:[A-Za-z0-9_-]{11}|\d{6,})[\])]\s*$/, '');   // a downloader's video ids: [Ab12Cd34Ef5], [1234567890123456789]
    s = s.replace(/\s*\(\d\)$/, '');                                       // "song(1)" copies
    s = s.replace(/^\s*\d{1,3}\s*[-.)_]\s+(?=\S)/, '');                   // "01 - ", "098. "
    s = s.replace(/_/g, ' ').replace(/\s{2,}/g, ' ').trim();
    return s || name;
  }
  const leadNum = name => { const m = name.match(/^\s*(\d{1,3})\s*[-.)_ ]/); return m ? +m[1] : 0; };

  /* ---------- making a track the screens can use ---------- */
  function finalize(t) {
    const e = (N.S && N.S.edits && N.S.edits[t.id]) || {};
    t.T = e.title || t.title || nameTitle(t.name);
    t.A = e.artist || t.artist || '';
    t.B = e.album || t.album || '';
    t.AA = (!e.artist && t.albumArtist) || t.A;
    t.n = t.track || leadNum(t.name);
    t.albumKey = t.B ? norm(t.B) + '\u0001' + (e.album ? norm(t.A) : t.albumArtist ? norm(t.albumArtist) : norm(t.dir)) : '';
    t.artistKey = norm(t.A);
    t.q = norm([t.T, t.A, t.B, t.albumArtist, t.genre, t.name].join(' '));
    return t;
  }
  function make(raw) {
    const t = { ...raw };
    t.id = raw.path.toLowerCase();
    t.name = base(raw.path);
    t.dir = dirOf(raw.path);
    return finalize(t);
  }

  // a song you hid (the duplicate checker's "Hide in String") stays out of the lists; its file is untouched
  const shown = t => !(N.S && N.S.hidden && N.S.hidden[t.id]);
  function index() {
    const albums = new Map(), artists = new Map();
    const all = [...L.tracks.values()].filter(shown);
    for (const t of all) {
      if (t.albumKey) {
        let a = albums.get(t.albumKey);
        if (!a) albums.set(t.albumKey, a = { key: t.albumKey, name: t.B, ids: [], artists: new Map(), year: 0, added: 0, dir: t.dir });
        a.ids.push(t.id);
        a.artists.set(t.AA, (a.artists.get(t.AA) || 0) + 1);
        a.year = Math.max(a.year, t.year || 0);
        a.added = Math.max(a.added, t.added || 0);
      }
      let r = artists.get(t.artistKey);
      if (!r) artists.set(t.artistKey, r = { key: t.artistKey, name: t.A || 'Unknown artist', ids: [], albums: new Set(), added: 0 });
      r.ids.push(t.id);
      if (t.albumKey) r.albums.add(t.albumKey);
      r.added = Math.max(r.added, t.added || 0);
    }
    for (const a of albums.values()) {
      const named = [...a.artists.entries()].filter(([k]) => k).sort((x, y) => y[1] - x[1]);
      a.artist = named.length > 2 && !L.tracks.get(a.ids[0]).albumArtist ? 'Various artists' : named.length ? named[0][0] : 'Unknown artist';
      a.artistKey = norm(named.length ? named[0][0] : '');
      a.ids.sort((x, y) => { const p = L.tracks.get(x), q = L.tracks.get(y); return (p.disc || 0) - (q.disc || 0) || (p.n || 999) - (q.n || 999) || coll.compare(p.T, q.T); });
      a.cover = a.ids.map(id => L.tracks.get(id)).find(t => N.art.source(t)) || L.tracks.get(a.ids[0]);
      a.duration = a.ids.reduce((s, id) => s + (L.tracks.get(id).duration || 0), 0);
    }
    for (const r of artists.values()) {
      r.ids.sort((x, y) => { const p = L.tracks.get(x), q = L.tracks.get(y); return coll.compare(p.B, q.B) || (p.n || 999) - (q.n || 999) || coll.compare(p.T, q.T); });
      const firstAlbum = [...r.albums][0];
      r.cover = (firstAlbum && albums.get(firstAlbum).cover) || r.ids.map(id => L.tracks.get(id)).find(t => N.art.source(t)) || L.tracks.get(r.ids[0]);
    }
    all.sort((p, q) => coll.compare(p.T, q.T) || coll.compare(p.A, q.A));
    L.all = all.map(t => t.id);
    L.albums = albums;
    L.artists = artists;
    L.version++;
  }

  // re-applies Edit info changes to every track (cheap), then indexes again
  function refresh() {
    for (const t of L.tracks.values()) finalize(t);
    index();
    N.emit('lib');
  }

  /* ---------- the cache in the data folder ---------- */
  let dataDir = '';
  async function dataPath() { if (!dataDir) dataDir = (await belt.native.paths()).data; return dataDir + '\\library.json'; }
  async function loadCache() {
    if (!belt.inBelt) return;
    try {
      const text = await belt.native.readText(await dataPath());
      if (!text) return;
      const j = JSON.parse(text);
      if (j && j.v === 1 && Array.isArray(j.tracks)) for (const raw of j.tracks) if (raw && raw.path) { const t = make(raw); L.tracks.set(t.id, t); }
    } catch (e) { console.warn('library cache', e); }
  }
  let saveTimer = 0;
  function saveCache() {
    clearTimeout(saveTimer);
    saveTimer = setTimeout(async () => {
      if (!belt.inBelt) return;
      const tracks = [...L.tracks.values()].map(t => {
        const o = {};
        for (const k of KEEP) { const v = t[k]; if (v !== '' && v !== 0 && v != null) o[k] = v; }
        return o;
      });
      try { await belt.native.writeText(await dataPath(), JSON.stringify({ v: 1, saved: Date.now(), tracks })); } catch (e) { console.warn('save library', e); }
    }, 800);
  }

  /* ---------- scanning ---------- */
  async function pool(items, n, fn) {
    let i = 0;
    await Promise.all(Array.from({ length: Math.min(n, items.length) }, async () => { while (i < items.length) { const k = i++; await fn(items[k], k); } }));
  }
  // unread: folders that couldn't be listed this time (no permission, a drive that stalled); the songs already known
  // under them stay in the library instead of being dropped as if they were gone
  async function walk(roots, found, covers, onDir, unread) {
    const queue = roots.map(p => ({ p, depth: 0 }));
    const seen = new Set();
    while (queue.length) {
      const batch = queue.splice(0, 8);
      await Promise.all(batch.map(async ({ p, depth }) => {
        const key = p.toLowerCase();
        if (seen.has(key)) return;
        seen.add(key);
        let list;
        try { list = await belt.native.list(p); } catch { unread.push(key.replace(/[\\/]+$/, '') + '\\'); return; }
        onDir();
        for (const f of list) {
          const full = p.replace(/[\\/]+$/, '') + '\\' + f.name;
          if (f.dir) { if (depth < 14 && !SKIP_DIR.test(f.name)) queue.push({ p: full, depth: depth + 1 }); continue; }
          if (COVER.test(f.name)) { const prev = covers.get(key); if (!prev || /^(cover|folder|front)/i.test(f.name)) covers.set(key, full); continue; }
          if (AUDIO.test(f.name) && f.size > 0) found.set(full.toLowerCase(), { path: full, size: f.size, mtime: f.mtime });
        }
      }));
    }
  }

  let activeSelection = '';
  function scan() {
    const selection = JSON.stringify([N.S.roots, N.S.files]);
    if (L.scanning) {
      if (selection === activeSelection) return L.scanning;
      return L.scanning.then(scan);
    }
    activeSelection = selection;
    L.scanning = (async () => {
      const roots = N.S.roots.slice(), files = N.S.files.slice();
      const found = new Map(), covers = new Map(), unread = [], keep = new Set();
      let dirs = 0;
      L.progress = { phase: 'look', done: 0, total: 0 };
      N.emit('scan');
      await walk(roots, found, covers, () => { dirs++; if (dirs % 20 === 0) { L.progress.done = found.size; N.emit('scan'); } }, unread);
      if (files.length) {
        try {
          const st = await belt.native.stats(files);
          files.forEach((p, i) => {
            const s = st[i];
            if (s && s.exists && !s.dir && s.size > 0) found.set(p.toLowerCase(), { path: p, size: s.size, mtime: s.mtime });
            else if (!s) keep.add(p.toLowerCase());   // no answer for it: not a sign it's gone
          });
        } catch { files.forEach(p => keep.add(p.toLowerCase())); }   // couldn't ask at all: keep them as they were
      }
      // what's new or changed gets read (and what couldn't be read last time); the rest comes from the cache
      const todo = [], now = Date.now(), firstScan = !L.lastScan && !L.tracks.size;
      let changed = 0;
      for (const [id, f] of found) {
        const old = L.tracks.get(id);
        const cover = covers.get(dirOf(f.path).toLowerCase()) || '';
        if (old && !old.readError && old.size === f.size && old.mtime === f.mtime) { if (old.cover !== cover) { old.cover = cover; changed++; } continue; }
        todo.push({ ...f, cover, added: old ? old.added : (firstScan ? f.mtime : now) });
      }
      let removed = 0;
      const stillThere = id => keep.has(id) || unread.some(dir => id.startsWith(dir));
      for (const id of [...L.tracks.keys()]) if (!found.has(id) && !stillThere(id)) { L.tracks.delete(id); removed++; }
      L.progress = { phase: 'read', done: 0, total: todo.length };
      N.emit('scan');
      let lastEmit = 0, lastIndexed = 0;
      await pool(todo, 6, async f => {
        let tags = null;
        try { tags = await StringTags.readTags(N.rangeReader(belt.native.mediaUrl(f.path)), f.size); } catch (e) { tags = { format: '', readError: 1 }; }
        const raw = { path: f.path, size: f.size, mtime: f.mtime, added: f.added, cover: f.cover };
        for (const k of KEEP) if (tags && tags[k] !== undefined && raw[k] === undefined) raw[k] = tags[k];
        const t = make(raw);
        L.tracks.set(t.id, t);
        L.progress.done++;
        if (Date.now() - lastEmit > 250) {
          lastEmit = Date.now();
          N.emit('scan');
          // the first songs show up while the rest load, then the screens catch up every so often
          const d = L.progress.done;
          if ((firstScan && lastIndexed === 0 && d >= 40) || d - lastIndexed >= 1500) { lastIndexed = d; index(); N.emit('lib'); }
        }
      });
      L.lastScan = Date.now();
      L.loaded = true;
      for (const t of L.tracks.values()) finalize(t);
      index();
      L.progress = null;
      N.emit('scan');
      N.emit('lib');
      if (todo.length || removed || changed) saveCache();   // changed: a folder's cover picture alone
      measureLater();
      return { added: todo.length, removed, total: L.tracks.size };
    })().finally(() => { L.scanning = null; });
    return L.scanning;
  }

  // songs whose tags gave no length (raw AAC, WebM): ask the browser, one at a time, in the background
  let measuring = false;
  async function measureLater() {
    if (measuring) return;
    measuring = true;
    try {
      const need = [...L.tracks.values()].filter(t => !t.duration && !t.unplayable);
      let changed = 0;
      for (const t of need) {
        const d = await new Promise(res => {
          const a = new Audio();
          a.preload = 'metadata';
          const done = v => { a.removeAttribute('src'); a.load(); res(v); };
          const timer = setTimeout(() => done(0), 8000);
          a.onloadedmetadata = () => { clearTimeout(timer); done(isFinite(a.duration) ? a.duration : 0); };
          a.onerror = () => { clearTimeout(timer); done(-1); };
          a.src = belt.native.mediaUrl(t.path);
        });
        if (d > 0) { t.duration = d; changed++; } else if (d < 0) t.unplayable = true;
      }
      if (changed) { index(); N.emit('lib'); saveCache(); }
    } finally { measuring = false; }
  }

  /* ---------- reading the library ---------- */
  const get = id => L.tracks.get(id);
  const ids = list => list.filter(id => L.tracks.has(id));
  function search(q) {
    const terms = norm(q).split(' ').filter(Boolean);
    if (!terms.length) return { tracks: [], albums: [], artists: [], playlists: [] };
    const hit = s => terms.every(w => s.includes(w));
    const nq = norm(q);
    const score = t => { const T = norm(t.T); return T === nq ? 0 : T.startsWith(nq) ? 1 : T.includes(nq) ? 2 : norm(t.A).includes(nq) ? 3 : 4; };
    const tracks = [...L.tracks.values()].filter(t => shown(t) && hit(t.q)).sort((a, b) => score(a) - score(b) || coll.compare(a.T, b.T)).map(t => t.id);
    const albums = [...L.albums.values()].filter(a => hit(norm(a.name + ' ' + a.artist))).map(a => a.key);
    const artists = [...L.artists.values()].filter(a => a.key && hit(a.key)).map(a => a.key);
    const playlists = (N.S.playlists || []).filter(p => hit(norm(p.name))).map(p => p.id);
    return { tracks, albums, artists, playlists };
  }

  Object.assign(L, { loadCache, saveCache, scan, refresh, index, get, ids, search, finalize, nameTitle, AUDIO, base, dirOf });
})();
