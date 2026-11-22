// Replaces selftest.js only in the measurement run's private copy of the published app.
window.smokeState = { step: 'measure', done: false, log: [] };
window.smokeTest = async () => {
  const st = window.smokeState, { lib: L, player: P, ui: U, viz } = window.N;
  const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
  const wait = async (fn, description, ms = 30000) => {
    const until = Date.now() + ms;
    while (!await fn()) {
      if (Date.now() >= until) throw new Error(description);
      await sleep(80);
    }
  };
  let data;
  try {
    await N.boot;
    const paths = await belt.native.paths();
    data = paths.data;
    const settings = JSON.parse(await belt.native.readText(data + '\\measure.json'));
    const write = (name, obj) => belt.native.writeText(data + '\\' + name + '.json', JSON.stringify(obj));
    const checkpoint = () => ({ playing: P.playing, visible: belt.visible, document: document.visibilityState,
      route: U.route(), nowPlaying: document.querySelector('#np.open #npMode button.on')?.dataset.k || null,
      frames: viz.frames, time: P.time(), track: P.current()?.id, nodes: document.getElementsByTagName('*').length,
      runningAnimations: document.getAnimations().filter(a => a.playState === 'running').map(a => a.animationName) });
    async function sample(name) {
      if (name !== 'minimised') st.step = name;
      // Give the scan, navigation and their temporary allocations time to settle.
      await sleep(settings.settleSeconds * 1000);
      const begin = checkpoint();
      const moving = ['playing', 'albums', 'record', 'lyrics', 'minimised'].includes(name);
      if (begin.playing !== moving || begin.visible !== (name !== 'minimised'))
        throw new Error('Unexpected playback or window state: ' + name);
      if ((name === 'record' || name === 'lyrics') && begin.nowPlaying !== name ||
          name === 'albums' && (begin.route !== 'albums' || begin.nowPlaying))
        throw new Error('Unexpected screen: ' + name);
      await write(name, begin);
      await sleep(settings.seconds * 1000);
      await wait(async () => (await belt.native.stat(data + '\\' + name + '.ack')).exists,
        'sampler did not finish ' + name, 60000);
      const end = checkpoint();
      if (begin.playing !== end.playing || begin.visible !== end.visible || begin.track !== end.track ||
          begin.nowPlaying !== end.nowPlaying || begin.route !== end.route ||
          (begin.playing && end.time <= begin.time) || (moving && begin.visible && begin.frames === end.frames))
        throw new Error('Phase changed or stalled: ' + name);
      if (name === 'minimised' && (begin.document !== 'hidden' || end.document !== 'hidden' || begin.frames !== end.frames))
        throw new Error('Minimised window drew a frame');
      await write(name + '-end', end);
    }
    N.S.muted = true;
    P.volumeChanged();
    U.addRoots([paths.testMedia]);
    await wait(() => !L.scanning && L.tracks.size === 1200 && L.albums.size === 100, 'incomplete scan', 120000);
    const albums = [...L.albums.values()].sort((a, b) => a.name.localeCompare(b.name));
    const go = route => U.go(route, { replace: true });
    go('albums'); await sleep(1000);
    go('songs'); await sleep(1000);
    go('stats'); await sleep(1000);
    go('album/' + encodeURIComponent(albums[0].key));
    const play = document.querySelector('#view .play-big').getBoundingClientRect();
    await belt.native.smokeClick(play.left + play.width / 2, play.top + play.height / 2);
    await wait(() => P.playing && P.time() > .5 && P.audio().routed, 'playback failed');
    go('songs');
    await sample('playing');
    P.pause();
    await sample('paused');
    P.seek(0);
    await P.play();
    await write('minimise', true);
    await wait(() => !belt.visible, 'window did not minimise');
    await sample('minimised');
    await write('restore', true);
    await wait(() => belt.visible, 'window did not restore');
    P.pause();
    for (const album of albums) {
      go('album/' + encodeURIComponent(album.key));
      await wait(() => document.querySelectorAll('#view .art img.in').length >= 3, 'album covers missing: ' + album.name);
    }
    go('albums');
    await sample('browsed');
    P.seek(0);
    await P.play();
    await sample('albums');
    U.openNP('record'); P.seek(0);
    await sample('record');
    U.openNP('lyrics'); P.seek(0);
    await wait(() => document.querySelector('#lyScroll.synced .ll.on'), 'synced lyrics missing');
    await sample('lyrics');
    U.closeNP();
    P.pause();
    if (settings.longHidden) {
      await P.play();
      P.setLoop(2, 3);
      P.seek(2);
      await belt.native.smokeMinimise(true);
      await wait(() => !belt.visible && document.hidden, 'WebView did not hide');
      const start = performance.now(), frames = viz.frames;
      let previous = start, position = P.time(), wraps = 0, worst = 0, lateMax = 0;
      const late = [];
      await new Promise(resolve => {
        const timer = setInterval(() => {
          const now = performance.now(), t = P.time();
          if (now - start > 300000) {
            late.push(now - previous);
            if (t < position - .5) wraps++;
            lateMax = Math.max(lateMax, t);
          }
          previous = now; position = t;
          worst = Math.max(worst, t);
          if (now - start >= 315000) { clearInterval(timer); resolve(); }
        }, 30);
      });
      late.sort((a, b) => a - b);
      const [state, listening] = await belt.native.stats(['state.json', 'listening.json'].map(f => data + '\\' + f));
      let presence;
      await wait(async () => {
        // A new presence update can replace the file between the host's stat and read.
        const text = await belt.native.readText(data + '\\presence.json').catch(() => null);
        if (text) presence = JSON.parse(text);
        return !!presence;
      }, 'Could not read hidden playback presence', 1500);
      const result = { seconds: (performance.now() - start) / 1000, lateTicks: late.length, lateWraps: wraps,
        lateP95ms: late[Math.floor(late.length * .95)], lateMaxMs: late.at(-1), maxPosition: worst, lateMaxPosition: lateMax,
        stateAgeMs: Date.now() - state.mtime, listeningAgeMs: Date.now() - listening.mtime, presenceAgeMs: Date.now() - presence.at,
        frames: viz.frames - frames, document: document.visibilityState };
      await write('long-hidden', result);
      if (late.length < 350 || wraps < 12 || result.lateP95ms > 65 || worst > 3.15 || result.frames)
        throw new Error('Hidden timing failed: ' + JSON.stringify(result));
      if (result.stateAgeMs > 20000 || result.listeningAgeMs > 25000 || result.presenceAgeMs > 2500 ||
          presence.state !== 'playing' || presence.title !== P.current().T) throw new Error('Stale hidden playback data');
    }
    st.log.push('100 albums, 1200 tracks');
  } catch (e) {
    st.log.push(String(e.stack || e));
    if (data) await belt.native.writeText(data + '\\measure-error.txt', st.log.join('\n'));
  } finally {
    if (!belt.visible) await belt.native.smokeMinimise(false).catch(() => {});
    st.done = true;
  }
};
