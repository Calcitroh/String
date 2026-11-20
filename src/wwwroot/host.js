/* String host bridge: window.belt, the page's calls to String's own window (src\App.cs), over WebView2's message
   channel. Also: an in-page toast, and the glyph set.
     belt.load() / belt.save(data | fn) state.json in the data folder (save is debounced; flushed before closing;
                                        every write is answered, and one that fails is kept and tried again)
     belt.store(name, data, ms)         <name>.json in the data folder (listening stats), debounced by ms, flushed on close
     belt.toast(text), belt.setTitle(text), belt.openExternal(url), belt.onFlush/onShow/onHide(fn), belt.visible
     belt.icon(name), belt.ready (resolves to the host's hello: { smoke })
     belt.onMedia(fn)                   the taskbar thumbnail buttons: fn('prev' | 'toggle' | 'next')
     belt.onLook(fn)                    a file in the look folder changed (look.css, its pictures)
     belt.setLook(on), belt.reloadLook() the person's look.css on or off (kept for the next start); loaded afresh
     belt.setPlaying(on)                tells the taskbar whether to show play or pause
     belt.native: paths, features, run, stat, stats, list, readText, writeText, pickFolder, pickFile, open, reveal, lookFolder,
                  dropped(files), mediaUrl(path), smokeClick(x, y), smokeDrop(paths, x, y), smokeThumb(i)
   Opened in a plain browser (no host), load/save fall back to localStorage and belt.native throws. */
(() => {
  'use strict';
  const ICONS = {
    belt: '<rect x="2.5" y="10" width="19" height="4" rx=".6"/><rect x="8" y="7" width="6" height="10" rx="1.4"/><path d="M8 12h5"/>',
    plus: '<path d="M12 5v14M5 12h14"/>',
    x: '<path d="M6 6l12 12M18 6L6 18"/>',
    check: '<path d="M5 12l5 5 9-10"/>',
    reload: '<path d="M20 11a8 8 0 1 0-2.3 5.7"/><path d="M20 4v7h-7"/>',
    sliders: '<path d="M4 6h9M17 6h3M4 12h3M11 12h9M4 18h11M19 18h1"/><circle cx="15" cy="6" r="2"/><circle cx="9" cy="12" r="2"/><circle cx="17" cy="18" r="2"/>',
    folder: '<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>',
    external: '<path d="M14 4h6v6M20 4l-9 9"/><path d="M18 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5"/>',
    power: '<path d="M12 3v8"/><path d="M6.3 6.8a8 8 0 1 0 11.4 0"/>',
    grid: '<rect x="4" y="4" width="6.5" height="6.5"/><rect x="13.5" y="4" width="6.5" height="6.5"/><rect x="4" y="13.5" width="6.5" height="6.5"/><rect x="13.5" y="13.5" width="6.5" height="6.5"/>',
    pencil: '<path d="M15.5 4.5l4 4L8 20H4v-4z"/><path d="M13 7l4 4"/>',
    note: '<path d="M6 3h9l4 4v14H6z"/><path d="M14 3v5h5M9 12h7M9 16h5"/>',
    list: '<path d="M9 6h11M9 12h11M9 18h11"/><path d="M4.5 6h.01M4.5 12h.01M4.5 18h.01" stroke-width="2.6"/>',
    checklist: '<path d="M4 6l1.5 1.5L8 5M4 12l1.5 1.5L8 11M4 18l1.5 1.5L8 17"/><path d="M11 6h9M11 12h9M11 18h9"/>',
    timer: '<circle cx="12" cy="13.5" r="7.5"/><path d="M12 13.5V9.5M10 2.5h4M12 2.5V6M18.5 6.5l1.5-1.5"/>',
    clock: '<circle cx="12" cy="12" r="8.5"/><path d="M12 7v5l3 2"/>',
    calendar: '<rect x="4" y="5" width="16" height="15" rx="1"/><path d="M4 10h16M9 3v4M15 3v4M8 14h2M14 14h2M8 17h2"/>',
    calc: '<rect x="5" y="3" width="14" height="18" rx="1"/><path d="M8 7h8M8.5 12h.01M12 12h.01M15.5 12h.01M8.5 15.5h.01M12 15.5h.01M15.5 15.5h.01M8.5 19h.01M12 19h.01" stroke-width="2.2"/>',
    chart: '<path d="M4 20V4M4 20h16"/><path d="M8 16v-4M12 16V8M16 16v-6"/>',
    pulse: '<path d="M3 12h4l2-5 4 10 2-5h6"/>',
    bolt: '<path d="M13 3L5 13.5h6L10 21l8-10.5h-6z"/>',
    flame: '<path d="M12 21c-3.9 0-6.5-2.6-6.5-6.2 0-3.6 3-5.6 3.6-9.3 2.4 1.4 3.7 3.5 3.9 5.4 1-.8 1.5-2 1.6-3.1 2 1.8 3.9 4.2 3.9 7 0 3.6-2.6 6.2-6.5 6.2z"/>',
    moon: '<path d="M20 14.5A8 8 0 1 1 9.5 4a6.5 6.5 0 0 0 10.5 10.5z"/>',
    sun: '<circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4"/>',
    bell: '<path d="M6 16V11a6 6 0 0 1 12 0v5l1.5 2h-15z"/><path d="M10 20.5a2 2 0 0 0 4 0"/>',
    star: '<path d="M12 3.5l2.6 5.3 5.9.9-4.3 4.1 1 5.8L12 16.9l-5.2 2.7 1-5.8-4.3-4.1 5.9-.9z"/>',
    heart: '<path d="M12 20s-7.5-4.4-7.5-10A4.3 4.3 0 0 1 12 7.3 4.3 4.3 0 0 1 19.5 10c0 5.6-7.5 10-7.5 10z"/>',
    book: '<path d="M4 5.5A2.5 2.5 0 0 1 6.5 3H20v15H6.5A2.5 2.5 0 0 0 4 20.5z"/><path d="M4 20.5A2.5 2.5 0 0 0 6.5 23H20v-5"/>',
    code: '<path d="M8.5 7L3.5 12l5 5M15.5 7l5 5-5 5M13.5 4.5l-3 15"/>',
    terminal: '<rect x="3" y="4.5" width="18" height="15" rx="1"/><path d="M7 9.5l3 2.5-3 2.5M12.5 15h4.5"/>',
    globe: '<circle cx="12" cy="12" r="8.5"/><path d="M3.5 12h17M12 3.5c2.4 2.3 3.6 5.1 3.6 8.5s-1.2 6.2-3.6 8.5c-2.4-2.3-3.6-5.1-3.6-8.5s1.2-6.2 3.6-8.5z"/>',
    link: '<path d="M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1"/><path d="M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1"/>',
    search: '<circle cx="10.5" cy="10.5" r="6"/><path d="M15 15l5 5"/>',
    music: '<path d="M9 18V5l11-2v13"/><circle cx="6.5" cy="18" r="2.5"/><circle cx="17.5" cy="16" r="2.5"/>',
    headphones: '<path d="M4 15v-3a8 8 0 0 1 16 0v3"/><rect x="3" y="14" width="4.5" height="6.5" rx="1"/><rect x="16.5" y="14" width="4.5" height="6.5" rx="1"/>',
    mic: '<rect x="9" y="3" width="6" height="11" rx="3"/><path d="M5.5 11a6.5 6.5 0 0 0 13 0M12 17.5V21M9 21h6"/>',
    image: '<rect x="3.5" y="4.5" width="17" height="15" rx="1"/><circle cx="9" cy="10" r="1.8"/><path d="M20.5 16l-5-5-8.5 8.5"/>',
    camera: '<path d="M4 8h3.5l1.5-2.5h6L16.5 8H20v11H4z"/><circle cx="12" cy="13.2" r="3.4"/>',
    film: '<rect x="3.5" y="4" width="17" height="16" rx="1"/><path d="M7.5 4v16M16.5 4v16M3.5 8h4M3.5 12h4M3.5 16h4M16.5 8h4M16.5 12h4M16.5 16h4"/>',
    game: '<path d="M7 8h10a4 4 0 0 1 4 4v1.5a3.5 3.5 0 0 1-6.3 2.1L14 14.5h-4l-.7 1.1A3.5 3.5 0 0 1 3 13.5V12a4 4 0 0 1 4-4z"/><path d="M8 10.5v3M6.5 12h3M15.5 11h.01M17.5 13h.01" stroke-width="2"/>',
    dice: '<rect x="4" y="4" width="16" height="16" rx="2.5"/><path d="M8.5 8.5h.01M15.5 8.5h.01M12 12h.01M8.5 15.5h.01M15.5 15.5h.01" stroke-width="2.6"/>',
    cup: '<path d="M5 8h11v6a5 5 0 0 1-5 5h-1a5 5 0 0 1-5-5z"/><path d="M16 9.5h1.5a2.5 2.5 0 0 1 0 5H16M8 3.5v2M11 3.5v2"/>',
    leaf: '<path d="M5 19c0-8 5-13.5 15-14-.5 10-6 15-14 15"/><path d="M5 19l7-7"/>',
    key: '<circle cx="8" cy="15" r="4"/><path d="M11 12l8.5-8.5M16 7l2.5 2.5M14 9l2 2"/>',
    lock: '<rect x="5" y="11" width="14" height="10" rx="1"/><path d="M8 11V7a4 4 0 0 1 8 0v4"/>',
    shield: '<path d="M12 3l7.5 3v5.5c0 4.6-3.2 8.3-7.5 9.5-4.3-1.2-7.5-4.9-7.5-9.5V6z"/>',
    mail: '<rect x="3.5" y="5.5" width="17" height="13" rx="1"/><path d="M4 6.5l8 6.5 8-6.5"/>',
    chat: '<path d="M4 5h16v11H9l-5 4z"/>',
    user: '<circle cx="12" cy="8.5" r="4"/><path d="M4.5 20.5c1.2-3.8 4-5.5 7.5-5.5s6.3 1.7 7.5 5.5"/>',
    home: '<path d="M4 11l8-7 8 7v9H4z"/><path d="M10 20v-6h4v6"/>',
    map: '<path d="M3.5 6.5L9 4l6 2.5L20.5 4v13.5L15 20l-6-2.5-5.5 2.5z"/><path d="M9 4v13.5M15 6.5V20"/>',
    pin: '<path d="M12 21s-6.5-6-6.5-11a6.5 6.5 0 0 1 13 0c0 5-6.5 11-6.5 11z"/><circle cx="12" cy="10" r="2.3"/>',
    cart: '<path d="M3 4h2.5l2 11h11l2-8H6.5"/><circle cx="9" cy="19.5" r="1.3"/><circle cx="17" cy="19.5" r="1.3"/>',
    coin: '<circle cx="12" cy="12" r="8.5"/><path d="M14.8 9.2c-.6-.9-1.6-1.4-2.8-1.4-1.6 0-2.8.9-2.8 2.1 0 2.9 5.6 1.5 5.6 4.3 0 1.2-1.2 2.1-2.8 2.1-1.2 0-2.3-.5-2.9-1.4M12 6v1.8M12 16.2V18"/>',
    gift: '<rect x="4" y="9" width="16" height="11.5"/><path d="M3 9h18M12 9v11.5M12 9C9.5 9 7.5 7.8 7.5 6.2S9.7 3.9 12 9c2.3-5.1 4.5-4.4 4.5-2.8S14.5 9 12 9z"/>',
    box: '<path d="M3.5 7.5L12 3l8.5 4.5v9L12 21l-8.5-4.5z"/><path d="M3.5 7.5L12 12l8.5-4.5M12 12v9"/>',
    gear: '<circle cx="12" cy="12" r="3"/><path d="M12 2.5v3M12 18.5v3M2.5 12h3M18.5 12h3M5.3 5.3l2.1 2.1M16.6 16.6l2.1 2.1M5.3 18.7l2.1-2.1M16.6 7.4l2.1-2.1"/>',
    wrench: '<path d="M14.5 6.5a4 4 0 0 0 5 5l-9.5 9.5a2.1 2.1 0 0 1-3-3L16.5 8.5"/><path d="M14.5 6.5a4 4 0 0 1 5-3l-2.5 2.5.5 2 2 .5 2.5-2.5a4 4 0 0 1-3 5"/>',
    ruler: '<path d="M3 16.5L16.5 3 21 7.5 7.5 21z"/><path d="M7 12.5l1.8 1.8M9.8 9.7l1.8 1.8M12.6 6.9l1.8 1.8"/>',
    scissors: '<circle cx="6" cy="7" r="2.6"/><circle cx="6" cy="17" r="2.6"/><path d="M8.2 8.4L20 17M8.2 15.6L20 7"/>',
    palette: '<path d="M12 3.5a8.5 8.5 0 0 0 0 17c1.3 0 1.9-.9 1.6-2-.4-1.4.5-2.6 2-2.6H18a2.5 2.5 0 0 0 2.5-2.5c0-5.5-3.8-9.9-8.5-9.9z"/><path d="M7.5 11.5h.01M10 7.5h.01M15 7.5h.01" stroke-width="2.4"/>',
    flask: '<path d="M9.5 3h5M10 3v6.5L4.5 19a1.3 1.3 0 0 0 1.1 2h12.8a1.3 1.3 0 0 0 1.1-2L14 9.5V3"/><path d="M7.5 15h9"/>',
    sparkle: '<path d="M12 3.5l1.9 5.6 5.6 1.9-5.6 1.9L12 18.5l-1.9-5.6L4.5 11l5.6-1.9z"/><path d="M19 17v4M17 19h4"/>',
    plug: '<path d="M9 3v5M15 3v5M6.5 8h11v3.5a5.5 5.5 0 0 1-11 0zM12 17v4"/>',
    download: '<path d="M12 4v11M7 10l5 5 5-5M5 20h14"/>',
    upload: '<path d="M12 20V9M7 14l5-5 5 5M5 4h14"/>',
    save: '<path d="M5 4h11l3 3v13H5z"/><path d="M8 4v5h7V4M8 20v-6h8v6"/>',
    trash: '<path d="M3 6h18M8 6V4h8v2M6 6l1 14h10l1-14M10 10.5v6M14 10.5v6"/>',
    play: '<path d="M8 5.5v13l10.5-6.5z"/>',
    pause: '<path d="M8.5 5.5v13M15.5 5.5v13"/>',
    stop: '<rect x="6.5" y="6.5" width="11" height="11"/>',
    flag: '<path d="M5.5 21V4M5.5 4.5h11.5l-2.5 4 2.5 4H5.5"/>',
  };
  const icon = n => `<svg viewBox="0 0 24 24" aria-hidden="true">${ICONS[n] || ICONS.box}</svg>`;
  const wv = window.chrome && window.chrome.webview;
  const inHost = !!wv;
  let seq = 0, saveTimer = 0, pending, visible = true, helloDone;
  const waiting = new Map(), flushers = [], showers = [], hiders = [], media = [], looks = [];
  let playingSent = null;
  const stores = new Map();   // name -> { data, timer }
  const ready = new Promise(r => { helloDone = r; });
  const send = (kind, extra) => { if (inHost) wv.postMessage({ kind, ...extra }); };
  const ask = (kind, extra) => new Promise(res => { const req = String(++seq); waiting.set(req, res); send(kind, { ...extra, req }); });
  const each = list => list.forEach(fn => { try { fn(); } catch (e) { console.error(e); } });

  /* ---------- the person's own look: <data>\look\look.css, served at /_look/v<n>/look.css after String's own styles
     (added here, not in index.html, so a person's edited index.html survives updates). It loads in the background, so
     a slow look (an @import from the web) never holds up the player. Each reload gets a new v<n> in the address, so
     pictures it names by file name (url(logo.png)) load fresh as well. Only the newest load takes over (the sheet in
     use carries id="lookCss" until then), and one that fails keeps the last good look. "Off" is remembered in
     localStorage: a look that is off isn't loaded at all at the next start. */
  const lookOffKey = 'string.lookOff';
  const hasPage = typeof document !== 'undefined';   // the Node tests load this file without a page
  let lookLatest = null, lookRev = Date.now();
  function lookLoad() {
    if (!hasPage) return;
    const l = document.createElement('link');
    l.rel = 'stylesheet'; l.dataset.look = '1'; l.href = '/_look/v' + (++lookRev) + '/look.css';   // never the same address twice
    lookLatest = l;
    const settle = ok => {
      if (l !== lookLatest) { l.remove(); return; }                                // a newer one is on its way
      if (!ok && document.getElementById('lookCss')) { l.remove(); return; }       // keep the last good look
      document.querySelectorAll('link[data-look]').forEach(x => { if (x !== l) x.remove(); });
      l.id = 'lookCss';
    };
    l.onload = () => settle(true);
    l.onerror = () => settle(false);
    document.head.appendChild(l);
  }
  function lookOn(on) {
    if (!hasPage) return;
    if (!on) { lookLatest = null; document.querySelectorAll('link[data-look]').forEach(l => l.remove()); return; }
    if (!document.querySelector('link[data-look]')) lookLoad();
  }
  let lookOff = false;
  try { lookOff = localStorage.getItem(lookOffKey) === '1'; } catch { }
  if (!lookOff) lookLoad();

  /* ---------- saving: every write is answered ("saved"). One that didn't reach the disk (a full or locked drive) keeps
     its data here, says so once, and is tried again every 10 s until it works; String then says the changes are saved.
     Closing with something unsaved asks first (closeBlocked). */
  const unacked = new Map();   // name -> { seq, data }: sent, not answered yet
  const failing = new Map();   // name -> why its last write failed
  let saveSeq = 0, retryTimer = 0, closingSoon = false;   // closingSoon: String is closing, so a change saves at once
  const savedFns = [], closeBlockedFns = [], updateFns = [];
  function sendSave(name, data) {
    const seq = ++saveSeq;
    unacked.set(name, { seq, data });
    if (name === 'state') send('save', { data, seq }); else send('store', { name, data, seq });
  }
  function retryFailed() {
    clearTimeout(retryTimer);
    retryTimer = 0;
    flushSave();
    [...stores.keys()].forEach(flushStore);
  }
  function onSaved(m) {
    const u = unacked.get(m.name);
    if (!u || u.seq !== m.seq) return;          // a newer write of it is on its way: that one's answer counts
    unacked.delete(m.name);
    if (m.ok) {
      const was = failing.size;
      failing.delete(m.name);
      if (was && !failing.size) { toast('Your changes are saved again'); each(savedFns); }
      return;
    }
    if (m.refused) return;                      // not yet allowed (the library is still opening): nothing to keep
    // keep it, unless something newer is already waiting to go
    if (m.name === 'state') { if (pending === undefined) pending = u.data; }
    else { let s = stores.get(m.name); if (!s) stores.set(m.name, s = { data: undefined, timer: 0 }); if (s.data === undefined) s.data = u.data; }
    if (!failing.size) toast(`String can't save right now (${m.error || 'the disk said no'}). Your changes are kept, and it keeps trying.`);
    failing.set(m.name, m.error || '');
    if (!retryTimer) retryTimer = setTimeout(retryFailed, 10000);
  }
  function flushSave() {
    clearTimeout(saveTimer);
    if (pending === undefined) return;
    let data = pending; pending = undefined;
    if (typeof data === 'function') data = data();   // made only now, not at every change (see belt.save)
    if (inHost) sendSave('state', data);
    else try { localStorage.setItem('string:state', JSON.stringify(data)); } catch { }
  }
  function flushStore(name) {
    const s = stores.get(name);
    if (!s || s.data === undefined) return;
    clearTimeout(s.timer);
    s.timer = 0;
    const data = s.data; s.data = undefined;
    if (inHost) sendSave(name, data);
    else try { localStorage.setItem('string:' + name, JSON.stringify(data)); } catch { }
  }

  /* ---------- toast: a line at the bottom, above the player bar ---------- */
  let toastEl = null, toastTimer = 0;
  function toast(text) {
    if (!hasPage) { console.log('toast: ' + text); return; }   // the Node tests
    if (!toastEl) {
      toastEl = document.createElement('div');
      toastEl.id = 'hostToast';
      toastEl.setAttribute('role', 'status');
      document.body.appendChild(toastEl);
    }
    toastEl.innerHTML = `<i></i><b>String</b><span></span>`;
    toastEl.querySelector('span').textContent = String(text);
    toastEl.classList.remove('on'); void toastEl.offsetWidth; toastEl.classList.add('on');
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => toastEl.classList.remove('on'), 3200);
  }

  const belt = {
    app: { id: 'string', name: 'String' }, inBelt: inHost, icon, icons: Object.keys(ICONS), ready,
    get visible() { return visible; },
    async load() {
      if (!inHost) return { state: JSON.parse(localStorage.getItem('string:state')), listening: JSON.parse(localStorage.getItem('string:listening')) };
      await ready;
      const profile = await ask('load');
      if (profile.error) throw new Error(profile.error);
      return profile;
    },
    profileReady() { send('profileReady'); },
    // data, or a function that makes it: called once when the save goes out, so changes in quick succession don't each
    // build the whole snapshot (String's queue and library state)
    save(data) { pending = typeof data === 'function' ? data : JSON.parse(JSON.stringify(data ?? null)); clearTimeout(saveTimer); if (closingSoon) flushSave(); else saveTimer = setTimeout(flushSave, 300); },
    unsaved: () => [...failing.keys()],          // names whose last write failed
    onSaved(fn) { savedFns.push(fn); },          // everything that failed is saved again
    onCloseBlocked(fn) { closeBlockedFns.push(fn); },   // closing waits: something isn't saved (fn gets why)
    closeAnyway() { send('closeAnyway'); },
    onUpdate(fn) { updateFns.push(fn); },        // an update downloading: { ev: 'progress', got, total } or { ev: 'verifying' }
    retrySaves: retryFailed,
    store(name, data, ms = 0) {
      let s = stores.get(name);
      if (!s) stores.set(name, s = { data: undefined, timer: 0 });
      s.data = data;
      if (ms === 0 || closingSoon) flushStore(name);
      else if (!s.timer) s.timer = setTimeout(() => flushStore(name), ms);
    },
    toast,
    setTitle(text) { send('title', { text: String(text ?? '') }); },
    setBadge() { }, attention() { },
    openExternal(url) { if (inHost) send('open', { url: String(url) }); else window.open(url, '_blank'); },
    onFlush(fn) { flushers.push(fn); },
    onShow(fn) { showers.push(fn); },
    onHide(fn) { hiders.push(fn); },
    onMedia(fn) { media.push(fn); },
    onLook(fn) { looks.push(fn); },   // something in the look folder changed (saved, added, removed)
    // the person's own look on or off (remembered for the next start)
    setLook(on) {
      on = !!on;
      try { localStorage.setItem(lookOffKey, on ? '0' : '1'); } catch { }
      lookOn(on);
    },
    // load it and its pictures again, if it's on (never changes whether it is)
    reloadLook() { if (hasPage && document.querySelector('link[data-look]')) lookLoad(); },
    setPlaying(on) { on = !!on; if (on !== playingSent) { playingSent = on; send('playing', { on }); } },
  };

  /* ---------- belt.native: files, folders, programs ---------- */
  const procs = new Map(), early = new Map();
  async function nat(op, args) {
    if (!inHost) throw new Error('Only in String');
    await ready;
    const r = await ask('native', { op, args: args || {} });
    if (!r || !r.ok) throw new Error((r && r.error) || 'That did not work');
    return r.value;
  }
  function deliver(m) {
    const p = procs.get(m.proc);
    if (!p) { if (!early.has(m.proc)) early.set(m.proc, []); early.get(m.proc).push(m); return; }
    if (m.ev === 'line') { try { if (p.onLine) p.onLine(m.text, m.stream); } catch (e) { console.error(e); } }
    else if (m.ev === 'exit') p.finish(m.code);
  }
  belt.native = {
    paths: () => nat('paths'),
    features: () => nat('features').catch(() => ({})),
    async run(exe, args = [], { onLine, onExit } = {}) {
      const id = await nat('run', { exe, args: args.map(String) });
      let finish;
      const done = new Promise(r => { finish = r; });
      procs.set(id, { onLine, finish: code => { procs.delete(id); try { if (onExit) onExit(code); } catch (e) { console.error(e); } finish(code); } });
      (early.get(id) || []).forEach(deliver);
      early.delete(id);
      return { id, done, kill: () => nat('kill', { proc: id }).catch(() => { }) };
    },
    stat: path => nat('stat', { path }),
    stats: paths => nat('stats', { paths }),
    list: path => nat('list', { path }),
    readText: path => nat('readText', { path }),
    writeText: (path, text) => nat('writeText', { path, text: String(text) }),
    copy: (src, dst) => nat('copy', { src, dst }),   // dst inside the data folder only
    recycle: paths => nat('recycle', { paths }),     // song files to the Recycle Bin (the Duplicates page): { done, failed }
    navidromePlays: (after, user = '') => nat('navidromePlays', { after, user }),
    replaceSong: (path, data) => nat('replaceSong', { path, data }),   // a downloaded song with its names written in (base64): { path, backup }
    mkdir: path => nat('mkdir', { path }),
    pickFolder: (opts = {}) => nat('pickFolder', opts),
    pickFile: (opts = {}) => nat('pickFile', opts),
    open: path => nat('open', { path }),
    reveal: path => nat('reveal', { path }),
    // the person's own look folder (<data>\look\): made with a starter look.css the first time, then shown in
    // Explorer (quiet: not shown): { path, made }
    lookFolder: (opts = {}) => nat('lookFolder', opts),
    async dropped(fileList) {
      const files = [...(fileList || [])].filter(f => f instanceof File);
      if (!files.length || !inHost) return [];
      await ready;
      const r = await new Promise(res => { const req = String(++seq); waiting.set(req, res); wv.postMessageWithAdditionalObjects({ kind: 'native', op: 'dropped', req }, files); });
      return r && r.ok ? r.value : [];
    },
    // an address that plays or shows a sound or picture from anywhere on the PC; seekable (Range)
    mediaUrl(path) {
      const p = String(path || '').replace(/\\/g, '/');
      return '/_media/' + p.split('/').map(encodeURIComponent).join('/');
    },
    smokeClick: (x, y) => nat('smokeClick', { x, y }),
    smokeDrop: (paths, x = 200, y = 200) => nat('smokeDrop', { paths, x, y }),
    smokeThumb: i => nat('smokeThumb', { i }),
    // String's own updates (Updater.cs): this version and whether it can update itself; what GitHub has; install one
    updateInfo: () => nat('updateInfo'),
    updateCheck: () => nat('updateCheck'),
    updateInstall: ({ version }) => nat('updateInstall', { version: String(version || '') }),
    smokeUpdate: ({ feed }) => nat('smokeUpdate', { feed: String(feed) }),   // self-test: another fake release feed
    smokeSaves: ({ broken }) => nat('smokeSaves', { broken: !!broken }),   // self-test: saves fail (as on a full disk), or work again
    smokeClose: () => nat('smokeClose'),                                    // self-test: close the window as a person would
    // LRCLIB lyrics (asked by the host, only its /api/ addresses): { status, body }
    lrclib: path => nat('lrclib', { path }),
    // Ollama on this PC (started by the host if it isn't running): the downloaded models, and a streamed chat whose
    // reply lines (Ollama's NDJSON) arrive through onLine like a program's output; kill() stops it
    ollamaModels: () => nat('ollamaModels'),
    async ollamaChat(request, { onLine, onExit } = {}) {
      const id = await nat('ollamaChat', { body: JSON.stringify(request) });
      let finish;
      const done = new Promise(r => { finish = r; });
      procs.set(id, { onLine, finish: code => { procs.delete(id); try { if (onExit) onExit(code); } catch (e) { console.error(e); } finish(code); } });
      (early.get(id) || []).forEach(deliver);
      early.delete(id);
      return { id, done, kill: () => nat('kill', { proc: id }).catch(() => { }) };
    },
  };
  window.belt = belt;

  /* ---------- add-ons: <data>\addons\<name>\ (string.js says what they can do). Their styles (before the person's
     own look, which stays on top), then their scripts in order, once the page's own scripts have run; String's start
     waits for them (belt.addonsReady). One that fails to load is skipped. ---------- */
  belt.addonsReady = (async () => {
    if (!inHost || !hasPage) return [];
    let list = [];
    try { list = await nat('addons'); } catch { return []; }
    if (document.readyState === 'loading') await new Promise(r => document.addEventListener('DOMContentLoaded', r, { once: true }));
    const at = (id, f) => `/_addon/${encodeURIComponent(id)}/${String(f).split('/').map(encodeURIComponent).join('/')}`;
    for (const a of list) {
      for (const f of a.styles || []) {
        const l = document.createElement('link');
        l.rel = 'stylesheet'; l.href = at(a.id, f); l.dataset.addon = a.id;
        document.head.insertBefore(l, document.querySelector('link[data-look]'));
      }
      for (const f of a.scripts || []) {
        await new Promise(res => { const s = document.createElement('script'); s.src = at(a.id, f); s.dataset.addon = a.id; s.onload = s.onerror = res; document.head.appendChild(s); });
      }
    }
    return list;
  })();
  if (!inHost) { helloDone({}); return; }

  wv.addEventListener('message', e => {
    const m = e.data;
    if (!m || typeof m !== 'object') return;
    switch (m.kind) {
      case 'hello': helloDone(m); break;
      case 'reply': { const res = waiting.get(m.req); waiting.delete(m.req); if (res) res(m.data); break; }
      case 'proc': deliver(m); break;
      case 'flush': closingSoon = true; each(flushers); flushSave(); [...stores.keys()].forEach(flushStore); send('flushed'); break;
      case 'show': if (!visible) { visible = true; each(showers); } break;
      case 'hide': if (visible) { visible = false; each(hiders); } break;
      case 'media': media.forEach(fn => { try { fn(m.action); } catch (e) { console.error(e); } }); break;
      case 'look': looks.forEach(fn => { try { fn(); } catch (e) { console.error(e); } }); break;
      case 'saved': onSaved(m); break;
      case 'update': updateFns.forEach(fn => { try { fn(m); } catch (e) { console.error(e); } }); break;
      case 'closeBlocked': closingSoon = false; closeBlockedFns.forEach(fn => { try { fn(m.error || ''); } catch (e) { console.error(e); } }); break;
    }
  });
  addEventListener('pagehide', () => { flushSave(); [...stores.keys()].forEach(flushStore); });
  send('hello');
})();
