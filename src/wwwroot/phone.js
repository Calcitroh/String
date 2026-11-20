/* Phone listening in Stats. A phone can play the library through Navidrome on this PC, which keeps its own
   history of every song played (its `scrobbles` table). The host reads that history, read-only and without a login
   (belt.native.navidromePlays), and each new play is counted in Stats as the whole song, at the time it was played,
   marked as phone time. Navidrome only hears about a song once it's been played about halfway, so skips don't count.
     S.phone = { after: the last Navidrome play number counted, plays: how many counted, found, at: last check, error }
     S.set.phoneStats === false turns it off. Runs a moment after start, every 5 minutes, and when String comes back. */
(function () {
  'use strict';
  const N = window.N;
  let busy = null, smoke = false, users = {};
  belt.ready.then(h => { smoke = !!(h && h.smoke); });
  async function run() {
    const S = N.S, L = N.lib;
    const st = S.phone = S.phone || { after: 0, plays: 0 };
    const doc = N.stats.model.doc;
    // Persist the import cursor with the listening seconds it describes. Older
    // installations already counted all plays up to their single saved cursor.
    const history = doc.phoneImport || { legacyAfter: st.after || 0, after: {}, plays: st.plays || 0 };
    let added = 0;
    try {
      for (let round = 0; round < 40; round++) {
        const selected = S.set.phoneUser || '';
        const res = await belt.native.navidromePlays(history.after[selected] ?? history.legacyAfter, selected);
        if (selected !== (S.set.phoneUser || '') || S.set.phoneStats === false) break;
        st.found = !!res.found;
        users = res.users || {};
        if (!res.found || !res.user) break;
        S.set.phoneUser = res.user;
        st.after = history.after[res.user] ?? history.legacyAfter;
        st.plays = history.plays;
        if (!res.plays.length) break;
        for (const p of res.plays) {
          if (p.n <= st.after) continue;
          const id = String(p.path || '').toLowerCase(), t = L.get(id);
          const track = t ? { id: t.id, title: t.T, artist: t.A || '' } : { id, title: p.title || '', artist: p.artist || '' };
          const secs = (t && t.duration) || p.duration;
          if (N.stats.addPhone(track, secs, new Date(p.at * 1000))) { st.plays = (st.plays || 0) + 1; added++; }
          st.after = Math.max(st.after || 0, p.n);
        }
        history.after[res.user] = st.after;
        history.plays = st.plays;
        doc.phoneImport = history;
        N.stats.saveNow();
        if (res.plays.length < 5000) break;
      }
      st.error = '';
    } catch (e) { st.error = String((e && e.message) || e); }
    st.at = Date.now();
    N.save();
    N.emit('phone', added);
    return added;
  }
  // auto: the timers' and window's calls, which a self-test leaves to the test itself
  function sync(auto) {
    if (!N.stats.loaded || (auto && smoke) || N.S.set.phoneStats === false || !belt.native.navidromePlays) return Promise.resolve(0);
    if (!busy) busy = run().finally(() => { busy = null; });
    return busy;
  }
  async function choose(user) {
    N.S.set.phoneUser = user;
    N.save();
    if (busy) await busy;
    return sync();
  }
  N.phone = { sync, choose, get users() { return users; } };
})();
