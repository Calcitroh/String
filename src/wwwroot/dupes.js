/* String duplicates: songs the library has more than once, for the Duplicates page (Library → Find duplicates).
   Pure, so it runs in Node too: tests\test-dupes.js. */
(function (root) {
  'use strict';
  const norm = s => String(s || '').normalize('NFKD').replace(/[̀-ͯ]/g, '').toLowerCase()
    .replace(/[^\p{L}\p{N}]+/gu, ' ').trim();
  // "(Official Video)", "[Lyrics]", "【MV】" and the like
  const JUNK = /\s*[([【〔「][^)\]】〕」]*?\b(official|lyrics?|audio|video|visuali[sz]er|mv|m\/v|hd|hq|4k|1080p|720p|amv|music\s+video|full\s+version|clip\s+officiel)\b[^)\]】〕」]*[)\]】〕」]/gi;

  /* songs the library has more than once. Two copies are surely the same song when their titles match (once
     "(Official Video)" and the like are gone) with the same artist, at about the same length (2 s; no length on one
     side is fine). They're maybe the same when the title before any brackets, " - " or "feat." matches at about the
     same length but the artist credit differs or is missing on one side: "Song (From X)" by a singer and "Song" by the
     composer, "Song - Long Ver." of the same length. Each group is an array, best copy first: lossless first, then the
     higher bitrate, then `prefer(t)` (the page: liked, in a playlist, played, in the folder that holds most songs).
     group.sets[i] says which copies are surely the same as each other (equal numbers); group.sure when all are.
     apart(a, b) true: you said those two are not the same song, so no "maybe" link between them */
  const QUALITY = { flac: 5, wav: 4, m4a: 3, opus: 3, ogg: 3, mp3: 2, aac: 2 };
  const plain = s => norm(String(s || '').replace(JUNK, ' '));
  const core = s => norm(String(s || '').replace(/[([{【〔「][^)\]}】〕」]*[)\]}】〕」]/g, ' ').replace(/\s+[-–—|｜]\s+.*$/, '').replace(/\s(feat|ft)\.?\s.*$/i, ''));
  function duplicates(tracks, { prefer = () => 0, apart = () => false } = {}) {
    const near = (a, b) => a.duration > 0 && b.duration > 0 && Math.abs(a.duration - b.duration) <= 2;
    // two union-finds: every link, and sure links only
    const all = new Map(), sure = new Map();
    const bounds = new Map();
    const find = (m, x) => { while (m.get(x) !== x) { m.set(x, m.get(m.get(x))); x = m.get(x); } return x; };
    const join = (m, a, b) => { const ra = find(m, a), rb = find(m, b); if (ra !== rb) m.set(ra, rb); };
    const buckets = new Map();
    for (const t of tracks) {
      all.set(t, t); sure.set(t, t); bounds.set(t, [t.duration, t.duration]);
      const k = core(t.T) || plain(t.T);
      if (!k) continue;
      if (!buckets.has(k)) buckets.set(k, []);
      buckets.get(k).push({ t, p: plain(t.T), a: norm(t.A) });
    }
    for (const list of buckets.values()) {
      if (list.length < 2 || list.length > 200) continue;   // 200 songs of one title: a name like "Intro", not copies
      for (let i = 0; i < list.length; i++) for (let j = i + 1; j < list.length; j++) {
        const x = list[i], y = list[j];
        if (apart(x.t, y.t)) continue;
        const sameCredit = x.p === y.p && x.a === y.a;
        if (!near(x.t, y.t) && !(sameCredit && (!x.t.duration || !y.t.duration))) continue;
        join(all, x.t, y.t);
        if (!sameCredit || !x.a || !near(x.t, y.t)) continue;
        const a = bounds.get(find(sure, x.t)), b = bounds.get(find(sure, y.t));
        const low = Math.min(a[0], b[0]), high = Math.max(a[1], b[1]);
        if (high - low <= 2) { join(sure, x.t, y.t); bounds.set(find(sure, x.t), [low, high]); }
      }
    }
    const groups = new Map();
    for (const t of tracks) { const r = find(all, t); if (!groups.has(r)) groups.set(r, []); groups.get(r).push(t); }
    const rate = t => t.duration > 0 ? (t.size || 0) / t.duration : 0;
    const rank = (a, b) => (QUALITY[b.format] || 1) - (QUALITY[a.format] || 1) || (rate(b) * 0.97 > rate(a) ? 1 : rate(a) * 0.97 > rate(b) ? -1 : 0) ||
      prefer(b) - prefer(a) || (b.size || 0) - (a.size || 0) || String(a.path || a.id).localeCompare(String(b.path || b.id));
    const out = [];
    for (const g of groups.values()) {
      if (g.length < 2) continue;
      g.sort(rank);
      // the copy to keep comes first: the best of the biggest set of sure copies
      const roots = g.map(t => find(sure, t)), size = new Map();
      roots.forEach(r => size.set(r, (size.get(r) || 0) + 1));
      let k = 0;
      roots.forEach((r, i) => { if (size.get(r) > size.get(roots[k])) k = i; });
      if (k) { g.unshift(g.splice(k, 1)[0]); roots.unshift(roots.splice(k, 1)[0]); }
      const ids = new Map();
      g.sets = roots.map(r => { if (!ids.has(r)) ids.set(r, ids.size); return ids.get(r); });
      g.sure = ids.size === 1;
      out.push(g);
    }
    // sure groups first, then by title
    return out.sort((a, b) => (b.sure - a.sure) || norm(a[0].T).localeCompare(norm(b[0].T)));
  }

  const api = { duplicates, norm };
  if (typeof module === 'object' && module.exports) { module.exports = api; return; }
  root.N = root.N || {};
  root.N.dupes = api;
})(typeof window !== 'undefined' ? window : globalThis);
