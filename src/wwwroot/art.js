/* String art: covers for songs, albums and playlists.
   A cover comes from (in order) a picture picked in Edit info, the picture inside the song (cut straight out of the file
   with a Range request), or a cover.jpg / folder.jpg next to it. Songs with none get a generated pattern, drawn in the
   tint colour, so the library never shows grey boxes. Pictures are shrunk once per session and kept as blob URLs. */
(function () {
  'use strict';
  const N = window.N = window.N || {};
  const SIZES = { sm: 176, md: 420, lg: 960 };
  const LIMIT = { sm: 128, md: 40, lg: 4 };
  const cache = { sm: new Map(), md: new Map(), lg: new Map() };
  const esc = s => String(s ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));

  function hash(s) {
    let h = 2166136261;
    for (let i = 0; i < s.length; i++) { h ^= s.charCodeAt(i); h = Math.imul(h, 16777619); }
    return h >>> 0;
  }
  function rng(seed) {
    let a = seed || 1;
    return () => { a |= 0; a = a + 0x6d2b79f5 | 0; let t = Math.imul(a ^ a >>> 15, 1 | a); t = t + Math.imul(t ^ t >>> 7, 61 | t) ^ t; return ((t ^ t >>> 14) >>> 0) / 4294967296; };
  }
  const f1 = n => (Math.round(n * 10) / 10).toString();
  function initials(label) {
    const w = String(label || '').replace(/[^\p{L}\p{N}\s]/gu, ' ').trim().split(/\s+/).filter(Boolean);
    return ((w[0] || '?')[0] + (w[1] ? w[1][0] : '')).toUpperCase();
  }

  /* ---------- generated covers: five monochrome patterns, picked and shaped by the name ---------- */
  const genMemo = new Map();
  function gen(seed, label, small) {
    const key = seed + '|' + (small ? 1 : 0);
    if (genMemo.has(key)) return genMemo.get(key);
    const h = hash(String(seed || '?')), r = rng(h), kind = h % 5;
    let body = '';
    if (kind === 0) {             // grooves round a point off the middle
      const cx = 25 + r() * 50, cy = 25 + r() * 50;
      for (let q = 3 + r() * 3; q < 120; q += 2.6 + r() * 4.5) body += `<circle cx="${f1(cx)}" cy="${f1(cy)}" r="${f1(q)}"/>`;
      body += `<circle class="f" cx="${f1(cx)}" cy="${f1(cy)}" r="2.2"/>`;
    } else if (kind === 1) {      // stacked waves
      const f = .04 + r() * .09, amp = 2 + r() * 7;
      for (let y = 6; y < 104; y += 5.5) {
        const ph = r() * 6.3, a = amp * (.4 + r() * .9);
        let d = `M-2 ${f1(y)}`;
        for (let x = 0; x <= 104; x += 4) d += ` L${x} ${f1(y + Math.sin(x * f + ph + y * .07) * a)}`;
        body += `<path d="${d}"/>`;
      }
    } else if (kind === 2) {      // a dot field that swells toward one point
      const cx = r() * 100, cy = r() * 100, s = 6 + Math.floor(r() * 3);
      for (let y = s / 2; y < 100; y += s) for (let x = s / 2; x < 100; x += s) {
        const d = Math.hypot(x - cx, y - cy), q = Math.max(.25, (s * .42) - d / 26);
        body += `<circle class="f" cx="${f1(x)}" cy="${f1(y)}" r="${f1(q)}"/>`;
      }
    } else if (kind === 3) {      // diagonal bands of changing width
      const ang = [35, -35, 55, -55, 90, 0][(h >>> 4) % 6];
      let g = '';
      for (let x = -60; x < 160; x += 1.5 + r() * 6) {
        g += r() < .22 ? `<rect class="f" x="${f1(x)}" y="-60" width="${f1(1 + r() * 4)}" height="220"/>` : `<path d="M${f1(x)} -60 V160"/>`;
      }
      body += `<g transform="rotate(${ang} 50 50)">${g}</g>`;
    } else {                      // orbits: arcs round the middle
      const cx = 50 + (r() - .5) * 20, cy = 50 + (r() - .5) * 20;
      for (let i = 0; i < 11; i++) {
        const q = 8 + i * 4.4 + r() * 2, a0 = r() * Math.PI * 2, a1 = a0 + .6 + r() * 3.6;
        const x0 = cx + q * Math.cos(a0), y0 = cy + q * Math.sin(a0), x1 = cx + q * Math.cos(a1), y1 = cy + q * Math.sin(a1);
        body += `<path d="M${f1(x0)} ${f1(y0)} A${f1(q)} ${f1(q)} 0 ${a1 - a0 > Math.PI ? 1 : 0} 1 ${f1(x1)} ${f1(y1)}"/>`;
        if (r() < .5) body += `<circle class="f" cx="${f1(x1)}" cy="${f1(y1)}" r="1.3"/>`;
      }
    }
    const text = small ? '' : `<text x="7" y="93">${esc(initials(label))}</text>`;
    const svg = `<svg class="gen" viewBox="0 0 100 100" preserveAspectRatio="xMidYMid slice" aria-hidden="true">${body}${text}</svg>`;
    if (genMemo.size > 3000) genMemo.clear();
    genMemo.set(key, svg);
    return svg;
  }

  /* ---------- where a track's picture is ---------- */
  function source(t) {
    if (!t) return null;
    const e = N.S && N.S.edits && N.S.edits[t.id];
    if (e && e.art) return { key: 'f:' + e.art.toLowerCase(), path: e.art };
    if (t.art) return { key: 'e:' + t.id + ':' + t.mtime, track: t };
    if (t.cover) return { key: 'f:' + t.cover.toLowerCase(), path: t.cover };
    return null;
  }
  async function rawBlob(src) {
    if (src.path) {
      const r = await fetch(belt.native.mediaUrl(src.path), { cache: 'no-store' });
      return r.ok ? r.blob() : null;
    }
    const t = src.track, url = belt.native.mediaUrl(t.path);
    if (t.art.o != null) {
      const r = await fetch(url, { headers: { Range: `bytes=${t.art.o}-${t.art.o + t.art.n - 1}` }, cache: 'no-store' });
      if (!r.ok) return null;
      let b = new Uint8Array(await r.arrayBuffer());
      if (r.status === 200) b = b.subarray(t.art.o, t.art.o + t.art.n);
      return new Blob([b], { type: t.art.m || 'image/jpeg' });
    }
    const a = await StringTags.readArt(N.rangeReader(url), t.size);
    return a ? new Blob([a.bytes], { type: a.mime }) : null;
  }
  // Only two full-size bitmaps at a time, even when a grid of covers comes into view.
  const decoders = [Promise.resolve(), Promise.resolve()];
  let nextDecoder = 0;
  function thumbnail(blob, px) {
    const slot = nextDecoder;
    nextDecoder = 1 - slot;
    const task = decoders[slot].then(() => shrink(blob, px));
    decoders[slot] = task.then(() => {}, () => {});
    return task;
  }
  async function shrink(blob, px) {
    const bmp = await createImageBitmap(blob);
    let c;
    try {
      const s = Math.min(bmp.width, bmp.height), out = Math.min(px, s);
      if (!out) return null;
      c = new OffscreenCanvas(out, out);
      const g = c.getContext('2d');
      g.imageSmoothingQuality = 'high';
      g.drawImage(bmp, (bmp.width - s) / 2, (bmp.height - s) / 2, s, s, 0, 0, out, out);
    } finally { bmp.close(); }
    try { return await c.convertToBlob({ type: 'image/jpeg', quality: .88 }); }
    finally { c.width = 0; }
  }

  const inflight = new Map(), signatures = new Map();
  async function readPicture(src) {
    const blob = await rawBlob(src);
    if (!blob) return null;
    let key = src.key;
    // Songs often embed identical covers. Share a thumbnail only for identical picture bytes.
    if (src.track && globalThis.crypto?.subtle) {
      const hash = await crypto.subtle.digest('SHA-256', await blob.arrayBuffer());
      key = 'pic:' + Array.from(new Uint8Array(hash), b => b.toString(16).padStart(2, '0')).join('');
    }
    return { key, blob };
  }
  function recent(m, key) {
    const p = m.get(key);
    m.delete(key); m.set(key, p);
    return p;
  }
  async function url(t, size = 'sm') {
    const src = source(t);
    if (!src) return null;
    const m = cache[size], known = signatures.get(src.key) || src.key;
    if (m.has(known)) return recent(m, known);
    let read = inflight.get(src.key);
    if (!read) {
      read = { result: readPicture(src).catch(() => null), cancelled: false };
      inflight.set(src.key, read);
    }
    const picture = await read.result;
    if (read.cancelled) return null;
    if (inflight.get(src.key) === read) {
      inflight.delete(src.key);
      if (picture && src.track) {
        signatures.set(src.key, picture.key);
        if (signatures.size > 2048) signatures.delete(signatures.keys().next().value);
      }
    }
    if (!picture) return null;
    const key = picture.key;
    if (m.has(key)) return recent(m, key);
    const p = thumbnail(picture.blob, SIZES[size]).then(b => b && URL.createObjectURL(b)).catch(() => null);
    m.set(key, p);
    while (m.size > LIMIT[size]) {
      const [k, old] = m.entries().next().value;
      m.delete(k);
      old.then(u => { if (u) setTimeout(() => URL.revokeObjectURL(u), 4000); });
    }
    return p;
  }
  // forget a track's picture (after Edit info changes it)
  function forget(t) {
    const src = source(t);
    if (src) {
      signatures.delete(src.key);
      const read = inflight.get(src.key);
      if (read) { read.cancelled = true; inflight.delete(src.key); }
    }
    for (const m of Object.values(cache)) for (const k of [...m.keys()]) {
      if (!((src && k === src.key) || k.startsWith('e:' + t.id + ':'))) continue;
      const old = m.get(k);
      m.delete(k);
      // its picture's memory goes too, a moment later (as when the cache lets one go), so nothing still showing it breaks
      old.then(u => { if (u) setTimeout(() => URL.revokeObjectURL(u), 4000); });
    }
  }

  /* ---------- the DOM side: <div class="art" data-art="<track id>"> fills itself when it scrolls into view ---------- */
  function html(t, seed, label, size = 'sm', cls = '', lazyPattern = false) {
    const small = size === 'sm' && !/\bbig\b/.test(cls);
    const lazy = lazyPattern && t && io;
    const pattern = lazy ? ` data-seed="${esc(seed)}" data-label="${esc(label)}"` : '';
    return `<div class="art ${cls}"${t ? ` data-art="${esc(t.id)}" data-size="${size}"` : ''}${pattern}>${lazy ? '' : gen(seed, label, small)}</div>`;
  }
  const io = 'IntersectionObserver' in window ? new IntersectionObserver(es => {
    for (const e of es) if (e.isIntersecting) { io.unobserve(e.target); load(e.target); }
  }, { rootMargin: '400px 0px' }) : null;
  async function load(el) {
    if (!el.isConnected) return;
    if (el.hasAttribute('data-seed')) {
      el.insertAdjacentHTML('afterbegin', gen(el.dataset.seed, el.dataset.label, false));
      delete el.dataset.seed; delete el.dataset.label;
    }
    const id = el.dataset.art, t = N.lib && N.lib.tracks.get(id);
    if (!t || el.dataset.loaded === id) return;
    el.dataset.loaded = id;
    const u = await url(t, el.dataset.size || 'sm');
    if (!u || !el.isConnected || el.dataset.art !== id) return;
    let img = el.querySelector('img');
    if (!img) { img = document.createElement('img'); img.alt = ''; img.decoding = 'async'; img.draggable = false; el.appendChild(img); }
    img.onload = () => img.classList.add('in');
    img.src = u;
  }
  function fill(root) {
    root.querySelectorAll('.art[data-art]').forEach(el => {
      if (el.dataset.loaded === el.dataset.art) return;
      if (io) io.observe(el); else load(el);
    });
  }
  // point an existing .art at another track (the player bar, Now Playing)
  function set(el, t, size, seed, label) {
    const id = t ? t.id : '';
    if (el.dataset.art === id && el.dataset.loaded === id) return;
    const keep = el.querySelector('.open-np');
    el.innerHTML = gen(seed || id || 'none', label || '', size === 'sm' && el.offsetWidth < 90);
    if (keep) el.appendChild(keep);
    delete el.dataset.loaded;
    if (t) { el.dataset.art = id; el.dataset.size = size; load(el); } else delete el.dataset.art;
  }

  N.art = { url, html, gen, fill, set, forget, hash, source };
})();
