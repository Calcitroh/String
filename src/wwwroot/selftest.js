/* String self-test, run by String's own window (see tests\README.md):
     node tests\test-tags.js        (makes the test songs in test-runs\string-media, checks the tag reader)
     app-new\String.exe --smoke-test --data <dir>\data --out <dir>\out --test-media test-runs\string-media
   It adds the test songs as the library, plays through them silently (muted) and checks the host's /_media/ address,
   the tags, albums, playing, seeking, next/previous, the gapless hand-over, crossfade, repeat, the queue, shuffle,
   likes, playlists, Edit info, search, Now Playing's visualizer, listening stats, presence.json, the library cache and
   the Discord helper. */
(function () {
  'use strict';
  window.smokeState = { step: 'loading', done: false, log: [] };
  window.smokeTest = async () => {
    const st = window.smokeState, N = window.N, L = N.lib, P = N.player, U = N.ui;
    const log = (k, v) => st.log.push(`${k}: ${typeof v === 'string' ? v : JSON.stringify(v)}`);
    let pass = 0, fail = 0;
    const check = (name, ok, extra) => { if (ok) pass++; else fail++; log((ok ? 'ok   ' : 'FAIL ') + name, extra === undefined ? '' : extra); };
    const sleep = ms => new Promise(r => setTimeout(r, ms));
    const waitFor = async (fn, ms = 8000) => { const end = Date.now() + ms; while (Date.now() < end) { try { if (fn()) return true; } catch { } await sleep(100); } return false; };
    const step = async (name, ms = 700) => { st.step = name; await sleep(ms); };
    const title = () => (P.current() || {}).T;
    const minimise = async on => {
      await belt.native.smokeMinimise(on);
      check(on ? 'WebView hidden on minimise' : 'WebView visible on restore',
        await waitFor(() => belt.visible === !on && document.hidden === on));
    };
    try {
      await N.boot;
      const paths = await belt.native.paths();
      const media = paths.testMedia;
      if (!media) throw new Error('start the test with --test-media <folder with the test songs>');
      const lower = p => (media + '\\' + p).toLowerCase();

      /* the host's seekable media address */
      check('features().media', (await belt.native.features()).media === true);
      const r = await fetch(belt.native.mediaUrl(media + '\\glass orbit.mp3'), { headers: { Range: 'bytes=0-9' } });
      const b = new Uint8Array(await r.arrayBuffer());
      check('Range gives 206 and the bytes asked for', r.status === 206 && b.length === 10 && String.fromCharCode(b[0], b[1], b[2]) === 'ID3', { status: r.status, len: b.length, range: r.headers.get('content-range') });
      // ranges that run to the end of the file must be that part of the file, not its start
      const tail = await fetch(belt.native.mediaUrl(media + '\\glass orbit.mp3'), { headers: { Range: 'bytes=-128' } });
      const tb = new Uint8Array(await tail.arrayBuffer());
      check('suffix Range is the end of the file', tail.status === 206 && tb.length === 128 && String.fromCharCode(tb[0], tb[1], tb[2]) !== 'ID3', { status: tail.status, len: tb.length, first: String.fromCharCode(tb[0], tb[1], tb[2]) });
      const mid = await fetch(belt.native.mediaUrl(media + '\\glass orbit.mp3'), { headers: { Range: 'bytes=4000-' } });
      const whole = new Uint8Array(await (await fetch(belt.native.mediaUrl(media + '\\glass orbit.mp3'))).arrayBuffer());
      const mb = new Uint8Array(await mid.arrayBuffer());
      check('open Range is the rest of the file', mid.status === 206 && mb.length === whole.length - 4000 && mb[0] === whole[4000] && mb[1] === whole[4001] && mb[500] === whole[4500], { status: mid.status, len: mb.length, whole: whole.length });
      check('a missing file is 404', (await fetch(belt.native.mediaUrl(media + '\\nope.mp3'))).status === 404);
      // a number too big for the host is a range it can't give (416), and the file is free again afterwards
      const huge = await fetch(belt.native.mediaUrl(media + '\\glass orbit.mp3'), { headers: { Range: 'bytes=99999999999999999999-' } });
      check('a Range number too big is 416, not an error', huge.status === 416, huge.status);
      check('a non-media file is 404', (await fetch(belt.native.mediaUrl(paths.tool + '\\wwwroot\\string.js'))).status === 404);

      await step('welcome', 900);
      check('welcome on an empty library', !!document.querySelector('.welcome'));

      /* the library */
      N.S.muted = true; P.volumeChanged();   // the test plays real songs: keep the room quiet
      U.addRoots([media]);
      await sleep(300);
      await waitFor(() => !L.scanning && L.tracks.size >= 8, 20000);
      check('8 songs found', L.tracks.size === 8, L.tracks.size);
      check('3 albums', L.albums.size === 3, [...L.albums.values()].map(a => `${a.name} / ${a.artist}`));
      const sw = [...L.albums.values()].find(a => a.name === 'Synthetic Weather');
      check('album in track order', !!sw && sw.ids.map(id => L.get(id).T).join('|') === 'Glass Orbit|Low Tide|Northern Static', sw && sw.ids.map(id => L.get(id).T));
      const notags = L.get(lower('Loose\\098. no tags at all.mp3')), loose = L.get(lower('Loose\\Downloaded Thing [Ab12Cd34Ef5].mp3'));
      const ogg = L.get(lower('Album One\\01 ogg song.ogg'));
      check('title from the file name', !!notags && notags.T === 'no tags at all', notags && notags.T);
      check('folder cover found', !!ogg && /cover\.jpg$/i.test(ogg.cover || ''), ogg && ogg.cover);
      check('lengths read', [...L.tracks.values()].every(t => t.duration > 7), [...L.tracks.values()].map(t => Math.round(t.duration)));

      U.go('home');
      await waitFor(() => document.querySelectorAll('#view .art img.in').length > 0, 5000);
      await step('home', 1200);
      check('covers shown', document.querySelectorAll('#view .art img.in').length > 0, document.querySelectorAll('#view .art img.in').length);
      const pictures = await Promise.all(['glass orbit.mp3', 'low tide.flac', 'Album One\\01 ogg song.ogg'].map(p => N.art.url(L.get(lower(p)), 'sm')));
      check('identical embedded covers share a thumbnail and a different cover keeps its own', pictures.every(Boolean) && pictures[0] === pictures[1] && pictures[1] !== pictures[2]);
      U.go('songs'); await step('songs', 1100);
      check('8 song rows', document.querySelectorAll('#view .tr').length === 8, document.querySelectorAll('#view .tr').length);
      U.go('albums'); await step('albums', 1000);
      U.go('artists'); await step('artists', 900);
      U.go('album/' + encodeURIComponent(sw.key)); await step('album', 1300);

      /* playing: first with no click yet (a media key right after String opens), which must play straight, not silently */
      U.playCtx({ kind: 'album', key: sw.key });
      const early = await waitFor(() => P.playing && P.time() > .4, 8000);
      // (WebView2 lets Web Audio run without a click, so it may already be routed; it must never be routed while stopped)
      check('plays before any click, never through a stopped mixer', early && (!P.audio().routed || P.audio().state === 'running'), { t: P.time(), audio: P.audio() });
      P.pause(); await sleep(300);
      // then a real click on the big play button, as a person would: the sound now goes through the mixer
      const pb = document.querySelector('#view .actions .play-big').getBoundingClientRect();
      await belt.native.smokeClick(pb.left + pb.width / 2, pb.top + pb.height / 2);
      const played = await waitFor(() => P.playing && P.time() > .6 && P.audio().routed, 8000);
      check('a click plays, through the mixer', played, { t: P.time(), title: title(), playing: P.playing, audio: P.audio() });
      await step('playing', 900);
      const nowRow = document.querySelector('#view .tr.now');
      check('playing row marked, laid out like the others', !!nowRow && nowRow.offsetHeight === 52 && nowRow.querySelector('.c-title .art').offsetWidth === 38, nowRow && [nowRow.offsetHeight, nowRow.querySelector('.c-title .art').offsetWidth]);
      check('visualizer hears the music even when muted', N.viz.level() > .01, N.viz.level());
      P.seek(10); await sleep(700);
      check('seek', P.time() >= 10 && P.time() < 12.5, P.time());
      P.next(); await waitFor(() => title() === 'Low Tide' && P.time() > .3, 6000);
      check('next', title() === 'Low Tide', title());
      P.prev(); await sleep(400);
      check('previous', title() === 'Glass Orbit', title());
      P.seek(P.duration() - 2.5);
      await waitFor(() => title() === 'Low Tide' && P.time() > .2, 9000);
      check('hands over to the next song by itself', title() === 'Low Tide' && P.playing, { title: title(), playing: P.playing });

      N.S.set.fade = 3;
      P.seek(P.duration() - 5);
      await waitFor(() => P.crossfading(), 7000);
      check('crossfade starts', P.crossfading() && title() === 'Northern Static', title());
      await step('crossfade', 400);
      await waitFor(() => !P.crossfading(), 8000);
      check('crossfade ends', !P.crossfading() && P.playing, title());
      N.S.set.fade = 0;

      /* repeat one: the song starts over by itself, and presence.json (Discord's time bar) starts over with it */
      const pdir = (await belt.native.paths()).data;
      N.S.queue.repeat = 'one';
      const loopTitle = title();
      P.seek(P.duration() - 1.5);
      const looped = await waitFor(() => P.time() < 1, 8000);
      await sleep(1600);
      const pr = JSON.parse(await belt.native.readText(pdir + '\\presence.json') || '{}');
      check('repeat one plays the same song again', !!looped && title() === loopTitle && P.playing, { title: title(), t: P.time() });
      check('repeat one gives Discord a fresh time bar', pr.state === 'playing' && pr.title === loopTitle && pr.endsAt > Date.now() + 3000 && Math.abs(pr.startedAt - (Date.now() - P.time() * 1000)) < 2500, { state: pr.state, endsIn: pr.endsAt - Date.now(), t: P.time() });
      N.S.queue.repeat = 'off';

      /* the queue */
      P.addToQueue([ogg.id]); P.playNext([notags.id]);
      let up = P.upcoming().map(x => L.get(x.id).T);
      check('play next comes before add to queue', up[0] === 'no tags at all' && up[1] === 'Ogg Song', up);
      P.setShuffle(true);
      up = P.upcoming().map(x => L.get(x.id).T);
      check('shuffle keeps queued songs first', up[0] === 'no tags at all' && up[1] === 'Ogg Song', up);
      P.setShuffle(false);
      U.openQueue(); await step('queue', 1000); U.closeQueue();
      P.cycleRepeat(); check('repeat all', N.S.queue.repeat === 'all');
      P.cycleRepeat(); check('repeat one', N.S.queue.repeat === 'one');
      P.cycleRepeat(); check('repeat off', N.S.queue.repeat === 'off');

      /* likes, playlists, Edit info, search */
      U.toggleLike([ogg.id]);
      check('liked', !!N.S.liked[ogg.id]);
      const p = { id: 'smoke', name: 'Smoke mix', items: [], created: Date.now(), updated: Date.now() };
      N.S.playlists.push(p);
      U.addToPlaylist(p, [ogg.id, loose.id]);
      U.addToPlaylist(p, [ogg.id]);
      check('playlist adds without doubles', p.items.length === 2, p.items.length);
      U.go('pl/smoke'); await step('playlist', 1200);
      U.removeFromPlaylist('smoke', [0]);
      check('playlist remove', p.items.length === 1 && p.items[0].id === loose.id);
      N.S.edits[loose.id] = { title: 'Renamed Thing', artist: 'Smoke Artist' };
      L.refresh();
      check('Edit info renames', L.get(loose.id).T === 'Renamed Thing' && L.artists.has('smoke artist'));
      U.go('liked'); await step('liked', 900);
      const q = document.querySelector('#q');
      q.value = 'orbit'; q.dispatchEvent(new Event('input'));
      await waitFor(() => U.route() === 'search', 2000);
      await step('search', 900);
      check('search', L.search('orbit').tracks.length === 1 && document.querySelectorAll('#view .tr').length === 1);
      q.value = '';

      /* Now Playing: the record and the ring */
      const f0 = N.viz.frames;
      U.openNP(); await step('now-playing', 2400);
      check('visualizer draws while playing', N.viz.frames - f0 > 20, N.viz.frames - f0);
      const cv = document.getElementById('bigviz'), px = cv.getContext('2d').getImageData(0, 0, cv.width, cv.height).data;
      let lit = 0; for (let i = 3; i < px.length; i += 16) if (px[i] > 0) lit++;
      check('ring drawn', lit > 100, lit);
      const ambient = document.getElementById('npAmb');
      const motion = el => getComputedStyle(el).animationPlayState;
      P.pause();
      const coastFrom = N.viz.frames;
      await sleep(300);
      check('the record coasts to a stop after pause, not dead still', N.viz.running() && N.viz.frames > coastFrom + 3);
      await waitFor(() => !N.viz.running());
      const still = N.viz.frames;
      await sleep(350);
      check('paused record and background stay still', still === N.viz.frames && motion(ambient) === 'paused');
      check('paused mixer sleeps after the fade', await waitFor(() => P.audio().state === 'suspended', 1200));
      await P.play();
      check('record, mixer and background resume', await waitFor(() => N.viz.frames > still + 3 && P.audio().state === 'running' && N.viz.level() > .01) && motion(ambient) === 'running');
      await minimise(true);
      try {
        const hiddenFrame = N.viz.frames;
        const wire = document.querySelector('.string-logo .wire');
        await sleep(300);
        check('hidden record and wire stay still', !N.viz.running() && hiddenFrame === N.viz.frames && motion(wire) === 'paused');
      } finally { await minimise(false); }
      check('restoring resumes the record and wire', await waitFor(() => N.viz.running()) && motion(document.querySelector('.string-logo .wire')) === 'running');
      U.closeNP();

      /* lyrics: inside the file (Low Tide, synced), in a .lrc next to it (Plain Wave), and online (Glass Orbit, from the
         fake LRCLIB this test runs with: never the real one), asked only after a yes */
      const lowTide = L.get(lower('low tide.flac')), glass = L.get(lower('glass orbit.mp3')), wave = L.get(lower('Loose\\Some Artist - Plain Wave.wav'));
      const lyLines = () => [...document.querySelectorAll('#npLyrics .ll')].map(x => x.textContent);
      const lySrc = () => (document.querySelector('#npLyrics .ly-src') || {}).textContent || '';
      N.S.set.lyricsOnline = null;
      P.playIds([lowTide.id], 0, null);
      await waitFor(() => title() === 'Low Tide' && P.playing, 5000);
      U.openNP('lyrics');
      await waitFor(() => lyLines().length > 0, 6000);
      check('lyrics from inside the song, synced', /song file/.test(lySrc()) && lyLines().length === 5 && !!document.querySelector('#npLyrics .ly-scroll.synced'), { src: lySrc(), lines: lyLines() });
      P.seek(5.3);
      await waitFor(() => (document.querySelector('#npLyrics .ll.on') || {}).textContent === 'Third line at five', 3000);
      check('the line being sung is lit', (document.querySelector('#npLyrics .ll.on') || {}).textContent === 'Third line at five' && document.querySelectorAll('#npLyrics .ll.past').length === 2, (document.querySelector('#npLyrics .ll.on') || {}).textContent);
      await step('lyrics', 1300);
      document.querySelectorAll('#npLyrics .ll')[1].click();
      await sleep(400);
      check('clicking a line goes there', P.time() >= 2.9 && P.time() < 4.2, P.time());
      P.playIds([wave.id], 0, null);
      await waitFor(() => title() === wave.T && lyLines().length > 0, 6000);
      check('lyrics from a .lrc next to the song', /\.lrc/.test(lySrc()) && lyLines()[0] === 'Sidecar line one', { src: lySrc(), lines: lyLines() });
      P.playIds([glass.id], 0, null);
      await waitFor(() => title() === 'Glass Orbit' && !!document.querySelector('#npLyrics [data-ly="yes"]'), 6000);
      check('no lyrics of its own: asks before going online', !!document.querySelector('#npLyrics [data-ly="yes"]'));
      await step('lyrics-ask', 1000);
      document.querySelector('#npLyrics [data-ly="yes"]').click();
      await waitFor(() => lyLines().length > 0, 8000);
      check('a yes finds them on LRCLIB', N.S.set.lyricsOnline === true && /LRCLIB/.test(lySrc()) && lyLines()[0] === 'Glass orbit online one', { src: lySrc(), lines: lyLines() });
      const seenAsks = JSON.parse((await belt.native.lrclib('/api/_seen')).body || '[]');
      check('LRCLIB is told who is asking, and only the song', seenAsks.length > 0 && seenAsks.every(s => /^String v1\.0/.test(s.ua) && Object.keys(s.query).every(k => ['track_name', 'artist_name', 'album_name', 'duration', 'q'].includes(k))), seenAsks);
      await step('lyrics-online', 1300);
      const cachedAsks = seenAsks.length;
      N.lyrics.forget(glass); P.playIds([lowTide.id], 0, null); await sleep(500); P.playIds([glass.id], 0, null);
      await waitFor(() => title() === 'Glass Orbit' && lyLines()[0] === 'Glass orbit online one', 6000);
      const seenAfter = JSON.parse((await belt.native.lrclib('/api/_seen')).body || '[]');
      check('found lyrics are kept: no second lookup', seenAfter.length === cachedAsks, [cachedAsks, seenAfter.length]);
      // being gentle with LRCLIB: a song skipped past within a moment is never asked about
      P.playIds([notags.id], 0, null); await sleep(350); P.playIds([ogg.id], 0, null);
      await sleep(2800);
      const seenSkip = JSON.parse((await belt.native.lrclib('/api/_seen')).body || '[]'), askedFor = s => JSON.stringify(s.query).toLowerCase();
      check('a song skipped past is not looked up; the one that stays is', !seenSkip.some(s => askedFor(s).includes('no tags at all')) && seenSkip.some(s => askedFor(s).includes('ogg song')), seenSkip.map(s => s.query));

      /* translation: a model on this PC (a fake Ollama in this test) writes each line's translation under it, live */
      const sent = [], realChat = belt.native.ollamaChat;
      check('only verified local models are selectable', (await N.translate.models()).join() === 'fake-translator:1b');
      for (const model of ['cloud-alias', 'unverified-model', 'fixture:cloud']) {
        let refused = false;
        try { await realChat({ model, messages: [] }, {}); } catch (e) { refused = /local model/i.test(e.message); }
        check('translation refuses ' + model + ' before sending a chat', refused);
      }
      belt.native.ollamaChat = (req, o) => { sent.push(req); return realChat(req, o); };
      const trTexts = () => [...document.querySelectorAll('#npLyrics .ll .ll-tr')].map(x => x.textContent);
      Object.assign(N.S.set, { translate: false, trLang: 'English', trModel: '' });
      P.playIds([lowTide.id], 0, null);
      await waitFor(() => title() === 'Low Tide' && lyLines().length === 5 && !!document.querySelector('#npLyrics .ly-tbtn'), 6000);
      document.querySelector('#npLyrics .ly-tbtn').click();
      const translated = await waitFor(() => trTexts().length === 5 && trTexts().every(x => x.startsWith('[EN] ')), 8000);
      check('Translate writes each line\'s translation under it', translated && trTexts()[2] === '[EN] Third line at five', trTexts());
      await waitFor(() => /^Translated/.test((document.getElementById('lyStatus') || {}).textContent || ''), 4000);
      const stat = (document.getElementById('lyStatus') || {}).textContent || '';
      check('says which model, on this PC', /Translated into English by fake-translator:1b, on this PC/.test(stat), stat);
      const inside = [...document.querySelectorAll('#npLyrics .ll')].every(l => { const s = l.querySelector('.ll-tr'); if (!s) return false; const a = s.getBoundingClientRect(), b = l.getBoundingClientRect(); return a.height > 0 && a.top >= b.top - 1 && a.bottom <= b.bottom + 1; });
      check('every translation shows, under its own line', inside);
      const rq = sent[0] || {};
      check('the model gets only the song and its lines', rq.model === 'fake-translator:1b' && rq.stream === true && rq.keep_alive && rq.think === undefined &&
        rq.messages[1].content === 'Song: "Low Tide" by Test Signal\n\n1| Low tide line one\n2| Low tide line two\n3| Third line at five\n4| Fourth line at eight\n5| Last line at twelve', rq.messages && rq.messages[1].content);
      P.seek(5.3); await sleep(600);
      await step('lyrics-translated', 1300);
      document.querySelector('#npLyrics .ly-tbtn').click();
      check('Translate off hides them', !document.getElementById('npLyrics').classList.contains('show-tr') && N.S.set.translate === false);
      document.querySelector('#npLyrics .ly-tbtn').click();
      await waitFor(() => trTexts().length === 5 && trTexts().every(x => x.startsWith('[EN] ')), 4000);
      check('a translation is kept: no second request', sent.length === 1, sent.length);
      P.playIds([wave.id], 0, null);
      check('the next song is translated by itself while Translate is on', await waitFor(() => title() === wave.T && trTexts()[0] === '[EN] Sidecar line one', 8000), trTexts());
      N.S.set.translate = false;
      belt.native.ollamaChat = realChat;
      U.closeNP();

      /* the loop: right-click the seek bar to start and end it; it holds; the looped time counts in Stats */
      P.playIds([glass.id], 0, null);
      await waitFor(() => title() === 'Glass Orbit' && P.playing && P.duration() > 20, 5000);
      const smL = N.stats.model, todayL = smL.period('day'), heardL0 = smL.range(todayL.from, todayL.to).total;
      const sb0 = document.getElementById('seek').getBoundingClientRect(), dur = P.duration();
      const rightClick = (frac, label) => {
        const sb = document.getElementById('seek').getBoundingClientRect();
        document.getElementById('seek').dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: sb.left + sb.width * frac, clientY: sb.top + sb.height / 2 }));
        const item = [...document.querySelectorAll('#menu button')].find(x => x.textContent.includes(label));
        if (item) item.click();
        return !!item;
      };
      check('right-click the bar: start the loop here', rightClick(3 / dur, 'Start the loop here') && Math.abs((P.loop() || {}).a - 3) < .3, P.loop());
      check('right-click again: end the loop here', rightClick(6 / dur, 'End the loop here') && Math.abs((P.loop() || {}).b - 6) < .3, P.loop());
      P.seek(4.5);
      let lo = 99, hi = 0, jumped = false, prevT = P.time();
      const tL = Date.now();
      while (Date.now() - tL < 4500) { await sleep(50); const t = P.time(); lo = Math.min(lo, t); hi = Math.max(hi, t); if (t < prevT - 1) jumped = true; prevT = t; }
      check('the loop holds between its ends', jumped && lo >= 2.8 && hi <= 6.4 && title() === 'Glass Orbit', { lo, hi, jumped, title: title() });
      const heardL1 = smL.range(todayL.from, todayL.to).total;
      check('looped time counts as listening time', heardL1 - heardL0 > 3.6, heardL1 - heardL0);
      await minimise(true);
      const originalSeek = P.seek, jumps = [];
      try {
        P.seek = sec => { jumps.push(P.time()); return originalSeek(sec); };
        const heardBefore = smL.range(todayL.from, todayL.to).total;
        await waitFor(() => jumps.length >= 2, 7500);
        check('muted hidden loops land at their boundary', jumps.length >= 2 && jumps.every(t => Math.abs(t - P.loop().b) < .12), jumps);
        check('listening ticks while hidden', smL.range(todayL.from, todayL.to).total - heardBefore > 2.8);
      } finally { P.seek = originalSeek; await minimise(false); }
      const band = document.getElementById('loopBand'), chip = document.getElementById('loopChip');
      check('the bar shows the loop, and a chip its times', !band.hidden && !chip.hidden && /0:0[23]–0:0[56]/.test(chip.textContent), chip.textContent);
      const sb1 = document.getElementById('seek').getBoundingClientRect();
      check('setting a loop never moves or resizes the bar', Math.abs(sb1.left - sb0.left) < .5 && Math.abs(sb1.width - sb0.width) < .5, [sb0.left, sb0.width, sb1.left, sb1.width]);
      await step('loop', 1000);
      P.pause(); await sleep(200);
      check('a song remembers its loop', !!(N.S.loops || {})[glass.id]);
      chip.click();
      check('the chip stops the loop', !P.loop() && band.hidden);
      P.play();
      P.seek(5.2);
      check('without the loop, the song plays on', await waitFor(() => P.time() > 6.8, 4000), P.time());
      P.setLoop(dur - 4, null);
      P.seek(dur - 1.4);
      await sleep(2600);
      check('a loop to the end goes round instead of moving on', title() === 'Glass Orbit' && P.playing && P.time() >= dur - 4.2 && P.time() < dur - 1, { title: title(), t: P.time(), dur });
      P.clearLoop();
      P.pause();
      await sleep(2200);
      const f1 = N.viz.frames; await sleep(700);
      check('animation rests when paused', N.viz.frames === f1, [f1, N.viz.frames]);
      check('paused', !P.playing);

      /* a huge queue (shuffle 12,000 songs) is saved as a window round where it stands */
      const realQ = N.S.queue;
      const big = Array.from({ length: 12000 }, (_, i) => ({ id: 'd:\\music\\some artist\\some album\\' + String(i).padStart(5, '0') + ' a song title.mp3', m: i === 5002 ? 1 : 0 }));
      N.S.queue = { ...realQ, items: big, at: 5000, orig: big.map(x => x.id), ctx: { kind: 'songs', key: '', name: 'All songs' } };
      const snap = N.snapshot(), size = JSON.stringify(snap).length;
      const back = N.mergeState({ ...N.S, queue: realQ }, JSON.parse(JSON.stringify(snap))).queue;
      N.S.queue = realQ;
      check('saved queue stays small', size < 250000, Math.round(size / 1024) + ' KB');
      check('all 12,000 queue entries and their original order survive restart', back.items.length === big.length && back.orig.length === big.length && big.every((x, i) => back.items[i].id === x.id && back.items[i].m === x.m && back.orig[i] === x.id));
      check('saved queue comes back at the same song', back.items[back.at].id === big[5000].id && back.items[back.at + 2].m === 1 && back.items[back.at + 1].m === 0, { at: back.at, len: back.items.length });

      /* files String keeps */
      await sleep(900);
      const pres = JSON.parse(await belt.native.readText(paths.data + '\\presence.json') || '{}');
      check('presence.json says what is playing', pres.app === 'String' && pres.state === 'paused' && !!pres.title && pres.duration > 0, pres);
      await sleep(1000);
      const cache = JSON.parse(await belt.native.readText(paths.data + '\\library.json') || '{}');
      check('library cache saved', cache.tracks && cache.tracks.length === 8, cache.tracks && cache.tracks.length);

      /* listening stats: the seconds played above were counted, by song and artist, and the Stats page shows them */
      const sm = N.stats.model, dayP = sm.period('day');
      const heard = sm.range(dayP.from, dayP.to);
      check('listening time counted', heard.total > 15, heard.total);
      check('top artist is the one played most', heard.artists.length > 0 && heard.artists[0].name === 'Test Signal', heard.artists.slice(0, 3));
      const wkP = sm.period('week'), moP = sm.period('month'), yrP = sm.period('year');
      check('this week, month and year include today', [wkP, moP, yrP].every(x => sm.range(x.from, x.to).total >= heard.total));
      check('the hour bar holds today\'s time', Math.abs(sm.series(dayP).reduce((a, b) => a + b, 0) - heard.total) < 1);
      N.S.statsKind = 'day';
      U.go('stats'); await sleep(1000);
      check('Stats page: four tiles, bars, top artists and songs', document.querySelectorAll('#view .stat-tile').length === 4 && document.querySelectorAll('#view .bars .b').length > 0 && document.querySelectorAll('#view .rank').length >= 2,
        { tiles: document.querySelectorAll('#view .stat-tile').length, bars: document.querySelectorAll('#view .bars .b').length, ranks: document.querySelectorAll('#view .rank').length });
      const hit = document.querySelector(`#view .bars .hit[data-i="${new Date().getHours()}"]`);
      if (hit) hit.dispatchEvent(new PointerEvent('pointermove', { bubbles: true }));
      const tipEl = document.querySelector('#view .chart-tip');
      check('a bar tells its value on hover', !!tipEl && !tipEl.hidden && /min/.test(tipEl.textContent), tipEl && tipEl.textContent);
      check('today shows in the sidebar', /\dm|<1m/.test(document.getElementById('cToday').textContent), document.getElementById('cToday').textContent);
      await step('stats-today', 1300);
      N.S.statsKind = 'week'; U.render({ keep: true }); await step('stats-week', 1300);
      document.getElementById('chartAsTable').click(); await sleep(400);
      check('the chart has a table view', document.querySelectorAll('#view .stat-tbl tr').length === 8);
      await step('stats-table', 900);
      document.getElementById('chartAsTable').click(); await sleep(300);
      N.stats.saveNow(); await sleep(700);
      const lj = JSON.parse(await belt.native.readText(paths.data + '\\listening.json') || '{}');
      check('listening.json saved', lj.v === 1 && !!lj.days && !!lj.days[sm.dayKey(new Date())], lj.days && Object.keys(lj.days));

      /* Discord: the helper starts from String, reports back, and stops (a pipe that doesn't exist: never the real Discord) */
      Object.assign(N.S.set, { discord: true, discordId: '123456789012345678', discordPipe: 'string-selftest-nopipe-' + Date.now() });
      U.go('settings'); await sleep(400);
      U.discordSync();
      const dcWait = await waitFor(() => N.discord.status === 'waiting' && !!N.discord.proc, 8000);
      check('Discord helper starts and waits for Discord', dcWait, { status: N.discord.status, error: N.discord.error });
      check('Discord status shown in settings', /Waiting for Discord/.test((document.getElementById('dcStatus') || {}).textContent || ''));
      document.getElementById('dcPanel').scrollIntoView();
      await step('discord', 1100);
      N.S.set.discordId = '12';
      U.discordSync();
      check('a short ID is refused and the helper stopped', N.discord.status === 'badid' && !N.discord.proc, N.discord.status);
      Object.assign(N.S.set, { discord: false, discordId: '', discordPipe: '' });
      U.discordSync();
      check('turned off', N.discord.status === 'off' && !N.discord.proc);
      document.getElementById('scroll').scrollTop = 0;
      await step('settings', 1100);
      U.go('album/' + encodeURIComponent(sw.key)); await sleep(900);
      const row = document.querySelector('#view .tr');
      const rr = row.getBoundingClientRect();
      U.trackMenu([sw.ids[0]], rr.left + 300, rr.top + 20); await step('menu', 900); U.closeMenu();
      P.play(); await sleep(300);
      U.go('home'); await step('home-playing', 1400);

      /* the taskbar thumbnail buttons (hover String's taskbar icon): previous, play/pause, next */
      await waitFor(() => P.playing, 3000);
      await sleep(300);
      let thumb = await belt.native.smokeThumb(-1);
      check('taskbar has three buttons, showing Pause while playing', thumb.buttons === 3 && thumb.play === 'Pause' && thumb.playing === true, thumb);
      await belt.native.smokeThumb(1);
      await waitFor(() => !P.playing, 3000);
      await sleep(300);
      thumb = await belt.native.smokeThumb(-1);
      check('taskbar play/pause pauses, and turns into Play', !P.playing && thumb.play === 'Play', { playing: P.playing, thumb });
      await belt.native.smokeThumb(1);
      check('taskbar play/pause plays again', await waitFor(() => P.playing, 3000));
      const was = title();
      await belt.native.smokeThumb(2);
      check('taskbar next skips ahead', await waitFor(() => title() !== was && P.playing, 4000), [was, title()]);
      await belt.native.smokeThumb(0);
      check('taskbar previous goes back', await waitFor(() => title() === was, 4000), title());
      P.pause();

      /* add-ons in the test's data folder (run-selftest.ps1 plants a small probe add-on, and -Addon <folder> adds a real
         one): each one's own checks */
      const addonsLoaded = await belt.addonsReady;
      check('add-ons: the probe add-on loaded, a broken one was skipped, and one naming a file outside its folder lost that file', addonsLoaded.some(a => a.id === 'probe') && !addonsLoaded.some(a => a.id === 'broken') && (addonsLoaded.find(a => a.id === 'sneaky') || { scripts: ['x'] }).scripts.length === 0, addonsLoaded.map(a => [a.id, a.scripts]));
      for (const t of N.addons.tests) await t({ check, sleep, waitFor, step, paths, media, lower, loose, notags, L, P, U, N, title });

      /* duplicates: a second copy of Glass Orbit (surely the same) and of Northern Static under another credit (maybe the
         same) in the library. The Duplicates page finds both, you pick the copy to keep; Hide moves the copy's
         like, plays, loop and playlist place to the kept one and leaves the file; the Recycle Bin (here: a folder in the
         test's data folder, never the real Recycle Bin) takes a ticked copy, and a copy that's playing hands over */
      const dupDir = paths.data + '\\dupes-selftest';
      await belt.native.mkdir(dupDir);
      if (!N.S.roots.includes(dupDir)) N.S.roots.push(dupDir);
      const gCopy = dupDir + '\\glass orbit (1).mp3', nCopy = dupDir + '\\northern static live.m4a';
      await belt.native.copy(media + '\\glass orbit.mp3', gCopy);
      await belt.native.copy(media + '\\northern static.m4a', nCopy);
      const gId = gCopy.toLowerCase(), nId = nCopy.toLowerCase(), gOrig = lower('glass orbit.mp3'), nOrig = lower('northern static.m4a');
      N.S.edits[nId] = { artist: 'Another Credit' };
      await L.scan(); await sleep(200);
      check('duplicates: the copies are in the library', L.tracks.has(gId) && L.tracks.has(nId));
      const smokePl = N.S.playlists.find(p => p.id === 'smoke');
      smokePl.items.push({ id: gId, at: Date.now() });
      N.S.liked[gId] = 1234; delete N.S.liked[gOrig];
      const origPlays = (N.S.plays[gOrig] || [0])[0];
      N.S.plays[gId] = [3, Date.now()];
      N.S.loops[gId] = { a: 1, b: 3 }; delete N.S.loops[gOrig];
      U.go('settings'); await sleep(400);
      document.querySelector('#dupBody [data-go-to="dupes"]').click(); await sleep(600);
      const dpGroups = () => [...document.querySelectorAll('#view .dp-group')];
      const rowOf = id => [...document.querySelectorAll('#view .dp-row')].find(r => r.querySelector(`[data-dp-play]`).dataset.dpPlay === id);
      const keepBtn = id => [...document.querySelectorAll('[data-dp-keep]')].find(r => r.value === id);
      check('duplicates: Library → Duplicates opens the page; both songs found', U.route() === 'dupes' && dpGroups().length === 2, dpGroups().map(g => g.textContent.slice(0, 60)));
      const gGroup = dpGroups().find(g => /Glass Orbit/.test(g.querySelector('.dp-h').textContent)), nGroup = dpGroups().find(g => /Northern Static/.test(g.querySelector('.dp-h').textContent));
      check('duplicates: Glass Orbit is surely the same, Northern Static under another credit only maybe', !!gGroup && !gGroup.classList.contains('maybe') && !!nGroup && nGroup.classList.contains('maybe'));
      keepBtn(gOrig).click(); await sleep(300);
      keepBtn(nOrig).click(); await sleep(300);
      check('duplicates: keeping the original ticks the sure copy, not the maybe one', rowOf(gOrig).classList.contains('keep') && rowOf(gId).classList.contains('drop') && !rowOf(nId).classList.contains('drop'), [...document.querySelectorAll('[data-dp-drop]')].map(c => c.checked));
      check('duplicates: the copy shows it is liked and in a playlist', /liked/.test(rowOf(gId).textContent) && /in 1 playlist/.test(rowOf(gId).textContent), rowOf(gId).querySelector('.dp-meta').textContent);
      await step('duplicates', 1300);
      const songsBefore = L.all.length;
      const realRecycle = belt.native.recycle;
      belt.native.recycle = async files => ({ done: [], failed: files.map(path => ({ path, error: 'Locked test copy' })) });
      document.querySelector('[data-dp-fix="recycle"]').click();
      await waitFor(() => document.querySelector('.scrim [data-k="ok"]'));
      document.querySelector('.scrim [data-k="ok"]').click();
      await waitFor(() => document.querySelector('#hostToast')?.textContent.includes('Locked test copy'));
      belt.native.recycle = realRecycle;
      check('failed recycling keeps the original like, loop, plays and playlist entry', N.S.liked[gId] === 1234 && N.S.loops[gId]?.b === 3 && N.S.plays[gId]?.[0] === 3 && smokePl.items.some(x => x.id === gId));
      keepBtn(gOrig).click(); await sleep(200);
      keepBtn(nOrig).click(); await sleep(200);
      document.querySelector('[data-dp-fix="hide"]').click(); await sleep(500);
      check('duplicates: Hide keeps the copy out of String, the file stays', !L.all.includes(gId) && L.all.length === songsBefore - 1 && (await belt.native.stat(gCopy)).exists, L.all.length);
      check('duplicates: hidden songs stay out of search', !L.search('glass orbit').tracks.includes(gId));
      check('duplicates: the kept copy takes the like, plays, loop and playlist place', N.S.liked[gOrig] === 1234 && !N.S.liked[gId] && N.S.plays[gOrig][0] === origPlays + 3 && !N.S.plays[gId] &&
        N.S.loops[gOrig] && N.S.loops[gOrig].b === 3 && !N.S.loops[gId] && smokePl.items.some(x => x.id === gOrig) && !smokePl.items.some(x => x.id === gId) && new Set(smokePl.items.map(x => x.id)).size === smokePl.items.length,
        { liked: N.S.liked[gOrig], plays: N.S.plays[gOrig], loop: N.S.loops[gOrig], pl: smokePl.items.map(x => x.id) });
      check('duplicates: only the maybe group is left, unticked', dpGroups().length === 1 && !document.querySelector('[data-dp-drop]:checked') && document.querySelector('[data-dp-fix="recycle"]').disabled);
      // "Not the same" on the maybe group: it stops being listed, until "list them again"
      document.querySelector('[data-dp-apart]').click(); await sleep(400);
      check('duplicates: Not the same stops listing that pair', !document.querySelector('#view .dp-group') && Object.keys(N.S.notDupes).length === 1 && !!document.getElementById('dpForget'), Object.keys(N.S.notDupes || {}).length);
      document.getElementById('dpForget').click(); await sleep(400);
      check('duplicates: …and it can be listed again', dpGroups().length === 1 && Object.keys(N.S.notDupes).length === 0);
      // the maybe copy is playing when it goes to the Recycle Bin: the kept one carries on
      P.playIds([nId], 0, null);
      await waitFor(() => P.playing && P.time() > 1.2, 5000);
      keepBtn(nOrig).click(); await sleep(300);
      [...document.querySelectorAll('[data-dp-drop]')].find(c => c.dataset.dpDrop === nId).click(); await sleep(300);
      document.querySelector('[data-dp-fix="recycle"]').click(); await sleep(300);
      check('duplicates: the Recycle Bin asks first', /Recycle Bin/.test((document.querySelector('.scrim .dialog') || {}).textContent || ''));
      await step('duplicates-ask', 700);
      const before = P.time();
      document.querySelector('.scrim [data-k="ok"]').click();
      check('duplicates: the ticked copy left its folder (into the test\'s own stand-in for the Recycle Bin)', await waitFor(() => !L.tracks.has(nId), 6000) && !(await belt.native.stat(nCopy)).exists && (await belt.native.stat(paths.data + '\\recycled-selftest\\northern static live.m4a')).exists);
      await sleep(600);
      check('duplicates: the song playing carries on from its kept copy, at the same moment', title() === 'Northern Static' && P.current().id === nOrig && P.playing && P.time() >= before - 0.5, { id: P.current().id, t: P.time(), before });
      check('duplicates: nothing left to fix', !document.querySelector('#view .dp-group') && /No duplicates/.test(document.getElementById('view').textContent));
      P.pause();
      document.getElementById('dupUnhide').click(); await sleep(400);
      check('duplicates: hidden copies shown again', L.all.includes(gId) && Object.keys(N.S.hidden).length === 0 && dpGroups().length === 1);
      await step('duplicates-after', 900);

      /* a song named the wrong way round: Swap title and artist, from its menu; an edit of your own */
      const swp = lower('glass orbit.mp3');
      U.go('songs'); await sleep(500);
      U.trackMenu([swp], 400, 300); await sleep(300);
      const swapItem = [...document.querySelectorAll('#menu button')].find(b => /Swap title and artist/.test(b.textContent));
      check('the song menu offers Swap title and artist', !!swapItem);
      if (swapItem) swapItem.click();
      await sleep(300);
      check('Swap title and artist swaps them, as an edit of your own', L.get(swp).T === 'Test Signal' && L.get(swp).A === 'Glass Orbit' && !!N.S.edits[swp] && !N.S.edits[swp].auto, { T: L.get(swp).T, A: L.get(swp).A, edit: N.S.edits[swp] });
      // and back (Stats counts it under its own names further on)
      U.trackMenu([swp], 400, 300); await sleep(300);
      [...document.querySelectorAll('#menu button')].find(b => /Swap title and artist/.test(b.textContent)).click(); await sleep(300);
      delete N.S.edits[swp]; N.save(); L.refresh();
      check('…and back again', L.get(swp).T === 'Glass Orbit' && L.get(swp).A === 'Test Signal');

      /* phone listening: songs played through Navidrome, from its play history (here a fake Navidrome database the test
         made, never a real one): Glass Orbit twice, Low Tide once, and a song String doesn't have (200 s) */
      const today = () => N.stats.model.range(new Date(), new Date());
      const phoneBefore = today();
      check('shared Navidrome waits for a choice', await N.phone.sync() === 0 && Object.keys(N.phone.users).length === 2);
      U.go('stats');
      await waitFor(() => document.getElementById('phoneUser'));
      const account = document.getElementById('phoneUser');
      account.value = 'u1';
      account.dispatchEvent(new Event('change', { bubbles: true }));
      await N.phone.sync();
      const added = N.S.phone.plays;
      const phoneAfter = today(), glassT = L.get(lower('glass orbit.mp3')), tideT = L.get(lower('low tide.flac'));
      const expect = glassT.duration * 2 + tideT.duration + 200;
      check('phone: Navidrome\'s plays are counted in Stats, as whole songs, and as phone time', added === 4 && Math.abs(phoneAfter.phone - phoneBefore.phone - expect) < 1 && Math.abs(phoneAfter.total - phoneBefore.total - expect) < 1 && N.S.phone.found && N.S.phone.after === 4, { added, phone: phoneAfter.phone - phoneBefore.phone, expect, st: N.S.phone });
      const tops = phoneAfter.tracks;
      check('phone: a song String has is counted under String\'s own names', tops.some(x => x.id === glassT.id && x.title === 'Glass Orbit') && !tops.some(x => x.title === 'Navidrome Title'), tops.slice(0, 5));
      check('phone: a song String doesn\'t have still counts, under Navidrome\'s names', tops.some(x => x.title === 'Old Song' && x.artist === 'Someone Else' && x.secs >= 200));
      check('phone: checking again counts nothing twice or from another listener', (await N.phone.sync()) === 0 && Math.abs(today().phone - phoneAfter.phone) < 0.01 && N.S.phone.plays === 4);
      U.go('stats'); await sleep(600);
      const statsText = () => document.getElementById('view').textContent;
      check('phone: the Stats page shows the phone time and where it comes from', /on your phone/.test(statsText()) && /Phone listening comes from Navidrome on this PC: 4 plays counted/.test(statsText()));
      document.querySelector('.stat-phone').scrollIntoView();
      await step('stats-phone', 1100);
      document.querySelector('[data-phone="off"]').click(); await sleep(300);
      check('phone: counting can be turned off', N.S.set.phoneStats === false && /isn't counted/.test(statsText()) && (await N.phone.sync()) === 0);
      document.querySelector('[data-phone="on"]').click(); await sleep(500);
      check('phone: …and on again, without counting anything twice', N.S.set.phoneStats === true && N.S.phone.plays === 4 && Math.abs(today().phone - phoneAfter.phone) < 0.01);

      /* your own look: <data>\look\look.css (pictures and fonts beside it) loads after String's own styles, from a folder
         updates never touch; the host watches it, so a saved change shows at once */
      const rootCss = () => getComputedStyle(document.documentElement);
      const lookHi = () => rootCss().getPropertyValue('--hi').trim();
      const lookLink = () => document.getElementById('lookCss');
      const hiBefore = lookHi();
      const emptyLook = await fetch('/_look/look.css');
      check('look: without a look.css it is an empty stylesheet, loaded after String\'s own (index.html unchanged)', emptyLook.ok && (await emptyLook.text()) === '' && [...document.querySelectorAll('link[rel=stylesheet]')].pop() === lookLink() && /\/_look\/v\d+\/look\.css$/.test(lookLink().href) && !/look/.test(await (await fetch('/index.html')).text()));
      const lookPaths = await belt.native.paths();
      await belt.native.writeText(lookPaths.data + '\\outside.css', 'body { color: red }');
      // run-selftest.ps1 put a folder link at look\linked, pointing outside the look folder
      const sneak = await Promise.all(['/_look/%2e%2e%5coutside.css', '/_look/..%2Foutside.css', '/_look/C%3A%5CWindows%5Cwin.ini', '/_look/linked/secret.css', '/_look/v7/linked/secret.css'].map(u => fetch(u).then(r => r.status)));
      check('look: nothing outside the look folder is served, not even through a link in it', sneak.every(s => s === 404), sneak);
      const made = await belt.native.lookFolder({ quiet: true });
      const starter = await (await fetch('/_look/look.css')).text();
      await sleep(600);
      check('look: the look folder is made with a starter look.css that changes nothing', made.made && made.path === lookPaths.look && /Your look for String/.test(starter) && lookHi() === hiBefore, { made, hi: lookHi() });
      check('look: opening it again keeps the person\'s file', !(await belt.native.lookFolder({ quiet: true })).made);
      // the starter's examples, all switched on (their pictures as small SVGs), saved the way an editor would
      const svg = fill => `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 200 60"><rect width="200" height="60" fill="${fill}"/></svg>`;
      await belt.native.writeText(lookPaths.look + '\\logo.svg', svg('#d4af37'));
      await belt.native.writeText(lookPaths.look + '\\background.svg', svg('#3a0a12'));
      const allOn = starter.split('\n').filter(l => !/^\/\* ---/.test(l) && l.trim() !== '*/').join('\n').split('logo.png').join('logo.svg').split('background.jpg').join('background.svg');
      await belt.native.writeText(lookPaths.look + '\\look.css', allOn);
      const applied = await waitFor(() => lookHi() === '212 175 55', 5000);
      const brand = document.querySelector('.brand');
      check('look: a saved look.css shows at once (colour, surfaces, text)', applied && rootCss().getPropertyValue('--bg').trim() === '#12060a' && rootCss().getPropertyValue('--text').trim() === '#f4ead2', { hi: lookHi() });
      check('look: the logo and background examples use pictures from the look folder', /logo\.svg/.test(getComputedStyle(brand).backgroundImage) && [...brand.children].every(c => getComputedStyle(c).display === 'none') && /background\.svg/.test(getComputedStyle(document.getElementById('main')).backgroundImage));
      const logoRes = await fetch('/_look/logo.svg');
      check('look: its pictures are served with their type', logoRes.ok && /image\/svg\+xml/.test(logoRes.headers.get('content-type') || ''));
      U.go('home'); await step('look', 1300);
      // a picture replaced under the same name shows the new one (each reload has its own address)
      const bgBefore = getComputedStyle(brand).backgroundImage;
      await belt.native.writeText(lookPaths.look + '\\logo.svg', svg('#2a6fdb'));
      const swapped = await waitFor(() => { const b = getComputedStyle(brand).backgroundImage; return b !== bgBefore && /logo\.svg/.test(b); }, 5000);
      const newUrl = (getComputedStyle(brand).backgroundImage.match(/url\("?([^")]+)"?\)/) || [])[1] || '';
      check('look: a picture saved again under the same name shows the new one', swapped && /#2a6fdb/.test(await (await fetch(newUrl)).text()), { before: bgBefore, now: getComputedStyle(brand).backgroundImage });
      U.go('settings'); await sleep(500);
      document.getElementById('lookOn').click(); await sleep(300);
      check('look: "Use my look.css" off puts String\'s own look back, and it isn\'t loaded at the next start', N.S.set.look === false && lookHi() === hiBefore && !lookLink() && localStorage.getItem('string.lookOff') === '1');
      document.getElementById('lookOn').click();
      check('look: …and on again', await waitFor(() => lookHi() === '212 175 55', 3000) && N.S.set.look === true && !!lookLink() && localStorage.getItem('string.lookOff') === '0');
      belt.reloadLook(); belt.reloadLook(); belt.reloadLook();
      const settled = await waitFor(() => document.querySelectorAll('link[data-look]').length === 1 && document.querySelector('link[data-look]') === lookLink(), 4000);
      check('look: reloads close together end with one sheet, the newest', settled && lookHi() === '212 175 55', document.querySelectorAll('link[data-look]').length);
      belt.setLook(false); belt.reloadLook(); await sleep(400);
      check('look: a reload (a save in the folder) never turns an off look on, nor changes the setting', !document.querySelector('link[data-look]') && localStorage.getItem('string.lookOff') === '1');
      belt.setLook(true);
      await waitFor(() => lookHi() === '212 175 55', 3000);
      await belt.native.writeText(lookPaths.look + '\\look.css', '');
      check('look: emptying look.css brings String\'s look back', await waitFor(() => lookHi() === hiBefore, 5000), lookHi());

      /* the icon's credit: a card the first time String starts with this icon (a self-test never shows it on its own);
         closing it is remembered, in the setting String 2.0 shares */
      const creditCard = () => document.getElementById('stringIconCredit');
      check('credit: a self-test doesn\'t show it on its own', !creditCard());
      delete N.S.set.iconCreditSeen;
      U.iconCredit();
      const credit = creditCard();
      const creditImg = credit && credit.querySelector('img');
      check('credit: the card thanks the icon\'s maker, with the logo', !!credit && /Credits to Discord — awepollyon for the icon!/.test(credit.textContent) && await waitFor(() => creditImg.naturalWidth > 0, 3000));
      // its own layout, whatever string.css says (an update keeps a person's edited one)
      const box = credit && credit.getBoundingClientRect(), bar = document.getElementById('bar').getBoundingClientRect();
      check('credit: it floats above the player bar, at the right', !!box && getComputedStyle(credit).position === 'fixed' && box.bottom <= bar.top && box.right >= innerWidth - 40, { box, barTop: bar.top });
      // Space on its Close button closes the card, and doesn't pause or play
      const wasPlaying = P.playing;
      const closeBtn = document.getElementById('stringIconCreditClose');
      closeBtn.focus();
      closeBtn.dispatchEvent(new KeyboardEvent('keydown', { key: ' ', bubbles: true }));
      await sleep(150);
      check('credit: Space on its Close button doesn\'t play or pause', P.playing === wasPlaying);
      await step('icon-credit', 600);
      closeBtn.click();
      check('credit: closing it is remembered', !creditCard() && N.S.set.iconCreditSeen === true);
      U.iconCredit();
      check('credit: once closed it doesn\'t come back', !creditCard());

      /* String's updates (src\Updater.cs) against a fake GitHub (tests\fake-github.js) with the test's own release key:
         never the real GitHub, and a test never starts an installer, it only unpacks the release */
      const fakeFeed = name => `http://127.0.0.1:47915/repos/${name}/releases/latest`;
      const uInfo = await belt.native.updateInfo();
      check('updates: a test copy counts as installed, with somewhere to look', uInfo.installed && uInfo.published && /^\d+\.\d+\.\d+$/.test(uInfo.current), uInfo);
      U.go('settings'); await sleep(500);
      await N.updates.check(false);
      const US = N.updates.state;
      check('updates: a newer release is found, with its notes', !!US.info && US.info.newer && US.info.latest === '9.9.9' && /Fake notes/.test(US.info.notes), US.info);
      check('updates: Settings offers to install it', /String 9\.9\.9 is out/i.test(document.getElementById('updBody').textContent) && !!document.getElementById('updGo'));
      document.getElementById('updPanel').scrollIntoView();
      await step('updates', 900);
      document.getElementById('updGo').click();
      await waitFor(() => document.querySelector('.scrim [data-k="ok"]'), 3000);
      document.querySelector('.scrim [data-k="ok"]').click();
      check('updates: installing downloads it, checks its signature and unpacks it', await waitFor(() => US.done && US.done.ready, 15000), US.msg);
      const uDir = US.done && US.done.ready;
      check('updates: …with its installer, run for this copy\'s own folder', !!uDir && (await belt.native.stat(uDir + '\\app\\setup\\install.ps1')).exists && /\\app\\setup\\install\.ps1$/.test(US.done.script || '') && !!US.done.to, US.done);
      const tryInstall = async name => { await belt.native.smokeUpdate({ feed: fakeFeed(name) }); try { await belt.native.updateInstall({ version: '' }); return 'installed'; } catch (e) { return e.message; } };
      const tamperedR = await tryInstall('tampered');
      check('updates: a download that isn\'t signed with String\'s release key is refused', /isn't signed/.test(tamperedR), tamperedR);
      const liarR = await tryInstall('liar');
      check('updates: a signed release that says inside it\'s another version is refused', /another version/.test(liarR), liarR);
      await belt.native.smokeUpdate({ feed: fakeFeed('old') });
      const oldR = await belt.native.updateCheck();
      check('updates: an older release isn\'t offered', oldR.latest === '0.0.1' && !oldR.newer, oldR);
      const olderR = await tryInstall('old');
      check('updates: …nor installed', /already up to date/.test(olderR), olderR);
      await belt.native.smokeUpdate({ feed: fakeFeed('none') });
      const noneR = await belt.native.updateCheck();
      check('updates: nothing released yet is just nothing new', noneR.published && !noneR.newer && !noneR.latest, noneR);
      US.done = null; US.busy = false; US.msg = '';

      /* icon centering, autoplay, time left, list filter and Recent */
      const centered = id => {
        const b = document.getElementById(id), s = b && b.querySelector('svg');
        if (!b || !s) return false;
        const br = b.getBoundingClientRect(), sr = s.getBoundingClientRect();
        const ink = s.getBBox(), vb = s.viewBox.baseVal;
        return Math.abs((sr.left + sr.width / 2) - (br.left + br.width / 2)) <= .6 && Math.abs((sr.top + sr.height / 2) - (br.top + br.height / 2)) <= .6 &&
          ink.width > 0 && ink.height > 0 && Math.abs(ink.x + ink.width / 2 - vb.x - vb.width / 2) <= 1.8 && Math.abs(ink.y + ink.height / 2 - vb.y - vb.height / 2) <= 1.8;
      };
      check('icons: buttons center their icons cleanly', ['newPl', 'back', 'fwd', 'openSet', 'shuffle', 'prev', 'pp', 'next', 'repeat', 'qBtn', 'npBtn'].every(centered));
      const singleAlbum = L.albums.get(ogg.albumKey), order = N.sortedAlbums();
      const nextAlbum = order[(order.indexOf(singleAlbum) + 1) % order.length];
      const albumCtx = { kind: 'album', key: singleAlbum.key, name: singleAlbum.name };
      const playingId = id => waitFor(() => P.current()?.id === id && P.playing && P.time() > .25, 5000);
      P.clearQueue();
      N.S.set.autoplay = true; N.S.queue.repeat = 'off'; N.S.queue.shuffle = false;
      P.playIds(singleAlbum.ids, 0, albumCtx);
      const albumStarted = await playingId(ogg.id);
      P.next(true);
      const albumSkipped = await playingId(nextAlbum.ids[0]);
      check('skipping a one-song album starts playback in the next album', singleAlbum.ids.length === 1 && albumStarted && albumSkipped && P.context()?.key === nextAlbum.key, { title: title(), time: P.time(), ctx: P.context() });
      const oldFade = N.S.set.fade;
      N.S.set.fade = 2;
      P.playIds(singleAlbum.ids, 0, albumCtx);
      await playingId(ogg.id);
      await minimise(true);
      try {
        P.seek(P.duration() - 4);
        const startFade = performance.now();
        const faded = await waitFor(() => P.crossfading() && P.current()?.id === nextAlbum.ids[0], 5000);
        const elapsed = (performance.now() - startFade) / 1000;
        check('autoplay crossfades into the next album on time while hidden', faded && elapsed >= 1.85 && elapsed < 2.5, { title: title(), elapsed });
        await waitFor(() => !P.crossfading());
        const presence = JSON.parse(await belt.native.readText(pdir + '\\presence.json'));
        check('hidden crossfade updates the presence time and title', presence.state === 'playing' && presence.title === title() && Math.abs(presence.startedAt - (Date.now() - P.time() * 1000)) < 1500);
        N.S.set.fade = 0;
        P.playIds(singleAlbum.ids, 0, albumCtx);
        await playingId(ogg.id);
        P.seek(P.duration() - 2.5);
        const startHandover = performance.now();
        const advanced = await waitFor(() => P.current()?.id === nextAlbum.ids[0] && P.time() > .05, 4000);
        const delay = (performance.now() - startHandover) / 1000 - P.time() - 2.5;
        check('hidden gapless playback hands over to the preloaded album', advanced && Math.abs(delay) < .25 && !P.crossfading(), { delay });
      } finally { await minimise(false); }
      N.S.set.fade = oldFade;
      const allSongs = N.ctxIds({ kind: 'songs', key: '' });
      const nextLiked = allSongs.find(id => id !== ogg.id && L.get(id).artistKey === ogg.artistKey);
      P.playIds([ogg.id], 0, { kind: 'liked', key: '', name: 'Liked songs' });
      const likedStarted = await playingId(ogg.id);
      P.next(true);
      const likedSkipped = await playingId(nextLiked);
      check('skipping a one-song Liked list starts playback of another song', likedStarted && likedSkipped, { title: title(), time: P.time() });
      P.playIds(allSongs, allSongs.length - 1, { kind: 'songs', key: '', name: 'All songs' });
      const lastStarted = await playingId(allSongs.at(-1));
      P.next(true);
      const songsSkipped = await playingId(allSongs[0]);
      check('skipping the last song in Songs starts playback at the top', lastStarted && songsSkipped, { title: title(), time: P.time() });
      const timeButton = document.getElementById('tDur'), seek = document.getElementById('seek'), beforeSeek = seek.getBoundingClientRect();
      timeButton.focus();
      timeButton.click();
      const afterSeek = seek.getBoundingClientRect();
      check('duration toggles remaining time without moving or resizing the seek bar', timeButton.tagName === 'BUTTON' && document.activeElement === timeButton &&
        timeButton.getAttribute('aria-pressed') === 'true' && /^-/.test(timeButton.textContent) && beforeSeek.left === afterSeek.left && beforeSeek.width === afterSeek.width, timeButton.textContent);
      timeButton.click();
      U.go('songs'); await sleep(400);
      const lf = document.querySelector('#view [data-lf]');
      lf.value = 'low tide'; lf.dispatchEvent(new Event('input')); await sleep(200);
      check('QOL: in-list filter narrows Songs', document.querySelectorAll('#view .tr').length === 1);
      document.querySelector('#view [data-lf-x]').click(); await sleep(200);
      check('QOL: clearing the filter restores all songs', document.querySelectorAll('#view .tr').length >= 8);
      U.openQueue(); await sleep(200);
      document.querySelector('#qTab [data-k="recent"]').click(); await sleep(200);
      check('QOL: Queue drawer Recent tab lists played songs', document.querySelectorAll('#qBody [data-rec]').length > 0);
      document.querySelector('#qTab [data-k="queue"]').click();
      U.closeQueue();
      P.pause();

      /* a save the disk refuses (a full or locked drive): the change is kept and tried again, and closing String waits,
         says why, and offers to stay open; once the disk works the change reaches state.json */
      const stateFile = paths.data + '\\state.json';
      const plName = 'Unsaved playlist ' + Date.now();
      const onDisk = async () => (await belt.native.readText(stateFile).catch(() => '')) || '';
      await belt.native.smokeSaves({ broken: true });
      N.S.playlists.push({ id: 'smoke-unsaved', name: plName, items: [] });
      N.save();
      check('saves: a save the disk refused is kept and reported', await waitFor(() => belt.unsaved().includes('state'), 4000), belt.unsaved());
      check('saves: …and the change isn\'t on the disk yet', !(await onDisk()).includes(plName));
      await step('save-refused', 800);
      await belt.native.smokeClose();
      const blocked = await waitFor(() => [...document.querySelectorAll('.scrim .dialog h2')].some(h => /aren.t saved/.test(h.textContent)), 6000);
      check('saves: closing with unsaved changes keeps String open and says why', blocked);
      await step('save-close-blocked', 900);
      await belt.native.smokeSaves({ broken: false });
      const stay = document.querySelector('.scrim [data-k="no"]');
      if (stay) stay.click();   // "Keep String open": tries again at once
      check('saves: once the disk works again, nothing is left unsaved', await waitFor(() => !belt.unsaved().length, 15000), belt.unsaved());
      let disk = '';
      for (let i = 0; i < 40 && !disk.includes(plName); i++) { await sleep(250); disk = await onDisk(); }
      check('saves: …and the change reached state.json', disk.includes(plName));
      N.S.playlists = N.S.playlists.filter(p => p.id !== 'smoke-unsaved');
      N.save();
    } catch (e) { fail++; log('ERROR', (e && e.stack) || String(e)); }
    log('result', `${pass} passed, ${fail} failed`);
    st.step = 'done';
    st.done = true;
  };
})();
