/* String stats: how long you actually listened (seconds of music heard, not plays), per day and hour, per
   artist and per song, kept in listening.json in the data folder (with daily backups, like state.json).
     { v: 1, tracks: [[id, title, artist]], artists: [name], days: { 'YYYY-MM-DD': { s, h: [24], a: { i: s }, t: { i: s }, p } } }
   (p: the part of that day's seconds heard on the phone, through Navidrome: whole songs, from its play history)
   The model (StringStats.create) is pure and runs in Node too: tests\test-stats.js checks its date maths.
   In the page, N.stats records from the player (a timeupdate tick adds the seconds just heard) and answers the Stats
   page: totals for today / this week / this month / this year / all time, the previous period, the chart's units,
   top artists and songs, and the day streak. */
(function (root) {
  'use strict';
  const pad = n => String(n).padStart(2, '0');
  const dayKey = d => `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
  const fromKey = k => { const [y, m, d] = k.split('-').map(Number); return new Date(y, m - 1, d); };
  const addDays = (d, n) => new Date(d.getFullYear(), d.getMonth(), d.getDate() + n);
  const startOfDay = d => new Date(d.getFullYear(), d.getMonth(), d.getDate());
  const round1 = x => Math.round(x * 10) / 10;

  // weekStart: 0 = Sunday … 6 = Saturday
  function create(doc, { weekStart = 1 } = {}) {
    if (!doc || doc.v !== 1 || typeof doc.days !== 'object') doc = { v: 1, tracks: [], artists: [], days: {} };
    if (!Array.isArray(doc.tracks)) doc.tracks = [];
    if (!Array.isArray(doc.artists)) doc.artists = [];
    const tIndex = new Map(doc.tracks.map((x, i) => [x[0], i]));
    const aIndex = new Map(doc.artists.map((x, i) => [x, i]));

    function add(track, secs, when = new Date()) {
      if (!track || !(secs > 0) || secs > 30) return false;
      return put(track, secs, when);
    }
    // a song played on the phone (Navidrome's history): the whole song at once, at the time it was played
    function addPhone(track, secs, when) {
      if (!track || !(secs > 0) || secs > 4 * 3600 || !(when instanceof Date) || isNaN(when)) return false;
      if (!put(track, secs, when)) return false;
      const day = doc.days[dayKey(when)];
      day.p = (day.p || 0) + secs;
      return true;
    }
    function put(track, secs, when) {
      const k = dayKey(when);
      let day = doc.days[k];
      if (!day) day = doc.days[k] = { s: 0, h: new Array(24).fill(0), a: {}, t: {} };
      let ti = tIndex.get(track.id);
      if (ti === undefined) { ti = doc.tracks.length; doc.tracks.push([track.id, track.title || '', track.artist || '']); tIndex.set(track.id, ti); }
      else { const row = doc.tracks[ti]; row[1] = track.title || row[1]; row[2] = track.artist || row[2]; }
      const name = track.artist || '';
      let ai = aIndex.get(name);
      if (ai === undefined) { ai = doc.artists.length; doc.artists.push(name); aIndex.set(name, ai); }
      day.s += secs;
      day.h[when.getHours()] = (day.h[when.getHours()] || 0) + secs;
      day.a[ai] = (day.a[ai] || 0) + secs;
      day.t[ti] = (day.t[ti] || 0) + secs;
      return true;
    }

    // a duplicate copy dropped for another (the Duplicates page): its listening time becomes the kept copy's
    function merge(fromId, toId) {
      const fi = tIndex.get(fromId);
      if (fi === undefined || fromId === toId) return false;
      const ti = tIndex.get(toId);
      if (ti === undefined) { doc.tracks[fi][0] = toId; tIndex.delete(fromId); tIndex.set(toId, fi); return true; }
      for (const day of Object.values(doc.days)) {
        const v = day.t[fi];
        if (v) { day.t[ti] = (day.t[ti] || 0) + v; delete day.t[fi]; }
      }
      return true;
    }

    // totals and rankings for the days from..to (Dates, both included)
    function range(from, to) {
      let total = 0, phone = 0;
      const byArtist = new Map(), byTrack = new Map();
      for (let d = startOfDay(from); d <= to; d = addDays(d, 1)) {
        const day = doc.days[dayKey(d)];
        if (!day) continue;
        total += day.s;
        phone += day.p || 0;
        for (const [i, s] of Object.entries(day.a)) byArtist.set(+i, (byArtist.get(+i) || 0) + s);
        for (const [i, s] of Object.entries(day.t)) byTrack.set(+i, (byTrack.get(+i) || 0) + s);
      }
      const artists = [...byArtist.entries()].sort((x, y) => y[1] - x[1]).map(([i, s]) => ({ name: doc.artists[i], secs: round1(s) }));
      const tracks = [...byTrack.entries()].sort((x, y) => y[1] - x[1]).map(([i, s]) => ({ id: doc.tracks[i][0], title: doc.tracks[i][1], artist: doc.tracks[i][2], secs: round1(s) }));
      return { total: round1(total), phone: round1(phone), artists, tracks };
    }

    function firstDay() {
      const keys = Object.keys(doc.days).filter(k => doc.days[k].s > 0).sort();
      return keys.length ? fromKey(keys[0]) : null;
    }

    // a period and the one before it: { from, to, prevFrom, prevTo, units: [{ label, long, from, to, hour? }] }
    function period(kind, now = new Date()) {
      const today = startOfDay(now);
      if (kind === 'day') {
        const units = Array.from({ length: 24 }, (_, h) => ({ hour: h, label: h % 6 === 0 ? (h === 0 ? '12a' : h === 12 ? '12p' : h < 12 ? h + 'a' : (h - 12) + 'p') : '', long: new Date(2000, 0, 1, h).toLocaleTimeString([], { hour: 'numeric' }), from: today, to: today }));
        return { kind, from: today, to: today, prevFrom: addDays(today, -1), prevTo: addDays(today, -1), units, current: now.getHours() };
      }
      if (kind === 'week') {
        const back = (today.getDay() - weekStart + 7) % 7, from = addDays(today, -back);
        const units = Array.from({ length: 7 }, (_, i) => { const d = addDays(from, i); return { label: d.toLocaleDateString([], { weekday: 'short' }), long: d.toLocaleDateString([], { weekday: 'long', month: 'short', day: 'numeric' }), from: d, to: d }; });
        return { kind, from, to: addDays(from, 6), prevFrom: addDays(from, -7), prevTo: addDays(from, -1), units, current: back };
      }
      if (kind === 'month') {
        const from = new Date(today.getFullYear(), today.getMonth(), 1), to = new Date(today.getFullYear(), today.getMonth() + 1, 0);
        const units = Array.from({ length: to.getDate() }, (_, i) => { const d = addDays(from, i); return { label: (i + 1) % 5 === 0 || i === 0 ? String(i + 1) : '', long: d.toLocaleDateString([], { weekday: 'short', month: 'short', day: 'numeric' }), from: d, to: d }; });
        return { kind, from, to, prevFrom: new Date(from.getFullYear(), from.getMonth() - 1, 1), prevTo: addDays(from, -1), units, current: today.getDate() - 1 };
      }
      if (kind === 'year') {
        const from = new Date(today.getFullYear(), 0, 1), to = new Date(today.getFullYear(), 11, 31);
        const units = Array.from({ length: 12 }, (_, m) => { const a = new Date(today.getFullYear(), m, 1); return { label: a.toLocaleDateString([], { month: 'narrow' }), long: a.toLocaleDateString([], { month: 'long', year: 'numeric' }), from: a, to: new Date(today.getFullYear(), m + 1, 0) }; });
        return { kind, from, to, prevFrom: new Date(from.getFullYear() - 1, 0, 1), prevTo: new Date(from.getFullYear() - 1, 11, 31), units, current: today.getMonth() };
      }
      // all time: by month, from the first day with music (at most the last 36 months)
      const first = firstDay() || today;
      let m0 = new Date(first.getFullYear(), first.getMonth(), 1);
      const last = new Date(today.getFullYear(), today.getMonth(), 1);
      const months = (last.getFullYear() - m0.getFullYear()) * 12 + last.getMonth() - m0.getMonth() + 1;
      if (months > 36) m0 = new Date(last.getFullYear(), last.getMonth() - 35, 1);
      const n = Math.min(36, months);
      const units = Array.from({ length: n }, (_, i) => { const a = new Date(m0.getFullYear(), m0.getMonth() + i, 1); return { label: a.getMonth() === 0 || i === 0 ? a.toLocaleDateString([], { month: 'short', year: '2-digit' }) : a.toLocaleDateString([], { month: 'narrow' }), long: a.toLocaleDateString([], { month: 'long', year: 'numeric' }), from: a, to: new Date(a.getFullYear(), a.getMonth() + 1, 0) }; });
      return { kind, from: first, to: today, prevFrom: null, prevTo: null, units, current: n - 1 };
    }

    // seconds in each unit of a period (hours of a day, days of a week or month, months of a year)
    function series(p) {
      if (p.kind === 'day') { const day = doc.days[dayKey(p.from)]; return p.units.map(u => round1(day ? day.h[u.hour] || 0 : 0)); }
      return p.units.map(u => range(u.from, u.to).total);
    }

    // days in a row with at least a minute of music, ending today (or yesterday, if today hasn't started yet)
    function streak(now = new Date()) {
      let d = startOfDay(now), n = 0;
      const heard = x => { const day = doc.days[dayKey(x)]; return !!day && day.s >= 60; };
      if (!heard(d)) d = addDays(d, -1);
      while (heard(d)) { n++; d = addDays(d, -1); }
      return n;
    }

    return { doc, add, addPhone, merge, range, period, series, streak, firstDay, dayKey };
  }

  const calendarDay = date => Date.UTC(date.getFullYear(), date.getMonth(), date.getDate()) / 864e5;
  const api = { create, dayKey, fromKey, addDays, startOfDay, calendarDay };
  if (typeof module === 'object' && module.exports) { module.exports = api; return; }
  root.StringStats = api;

  /* ---------- in the page ---------- */
  const N = root.N = root.N || {};
  const loc = (() => { try { return new Intl.Locale(navigator.language || 'en-US'); } catch { return null; } })();
  const wi = loc && (typeof loc.getWeekInfo === 'function' ? loc.getWeekInfo() : loc.weekInfo);
  const weekStart = wi && wi.firstDay ? wi.firstDay % 7 : 1;   // Intl: 1 = Monday … 7 = Sunday; ours: 0 = Sunday
  let model = create(null, { weekStart }), loaded = false;

  N.stats = {
    get model() { return model; },
    get loaded() { return loaded; },
    weekStart,
    load(doc) {
      model = create(doc, { weekStart });
      loaded = true;
      N.emit && N.emit('stats');
    },
    // the player calls this with the seconds of music just heard
    add(t, secs) {
      if (!loaded || !t) return;
      if (model.add({ id: t.id, title: t.T, artist: t.A || '' }, secs)) {
        belt.store('listening', model.doc, 20000);
        tick();
      }
    },
    saveNow() { if (loaded) belt.store('listening', model.doc, 0); },
    // a song played on the phone, from Navidrome's history (N.phone): the whole song, at the time it was played
    addPhone(track, secs, when) {
      if (!loaded || !track) return false;
      if (!model.addPhone(track, secs, when)) return false;
      belt.store('listening', model.doc, 3000);
      tick();
      return true;
    },
    // a duplicate's listening time goes to the copy that's kept
    merge(fromId, toId) { if (loaded && model.merge(fromId, toId)) { belt.store('listening', model.doc, 2000); tick(); } },
  };
  // the screens hear about it every few seconds, not every tick
  let tickTimer = 0;
  function tick() { if (!tickTimer) tickTimer = setTimeout(() => { tickTimer = 0; N.emit && N.emit('stats'); }, 4000); }
})(typeof window !== 'undefined' ? window : globalThis);
