/* String lyrics: for the song playing, from (in order) a .lrc file next to it, the lyrics kept inside the file, and,
   only if you said yes, LRCLIB (lrclib.net, a free open lyrics library: the song's title, artist, album and
   length are sent, nothing else). Synced lyrics (LRC, [mm:ss.xx] times) light up line by line; plain ones just show.
   What's found (or that nothing was) is kept in the data folder's lyrics\, so each song is looked up once.
   The text rules (parseLrc, lineAt, pickBest) are pure and run in Node too: tests\test-lyrics.js. */
(function (root) {
  'use strict';
  const norm = s => String(s || '').normalize('NFKD').replace(/[̀-ͯ]/g, '').toLowerCase()
    .replace(/\s*[([](?:feat|ft|with|prod|remaster|remastered|live|version|edit|mix|official|lyrics?|audio|video|hd|hq)[^)\]]*[)\]]/g, '')
    .replace(/[^\p{L}\p{N}]+/gu, ' ').trim();

  // LRC: [mm:ss.xx]text (several times on one line allowed), [offset:+ms]; <mm:ss.xx> word times and [ar:]-style tags dropped
  function parseLrc(text) {
    const timedLines = [], plain = [];
    let offset = 0, timed = 0, hasUntimedWords = false;
    for (const raw of String(text || '').replace(/\r\n?/g, '\n').split('\n')) {
      const tags = [];
      let rest = raw, m;
      while ((m = rest.match(/^\s*\[([^\]]*)\]/))) { tags.push(m[1]); rest = rest.slice(m[0].length); }
      const words = rest.replace(/<\d{1,3}:\d{1,2}(?:[.:]\d{1,3})?>/g, '').replace(/\s+/g, ' ').trim();
      const times = [];
      for (const tg of tags) {
        const tm = tg.match(/^\s*(\d{1,3}):(\d{1,2})(?:[.:](\d{1,3}))?\s*$/);
        if (tm) { times.push(+tm[1] * 60 + +tm[2] + (tm[3] ? +tm[3] / Math.pow(10, tm[3].length) : 0)); continue; }
        const om = tg.match(/^\s*offset\s*:\s*([+-]?\d+)\s*$/i);
        if (om) offset = +om[1] / 1000;
      }
      if (times.length) { timed++; plain.push(words); for (const t of times) timedLines.push({ t, text: words }); }
      else if (!tags.length || words) { plain.push(words); if (words) hasUntimedWords = true; }
    }
    if (timed >= 2 || (timed === 1 && !hasUntimedWords)) {
      for (const l of timedLines) l.t = Math.max(0, Math.round((l.t - offset) * 1000) / 1000);   // + offset: lyrics come sooner
      timedLines.sort((a, b) => a.t - b.t);
      // drop the blank line(s) a lot of LRC files start with: the intro shows as nothing lit yet
      while (timedLines.length > 1 && !timedLines[0].text) timedLines.shift();
      return { synced: true, lines: timedLines };
    }
    const txt = plain.join('\n').replace(/\n{3,}/g, '\n\n').trim();
    return { synced: false, lines: txt ? txt.split('\n').map(t => ({ t: null, text: t.trim() })) : [] };
  }

  // the line being sung at time t (the last one that has started), or -1 before the first
  function lineAt(lines, t) {
    let lo = 0, hi = lines.length - 1, at = -1;
    while (lo <= hi) { const mid = (lo + hi) >> 1; if (lines[mid].t <= t) { at = mid; lo = mid + 1; } else hi = mid - 1; }
    return at;
  }

  // the best of LRCLIB's search results for a song, or null when none is close enough to trust
  function pickBest(results, { title, artist, duration }) {
    const nt = norm(title), na = norm(artist);
    let best = null, bestScore = -1;
    for (const r of results || []) {
      if (!r || (!r.syncedLyrics && !r.plainLyrics && !r.instrumental)) continue;
      const rt = norm(r.trackName || r.name), ra = norm(r.artistName);
      if (!rt || !(rt === nt || rt.includes(nt) || nt.includes(rt))) continue;
      const dd = duration > 0 && r.duration > 0 ? Math.abs(r.duration - duration) : null;
      if (dd !== null && dd > 4) continue;
      const artistOk = !!na && !!ra && (ra === na || ra.includes(na) || na.includes(ra));
      if (na && !artistOk && dd === null) continue;   // a different artist and no length to vouch for it
      let score = 0;
      if (rt === nt) score += 3;
      if (artistOk) score += 4;
      if (dd !== null) score += 3 - dd * .5;
      if (r.syncedLyrics) score += 2;
      if (score > bestScore) { bestScore = score; best = r; }
    }
    return best;
  }

  const api = { parseLrc, lineAt, pickBest, norm };
  if (typeof module === 'object' && module.exports) { module.exports = api; return; }
  root.StringLyrics = api;

  /* ---------- in the page ---------- */
  const N = root.N;
  let dataDir = '';
  const memo = new Map();   // track id -> Promise of the result, this session
  // LRCLIB runs on donations: be gentle. A song is asked about once (what's found is kept for good); "nothing found" is
  // asked again only after a month; and nothing is asked until a song has been playing a moment (skipping is free)
  const RETRY_NONE = 30 * 864e5;
  const sleep = ms => new Promise(r => setTimeout(r, ms));

  async function cachePath(t) {
    dataDir = dataDir || (await belt.native.paths()).data;
    return `${dataDir}\\lyrics\\${N.art.hash(t.id).toString(16)}-${Math.round(t.duration || 0)}.json`;
  }
  const result = (source, text, extra = {}) => {
    const p = text ? parseLrc(text) : { synced: false, lines: [] };
    return { source, text: text || '', synced: p.synced, lines: p.lines, instrumental: !!extra.instrumental, online: !!extra.online, busy: !!extra.busy };
  };

  async function lrclib(path) {
    const r = await belt.native.lrclib(path);
    if (r.status === 429) { const e = new Error('busy'); e.busy = true; throw e; }
    if (r.status === 404) return null;
    if (r.status !== 200) throw new Error('LRCLIB answered ' + r.status);
    return JSON.parse(r.body);
  }
  async function online(t) {
    const title = t.T, artist = t.A || '', album = t.B || '', duration = Math.round(t.duration || 0);
    const q = o => Object.entries(o).filter(([, v]) => v !== '' && v != null).map(([k, v]) => `${k}=${encodeURIComponent(v)}`).join('&');
    let hit = null;
    if (artist) hit = await lrclib('/api/get?' + q({ track_name: title, artist_name: artist, album_name: album, duration: duration || '' }));
    if (!hit) {
      const list = await lrclib('/api/search?' + (artist ? q({ track_name: title, artist_name: artist }) : q({ q: title })));
      hit = pickBest(list, { title, artist, duration });
    }
    return hit;
  }

  async function find(t, { force = false, settle = null } = {}) {
    const allowOnline = N.S.set.lyricsOnline === true;
    const path = await cachePath(t);
    if (!force) {
      try {
        const c = JSON.parse(await belt.native.readText(path) || 'null');
        if (c && c.v === 1) {
          const r = result(c.source, c.text, c);
          // nothing found, or only unsynced lyrics of the song's own, and LRCLIB never asked (online lyrics were off,
          // or it couldn't be reached): ask it now. "Nothing anywhere" is asked again after a month.
          const neverAsked = allowOnline && !c.online && !c.asked && (c.source === 'none' || ((c.source === 'lrc' || c.source === 'file') && !r.synced));
          const stale = neverAsked || (c.source === 'none' && Date.now() - c.at > RETRY_NONE);
          if (!stale) return r;
        }
      } catch { }
    }
    // 1. a .lrc next to the song   2. the lyrics inside it
    let local = null;
    try {
      const lrc = t.path.replace(/\.[^.\\/]+$/, '') + '.lrc';
      const text = await belt.native.readText(lrc);
      if (text && text.trim()) local = result('lrc', text.trim());
    } catch { }
    if (!local) {
      try {
        const text = await StringTags.readLyrics(N.rangeReader(belt.native.mediaUrl(t.path)), t.size);
        if (text) local = result('file', text);
      } catch { }
    }
    let found = local, asked = false;
    // 3. LRCLIB, when you said yes, and the song has nothing synced of its own
    if ((!local || !local.synced) && allowOnline) {
      // still the song being looked at after a moment? (skipping through songs asks LRCLIB nothing)
      if (settle) {
        await sleep(settle.ms);
        if (!settle.stillWanted()) return { ...(local || result('none', '', { online: false })), skipped: true };
      }
      try {
        const hit = await online(t);
        asked = true;
        if (hit && (hit.syncedLyrics || hit.plainLyrics)) found = result('lrclib', hit.syncedLyrics || hit.plainLyrics, { online: true });
        else if (hit && hit.instrumental) found = result('lrclib', '', { instrumental: true, online: true });
        else if (!local) found = result('none', '', { online: true });
      } catch (e) {
        // offline or busy: nothing is remembered, so LRCLIB is asked again later (the song's own unsynced lyrics show
        // meanwhile, instead of being kept as the answer for good)
        if (!local) return result('none', '', { busy: !!e.busy, online: false });
        return { ...local, retry: true };
      }
    }
    if (!found) found = result('none', '', { online: false });
    try { await belt.native.writeText(path, JSON.stringify({ v: 1, source: found.source, text: found.text, instrumental: found.instrumental, online: found.online, asked, at: Date.now() })); } catch { }
    return found;
  }

  N.lyrics = {
    // a Promise of { source: 'lrc' | 'file' | 'lrclib' | 'none', synced, lines: [{ t, text }], instrumental, online, busy }
    get(t, opts = {}) {
      if (!t) return Promise.resolve(result('none', ''));
      if (opts.force) memo.delete(t.id);
      if (!memo.has(t.id)) {
        const p = find(t, opts).catch(() => result('none', ''));
        memo.set(t.id, p);
        p.then(r => {
          if (r.skipped) { if (memo.get(t.id) === p) memo.delete(t.id); return; }   // skipped past: look again if it comes back
          if (r.busy || r.retry || r.source === 'none') setTimeout(() => { if (memo.get(t.id) === p) memo.delete(t.id); }, 60e3);
        });
      }
      return memo.get(t.id);
    },
    forget(t) { if (t) memo.delete(t.id); },
    lineAt, parseLrc,
  };
})(typeof window !== 'undefined' ? window : globalThis);
