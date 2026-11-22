/* String visualizer: one animation loop for the small spectrum in the player bar and the fluid ring round the record in
   Now Playing. It runs only while something can be seen moving (String in view, music playing or still settling after
   a pause), so String minimized or paused costs nothing. Everything is drawn in the tint colour. */
(function () {
  'use strict';
  const N = window.N;
  const V = N.viz = { frames: 0, beats: 0 };
  const BANDS = 64;
  const raw = new Float32Array(BANDS), lvl = new Float32Array(BANDS), slow = new Float32Array(BANDS);
  let analyser = null, freq = null, edges = null, raf = 0, last = 0;
  let bass = 0, bassAvg = .2, pulse = 0, energy = 0, lastBeat = 0, spin = 0, spinV = 0;
  let hi = '236 238 242', hiAt = 0;
  const parts = [];
  const want = { mini: true, big: false };

  V.attach = (a, ctx) => {
    analyser = a;
    freq = new Uint8Array(a.frequencyBinCount);
    // log-spaced bands from 35 Hz to 15 kHz
    const binHz = ctx.sampleRate / a.fftSize;
    edges = Array.from({ length: BANDS + 1 }, (_, i) => Math.max(1, Math.round(35 * Math.pow(15000 / 35, i / BANDS) / binHz)));
    V.wake();
  };
  V.show = (which, on) => { want[which] = on; if (which === 'big' && !on) parts.length = 0; V.wake(); };
  V.wake = () => { if (!raf && belt.visible !== false && document.visibilityState !== 'hidden') { last = performance.now(); raf = requestAnimationFrame(frame); } };
  V.stop = () => { if (raf) cancelAnimationFrame(raf); raf = 0; };
  belt.onHide(V.stop);
  belt.onShow(V.wake);
  document.addEventListener('visibilitychange', () => document.visibilityState === 'hidden' ? V.stop() : V.wake());

  function read(dt) {
    const playing = N.player && N.player.playing;
    if (analyser && playing) {
      analyser.getByteFrequencyData(freq);
      for (let b = 0; b < BANDS; b++) {
        let m = 0;
        const a = edges[b], z = Math.max(a + 1, edges[b + 1]);
        for (let i = a; i < z && i < freq.length; i++) if (freq[i] > m) m = freq[i];
        // treble reads quieter than bass: lift it a little so the ring isn't lopsided
        raw[b] = Math.pow(m / 255, 1.5) * (1 + b / BANDS * .55);
      }
    } else raw.fill(0);
    const k = Math.min(1, dt * 60);
    let e = 0;
    for (let b = 0; b < BANDS; b++) {
      const r = raw[b];
      lvl[b] += (r - lvl[b]) * (r > lvl[b] ? .55 : .11) * k;
      slow[b] += (lvl[b] - slow[b]) * .06 * k;
      e += lvl[b];
    }
    energy = e / BANDS;
    bass = (lvl[0] + lvl[1] + lvl[2] + lvl[3] + lvl[4]) / 5;
    bassAvg += (bass - bassAvg) * .02 * k;
    const now = performance.now();
    if (playing && bass > .28 && bass > bassAvg * 1.32 && now - lastBeat > 230) { lastBeat = now; pulse = 1; V.beats++; emit(); }
    pulse = Math.max(0, pulse - dt * 3.2);
    // the record: turns while playing, coasts to a stop on pause (the analyser isn't read then: the mixer may be asleep)
    spinV += ((playing ? 40 : 0) - spinV) * Math.min(1, dt * (playing ? 1.6 : .9));
    spin = (spin + spinV * dt) % 360;
  }

  function color() {
    const now = performance.now();
    if (now - hiAt > 1000) { hiAt = now; hi = getComputedStyle(document.documentElement).getPropertyValue('--hi').trim() || hi; }
    return a => `rgb(${hi} / ${a})`;
  }
  // each canvas's size on screen, kept by a ResizeObserver instead of measured every frame (a layout read per frame);
  // canvases that left the page are let go when a new one comes
  const sizes = new WeakMap(), watched = new Set();
  const ro = window.ResizeObserver ? new ResizeObserver(es => {
    for (const e of es) sizes.set(e.target, { w: e.contentRect.width, h: e.contentRect.height });
  }) : null;
  function cssSize(cv) {
    let s = ro && sizes.get(cv);
    if (s) return s;
    const r = cv.getBoundingClientRect();
    s = { w: r.width, h: r.height };
    if (ro) {
      for (const old of watched) if (!old.isConnected) { ro.unobserve(old); watched.delete(old); }
      sizes.set(cv, s);
      watched.add(cv);
      ro.observe(cv);
    }
    return s;
  }
  function fit(cv) {
    const r = cssSize(cv), dpr = Math.min(2, devicePixelRatio || 1);
    const w = Math.round(r.w * dpr), h = Math.round(r.h * dpr);
    if (w && h && (cv.width !== w || cv.height !== h)) { cv.width = w; cv.height = h; }
    return { w: cv.width, h: cv.height, dpr };
  }

  /* ---------- the player bar's little spectrum ---------- */
  function drawMini(c) {
    const cv = document.getElementById('mini');
    if (!cv || !cv.isConnected) return; { const z = cssSize(cv); if (!z.w || !z.h) return; }   // hidden: nothing to draw
    const { w, h } = fit(cv), g = cv.getContext('2d');
    g.clearRect(0, 0, w, h);
    const bars = 18, gap = w / bars, bw = Math.max(2, gap * .42);
    for (let i = 0; i < bars; i++) {
      const b = Math.min(BANDS - 1, Math.floor(Math.pow(i / bars, 1.25) * BANDS * .82));
      const v = Math.max(.05, lvl[b]);
      const bh = Math.max(2, v * h * .92);
      g.fillStyle = c(.25 + v * .7);
      g.fillRect(i * gap + (gap - bw) / 2, h - bh, bw, bh);
    }
  }

  /* ---------- Now Playing: a fluid ring round the record, ticks like the HUD, sparks on the beat ---------- */
  function emit() {
    if (!want.big) return;
    for (let i = 0; i < 9; i++) {
      const a = Math.random() * Math.PI * 2;
      parts.push({ a, r: 1.02 + Math.random() * .05, v: .18 + Math.random() * .35, life: 1, decay: .45 + Math.random() * .5, s: .6 + Math.random() * 1.5 });
    }
    if (parts.length > 220) parts.splice(0, parts.length - 220);
  }
  function ring(g, cx, cy, R, amp, src, rot, wob, t) {
    const n = 128, pts = [];
    for (let i = 0; i < n; i++) {
      const a = i / n * Math.PI * 2;
      // the spectrum runs down the right side and mirrors up the left
      const u = i / n, f = u <= .5 ? u * 2 : (1 - u) * 2;
      const b = Math.min(BANDS - 1, Math.floor(f * (BANDS - 1)));
      const v = src[b] * .85 + src[Math.min(BANDS - 1, b + 1)] * .15;
      const r = R + amp * v + Math.sin(a * 3 + t * .9 + wob) * R * .012 + Math.sin(a * 5 - t * .6) * R * .008;
      pts.push([cx + Math.cos(a + rot) * r, cy + Math.sin(a + rot) * r]);
    }
    g.beginPath();
    for (let i = 0; i < n; i++) {
      const p0 = pts[(i - 1 + n) % n], p1 = pts[i], p2 = pts[(i + 1) % n], p3 = pts[(i + 2) % n];
      if (i === 0) g.moveTo(p1[0], p1[1]);
      g.bezierCurveTo(p1[0] + (p2[0] - p0[0]) / 6, p1[1] + (p2[1] - p0[1]) / 6, p2[0] - (p3[0] - p1[0]) / 6, p2[1] - (p3[1] - p1[1]) / 6, p2[0], p2[1]);
    }
    g.closePath();
  }
  function drawBig(c, t, dt) {
    const cv = document.getElementById('bigviz');
    if (!cv || !cv.isConnected) return; { const z = cssSize(cv); if (!z.w || !z.h) return; }   // hidden: nothing to draw
    const { w, h, dpr } = fit(cv), g = cv.getContext('2d');
    g.clearRect(0, 0, w, h);
    const cx = w / 2, cy = h / 2, R = w * .2574;   // the record is 70% of the stage; the canvas is 136% of it
    const rot = t * .05;
    // a soft glow that breathes with the music
    const glow = g.createRadialGradient(cx, cy, R * .9, cx, cy, R * 1.9);
    glow.addColorStop(0, c(.06 + energy * .22 + pulse * .08));
    glow.addColorStop(1, c(0));
    g.fillStyle = glow;
    g.fillRect(0, 0, w, h);
    g.globalCompositeOperation = 'lighter';
    // outer, slower ring
    ring(g, cx, cy, R * 1.08, R * .55, slow, -rot * .7, 2, t);
    g.lineWidth = 1 * dpr; g.strokeStyle = c(.22); g.stroke();
    // the main ring, filled faintly, edged bright
    ring(g, cx, cy, R * 1.045, R * .42, lvl, rot, 0, t);
    g.fillStyle = c(.05 + energy * .08); g.fill();
    g.lineWidth = 1.6 * dpr; g.strokeStyle = c(.55 + energy * .4); g.shadowColor = c(.9); g.shadowBlur = 12 * dpr; g.stroke();
    g.shadowBlur = 0;
    // HUD ticks round the edge
    const ticks = 96;
    g.lineWidth = 1 * dpr;
    for (let i = 0; i < ticks; i++) {
      const u = i / ticks, f = u <= .5 ? u * 2 : (1 - u) * 2, v = lvl[Math.min(BANDS - 1, Math.floor(f * (BANDS - 1)))];
      const a = u * Math.PI * 2 - rot * .35, r0 = R * 1.62, r1 = r0 + R * (.025 + v * .16);
      g.strokeStyle = c(i % 8 === 0 ? .4 : .14 + v * .45);
      g.beginPath(); g.moveTo(cx + Math.cos(a) * r0, cy + Math.sin(a) * r0); g.lineTo(cx + Math.cos(a) * r1, cy + Math.sin(a) * r1); g.stroke();
    }
    // sparks thrown off on the beat
    for (let i = parts.length - 1; i >= 0; i--) {
      const p = parts[i];
      p.r += p.v * dt; p.life -= p.decay * dt; p.a += dt * .08;
      if (p.life <= 0) { parts.splice(i, 1); continue; }
      const x = cx + Math.cos(p.a) * R * p.r, y = cy + Math.sin(p.a) * R * p.r;
      g.fillStyle = c(p.life * .8);
      g.beginPath(); g.arc(x, y, p.s * dpr, 0, Math.PI * 2); g.fill();
    }
    g.globalCompositeOperation = 'source-over';
  }

  function frame(now) {
    raf = 0;
    const dt = Math.min(.1, (now - last) / 1000);
    last = now;
    read(dt);
    const c = color(), t = now / 1000;
    if (want.mini) drawMini(c);
    if (want.big) {
      drawBig(c, t, dt);
      const sp = document.getElementById('spin'), disc = document.getElementById('disc');
      if (sp) sp.style.transform = `rotate(${spin.toFixed(2)}deg)`;
      if (disc) disc.style.setProperty('--pulse', (1 + pulse * .022 + energy * .02).toFixed(4));
    }
    V.frames++;
    const playing = N.player && N.player.playing;
    // after a pause: the bars fall, and in Now Playing the record coasts to a stop and the last sparks fade
    const settling = energy > .002 || (want.big && (pulse > 0 || parts.length > 0 || spinV > .5));
    if ((playing || settling) && (want.mini || want.big)) raf = requestAnimationFrame(frame);
    else if (!playing && want.mini) drawMini(c);
  }
  V.running = () => !!raf;
  V.level = () => energy;
  N.on('state', V.wake);
})();
