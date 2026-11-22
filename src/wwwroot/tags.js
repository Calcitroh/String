/* String tags: title, artist, album, length and where the cover picture is, read from the first bytes of a song (and
   sometimes the last), never the whole file. Pure: give it read(start, length) -> Promise<Uint8Array> and the file size.
   Formats: MP3 (ID3v2.2/2.3/2.4, ID3v1, Xing/Info/VBRI lengths), FLAC, MP4/M4A, Ogg Vorbis and Opus, WAV (LIST INFO, id3).
     readTags(read, size)  -> { title, artist, album, albumArtist, track, disc, year, genre, duration (s), art, format }
                              art: { o, n, m } (bytes o..o+n are the picture, m its type), { p: 1 } (needs readArt), or null
     readArt(read, size)   -> { bytes, mime } or null: the cover itself, for the art that can't be cut straight from the file
   Works in the page (window.StringTags) and in Node (module.exports), where tests\test-tags.js checks it. */
(function (root) {
  'use strict';
  const dec = enc => { try { return new TextDecoder(enc); } catch { return new TextDecoder('utf-8'); } };
  const U8 = dec('utf-8'), L1 = dec('windows-1252'), LE16 = dec('utf-16le'), BE16 = dec('utf-16be');
  const GENRES = ('Blues|Classic Rock|Country|Dance|Disco|Funk|Grunge|Hip-Hop|Jazz|Metal|New Age|Oldies|Other|Pop|R&B|Rap|Reggae|' +
    'Rock|Techno|Industrial|Alternative|Ska|Death Metal|Pranks|Soundtrack|Euro-Techno|Ambient|Trip-Hop|Vocal|Jazz+Funk|Fusion|' +
    'Trance|Classical|Instrumental|Acid|House|Game|Sound Clip|Gospel|Noise|Alternative Rock|Bass|Soul|Punk|Space|Meditative|' +
    'Instrumental Pop|Instrumental Rock|Ethnic|Gothic|Darkwave|Techno-Industrial|Electronic|Pop-Folk|Eurodance|Dream|' +
    'Southern Rock|Comedy|Cult|Gangsta|Top 40|Christian Rap|Pop/Funk|Jungle|Native American|Cabaret|New Wave|Psychedelic|Rave|' +
    'Showtunes|Trailer|Lo-Fi|Tribal|Acid Punk|Acid Jazz|Polka|Retro|Musical|Rock & Roll|Hard Rock').split('|');

  /* ---------- nearby fields share a small read; large fields are read in one request ---------- */
  function reader(read, size) {
    let start = 0, data = new Uint8Array(0);
    async function bytes(o, n) {
      o = Math.max(0, Math.floor(o)); n = Math.max(0, Math.min(Math.floor(n), size - o));
      if (!n) return new Uint8Array(0);
      if (o < start || o + n > start + data.length) {
        const b = await read(o, Math.min(size - o, Math.max(8192, n)));
        start = o;
        data = b instanceof Uint8Array ? b : new Uint8Array(b);
      }
      return data.subarray(o - start, o - start + n);
    }
    return { size, bytes };
  }

  const ascii = (b, o, n) => { let s = ''; for (let i = 0; i < n && o + i < b.length; i++) s += String.fromCharCode(b[o + i]); return s; };
  const be32 = (b, o) => ((b[o] << 24) >>> 0) + (b[o + 1] << 16) + (b[o + 2] << 8) + b[o + 3];
  const be24 = (b, o) => (b[o] << 16) + (b[o + 1] << 8) + b[o + 2];
  const be16 = (b, o) => (b[o] << 8) + b[o + 1];
  const le32 = (b, o) => (b[o] + (b[o + 1] << 8) + (b[o + 2] << 16) + ((b[o + 3] << 24) >>> 0)) >>> 0;
  const le16 = (b, o) => b[o] + (b[o + 1] << 8);
  const syncsafe = (b, o) => ((b[o] & 0x7f) << 21) | ((b[o + 1] & 0x7f) << 14) | ((b[o + 2] & 0x7f) << 7) | (b[o + 3] & 0x7f);
  const be64 = (b, o) => be32(b, o) * 4294967296 + be32(b, o + 4);
  const le64 = (b, o) => le32(b, o + 4) * 4294967296 + le32(b, o);
  const clean = s => (s || '').replace(/\0+$/g, '').replace(/\0/g, '; ').trim();
  const imgType = (b, o) => b[o] === 0xff && b[o + 1] === 0xd8 ? 'image/jpeg' : b[o] === 0x89 && b[o + 1] === 0x50 ? 'image/png'
    : ascii(b, o, 4) === 'RIFF' ? 'image/webp' : ascii(b, o, 3) === 'GIF' ? 'image/gif' : ascii(b, o, 2) === 'BM' ? 'image/bmp' : '';

  function genre(g) {
    g = clean(g);
    const m = g.match(/^\((\d+)\)\s*(.*)$/);
    if (m) return m[2] || GENRES[+m[1]] || '';
    if (/^\d+$/.test(g)) return GENRES[+g] || '';
    return g;
  }
  const num = v => { const m = String(v || '').match(/\d+/); return m ? +m[0] : 0; };
  const year = v => { const m = String(v || '').match(/\b(1[89]\d\d|2\d\d\d)\b/); return m ? +m[1] : 0; };

  function blank(format) {
    return { format, title: '', artist: '', album: '', albumArtist: '', track: 0, disc: 0, year: 0, genre: '', duration: 0, art: null };
  }

  /* ---------- ID3v2 ---------- */
  function textOf(b, o, end, enc) {
    const s = b.subarray(o, end);
    if (enc === 0) return L1.decode(s);
    if (enc === 3) return U8.decode(s);
    if (enc === 2) return BE16.decode(s);
    if (s[0] === 0xfe && s[1] === 0xff) return BE16.decode(s.subarray(2));
    if (s[0] === 0xff && s[1] === 0xfe) return LE16.decode(s.subarray(2));
    return LE16.decode(s);
  }
  // end of a null-terminated string in this encoding (index of the terminator)
  function strEnd(b, o, end, enc) {
    if (enc === 1 || enc === 2) { for (let i = o; i + 1 < end; i += 2) if (b[i] === 0 && b[i + 1] === 0) return i; return end; }
    for (let i = o; i < end; i++) if (b[i] === 0) return i; return end;
  }
  const unsync = b => {
    const out = new Uint8Array(b.length);
    let n = 0;
    for (let i = 0; i < b.length; i++) { out[n++] = b[i]; if (b[i] === 0xff && b[i + 1] === 0x00) i++; }
    return out.subarray(0, n);
  };
  const ID3_22 = { TT2: 'title', TP1: 'artist', TP2: 'albumArtist', TAL: 'album', TRK: 'track', TPA: 'disc', TYE: 'year', TDA: '', TCO: 'genre', TLE: 'tlen' };
  const ID3_23 = { TIT2: 'title', TPE1: 'artist', TPE2: 'albumArtist', TALB: 'album', TRCK: 'track', TPOS: 'disc', TYER: 'year', TDRC: 'year', TDOR: 'year', TCON: 'genre', TLEN: 'tlen' };

  // reads the tag at `at` into t; returns the offset just past it. wantArt: also return the picture bytes
  async function id3v2(r, at, t, wantArt) {
    const h = await r.bytes(at, 10);
    if (ascii(h, 0, 3) !== 'ID3') return at;
    const ver = h[3], flags = h[5], size = syncsafe(h, 6);
    const end = at + 10 + size + (ver === 4 && flags & 0x10 ? 10 : 0);
    if (ver < 2 || ver > 4) return end;
    let base = at + 10, buf = null;   // buf: the whole tag body, when it must be read at once (unsynchronised v2.2/2.3)
    if (flags & 0x80 && ver < 4) {
      buf = unsync(await r.bytes(base, Math.min(size, 16 << 20)));
    }
    const get = async (o, n) => buf ? buf.subarray(o - base, o - base + n) : r.bytes(o, n);
    let p = base;
    const limit = base + (buf ? buf.length : size);
    if (flags & 0x40 && ver > 2) {
      const x = await get(p, 4);
      p += ver === 4 ? syncsafe(x, 0) : be32(x, 0) + 4;
    }
    let best = null, pic = null;
    const hl = ver === 2 ? 6 : 10;
    while (p + hl <= limit) {
      const fh = await get(p, hl);
      if (fh[0] === 0) break;   // padding
      const id = ascii(fh, 0, ver === 2 ? 3 : 4);
      if (!/^[A-Z0-9]{3,4}$/.test(id)) break;
      const fsize = ver === 2 ? be24(fh, 3) : ver === 4 ? syncsafe(fh, 4) : be32(fh, 4);
      const fflags = ver === 2 ? 0 : be16(fh, 8);
      let o = p + hl, n = fsize;
      p = o + fsize;
      if (fsize <= 0 || p > limit + 4) break;
      const compressed = ver === 3 ? fflags & 0x80 : ver === 4 ? fflags & 0x08 : 0;
      const encrypted = ver === 3 ? fflags & 0x40 : ver === 4 ? fflags & 0x04 : 0;
      if (compressed || encrypted) continue;
      let frameUnsync = false;
      if (ver === 3 && fflags & 0x20) { o += 1; n -= 1; }   // grouping byte
      if (ver === 4) {
        if (fflags & 0x40) { o += 1; n -= 1; }               // grouping byte
        if (fflags & 0x01) { o += 4; n -= 4; }               // data length indicator
        frameUnsync = !!(fflags & 0x02) || !!(flags & 0x80);
      }
      const isPic = id === 'APIC' || id === 'PIC';
      const key = ver === 2 ? ID3_22[id] : ID3_23[id];
      if (isPic) {
        let head = await get(o, Math.min(n, 512));
        if (frameUnsync) head = unsync(head);
        const enc = head[0];
        let q, mime;
        if (id === 'PIC') { const f = ascii(head, 1, 3).toUpperCase(); mime = f === 'PNG' ? 'image/png' : 'image/jpeg'; q = 4; }
        else { const me = strEnd(head, 1, head.length, 0); mime = ascii(head, 1, me - 1).toLowerCase(); q = me + 1; }
        const kind = head[q];
        const de = strEnd(head, q + 1, head.length, enc);
        const dataAt = de + (enc === 1 || enc === 2 ? 2 : 1);
        if (dataAt >= head.length && n > 512) continue;   // a description longer than we looked at: rare, skip
        if (!mime || mime === '-->' || mime.indexOf('/') < 0) mime = imgType(head, dataAt) || (mime === 'png' ? 'image/png' : 'image/jpeg');
        const cand = { o: o + dataAt, n: n - dataAt, m: mime, direct: !buf && !frameUnsync, frameUnsync, kind };
        if (!best || (kind === 3 && best.kind !== 3)) best = cand;
        continue;
      }
      // lyrics (only when asked: readLyrics): USLT is encoding, language (3), a description, then the text
      if ((id === 'USLT' || id === 'ULT') && t._wantLyrics) {
        if (n > (1 << 20) || t._lyrics) continue;
        let f = await get(o, n);
        if (frameUnsync) f = unsync(f);
        const enc = f[0], de = strEnd(f, 4, f.length, enc);
        t._lyrics = textOf(f, de + (enc === 1 || enc === 2 ? 2 : 1), f.length, enc).replace(/\0/g, '');
        continue;
      }
      if (key === undefined && id !== 'TXXX') continue;
      if (n > (id === 'TXXX' && t._wantLyrics ? (1 << 20) : 4096)) continue;
      let f = await get(o, n);
      if (frameUnsync) f = unsync(f);
      const enc = f[0];
      if (id === 'TXXX') {
        const de = strEnd(f, 1, f.length, enc);
        const desc = clean(textOf(f, 1, de, enc)).toUpperCase();
        const raw = textOf(f, de + (enc === 1 || enc === 2 ? 2 : 1), f.length, enc);
        // TXXX:LYRICS from taggers; TXXX:USLT is how FFmpeg (and tools built on it) writes a "lyrics" tag into ID3v2.4
        if (/^((UNSYNCED|SYNCED)?LYRICS|USLT)$/.test(desc)) { if (t._wantLyrics && !t._lyrics) t._lyrics = raw.replace(/\0/g, ''); continue; }
        const val = clean(raw);
        if ((desc === 'ALBUM ARTIST' || desc === 'ALBUMARTIST') && !t.albumArtist) t.albumArtist = val;
        continue;
      }
      if (!key) continue;
      const v = clean(textOf(f, 1, f.length, enc));
      if (!v) continue;
      if (key === 'track' || key === 'disc') { if (!t[key]) t[key] = num(v); }
      else if (key === 'year') { if (!t.year) t.year = year(v); }
      else if (key === 'genre') { if (!t.genre) t.genre = genre(v); }
      else if (key === 'tlen') { if (!t.duration && num(v) > 0) t.duration = num(v) / 1000; }
      else if (!t[key]) t[key] = v;
    }
    if (best) {
      t.art = best.direct ? { o: best.o, n: best.n, m: best.m } : { p: 1 };
      if (wantArt) {
        let bytes = buf ? buf.subarray(best.o - base, best.o - base + best.n) : await r.bytes(best.o, best.n);
        if (best.frameUnsync) bytes = unsync(bytes);
        pic = { bytes, mime: best.m };
      }
    }
    t._pic = pic;
    return end;
  }

  /* ---------- MP3 frames: where the audio starts, and how long it is ---------- */
  const BR = {
    '1-1': [0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448],
    '1-2': [0, 32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384],
    '1-3': [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320],
    '2-1': [0, 32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256],
    '2-2': [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160],
  };
  const SR = { 3: [44100, 48000, 32000], 2: [22050, 24000, 16000], 0: [11025, 12000, 8000] };
  function frameHeader(b, i) {
    if (b[i] !== 0xff || (b[i + 1] & 0xe0) !== 0xe0) return null;
    const v = (b[i + 1] >> 3) & 3, l = (b[i + 1] >> 1) & 3, bi = b[i + 2] >> 4, si = (b[i + 2] >> 2) & 3, pad = (b[i + 2] >> 1) & 1, ch = b[i + 3] >> 6;
    if (v === 1 || l === 0 || bi === 0 || bi === 15 || si === 3) return null;
    const layer = 4 - l, v1 = v === 3;
    const kbps = BR[v1 ? `1-${layer}` : `2-${layer === 1 ? 1 : 2}`][bi], rate = SR[v][si];
    const spf = layer === 1 ? 384 : layer === 2 || v1 ? 1152 : 576;
    const len = layer === 1 ? Math.floor((12 * kbps * 1000 / rate + pad) * 4) : Math.floor(spf / 8 * kbps * 1000 / rate) + pad;
    return { v1, layer, kbps, rate, spf, mono: ch === 3, len };
  }
  async function mpeg(r, start, t) {
    // Most audio starts right after its tags. Keep the full sync search for leading junk.
    let b = await r.bytes(start, 4096);
    const first = frameHeader(b, 0);
    if (!first || first.len < 24 || first.len + 4 > b.length || !frameHeader(b, first.len))
      b = await r.bytes(start, 65536);
    for (let i = 0; i + 4 < b.length; i++) {
      const h = frameHeader(b, i);
      if (!h || h.len < 24) continue;
      const n2 = i + h.len;   // a second frame right after: not a false sync inside junk
      if (n2 + 4 <= b.length && !frameHeader(b, n2)) continue;
      const side = h.v1 ? (h.mono ? 17 : 32) : (h.mono ? 9 : 17);
      const x = i + 4 + side;
      const tag = ascii(b, x, 4);
      if (tag === 'Xing' || tag === 'Info') {
        const fl = be32(b, x + 4);
        if (fl & 1) { const frames = be32(b, x + 8); if (frames) { t.duration = frames * h.spf / h.rate; return; } }
      }
      if (ascii(b, i + 36, 4) === 'VBRI') { const frames = be32(b, i + 36 + 14); if (frames) { t.duration = frames * h.spf / h.rate; return; } }
      if (!t.duration && h.kbps) {
        const tail = r.size >= 128 && ascii(await r.bytes(r.size - 128, 3), 0, 3) === 'TAG' ? 128 : 0;
        t.duration = (r.size - (start + i) - tail) * 8 / (h.kbps * 1000);
      }
      return;
    }
  }
  async function id3v1(r, t) {
    if (r.size < 128) return;
    const b = await r.bytes(r.size - 128, 128);
    if (ascii(b, 0, 3) !== 'TAG') return;
    const s = (o, n) => clean(L1.decode(b.subarray(o, o + n)).replace(/\0.*$/s, ''));
    if (!t.title) t.title = s(3, 30);
    if (!t.artist) t.artist = s(33, 30);
    if (!t.album) t.album = s(63, 30);
    if (!t.year) t.year = year(s(93, 4));
    if (!t.track && b[125] === 0 && b[126]) t.track = b[126];
    if (!t.genre && b[127] < GENRES.length) t.genre = GENRES[b[127]];
  }

  /* ---------- Vorbis comments (FLAC, Ogg) ---------- */
  const VC = { TITLE: 'title', ARTIST: 'artist', ALBUM: 'album', ALBUMARTIST: 'albumArtist', 'ALBUM ARTIST': 'albumArtist',
    TRACKNUMBER: 'track', DISCNUMBER: 'disc', DATE: 'year', YEAR: 'year', GENRE: 'genre' };
  function vorbisComments(b, o, t, picFound) {
    if (o + 8 > b.length) return;
    const vl = le32(b, o); o += 4 + vl;
    if (o + 4 > b.length) return;
    let count = le32(b, o); o += 4;
    while (count-- > 0 && o + 4 <= b.length) {
      const l = le32(b, o); o += 4;
      if (o + l > b.length) { if (picFound) picFound(null); break; }
      const s = b.subarray(o, o + l); o += l;
      let eq = s.indexOf(61);
      if (eq < 0) continue;
      const k = ascii(s, 0, eq).toUpperCase();
      if (k === 'METADATA_BLOCK_PICTURE') { if (picFound) picFound(s.subarray(eq + 1)); continue; }
      if (k === 'LYRICS' || k === 'UNSYNCEDLYRICS' || k === 'SYNCEDLYRICS') { if (t._wantLyrics && !t._lyrics) t._lyrics = U8.decode(s.subarray(eq + 1)); continue; }
      const key = VC[k];
      if (!key) continue;
      const v = clean(U8.decode(s.subarray(eq + 1)));
      if (!v) continue;
      if (key === 'track' || key === 'disc') { if (!t[key]) t[key] = num(v); }
      else if (key === 'year') { if (!t.year) t.year = year(v); }
      else if (key === 'genre') { if (!t.genre) t.genre = genre(v); }
      else if (!t[key]) t[key] = v;
    }
  }
  // a FLAC PICTURE block (also what Ogg keeps base64-encoded): where its picture bytes are, from the block's start
  function flacPicture(b, o) {
    const kind = be32(b, o);
    const ml = be32(b, o + 4), mime = ascii(b, o + 8, ml);
    let q = o + 8 + ml;
    const dl = be32(b, q); q += 4 + dl + 16;
    const n = be32(b, q); q += 4;
    return { kind, mime: mime || imgType(b, q), at: q, n };
  }

  async function flac(r, at, t, wantArt) {
    let p = at + 4, best = null;
    for (let guard = 0; guard < 64 && p + 4 <= r.size; guard++) {
      const h = await r.bytes(p, 4);
      const last = h[0] & 0x80, type = h[0] & 0x7f, len = be24(h, 1);
      const o = p + 4;
      if (type === 0) {
        const s = await r.bytes(o, 18);
        const rate = (s[10] << 12) | (s[11] << 4) | (s[12] >> 4);
        const total = (s[13] & 0x0f) * 4294967296 + be32(s, 14);
        if (rate && total) t.duration = total / rate;
      } else if (type === 4 && len < (4 << 20)) {
        vorbisComments(await r.bytes(o, len), 0, t);
      } else if (type === 6) {
        const head = await r.bytes(o, Math.min(len, 4096));
        const pic = flacPicture(head, 0);
        if (pic.at <= head.length && (!best || (pic.kind === 3 && best.kind !== 3))) best = { o: o + pic.at, n: pic.n, m: pic.mime, kind: pic.kind };
      }
      p = o + len;
      if (last) break;
    }
    if (best) {
      t.art = { o: best.o, n: best.n, m: best.m };
      if (wantArt) t._pic = { bytes: await r.bytes(best.o, best.n), mime: best.m };
    }
  }

  /* ---------- Ogg ---------- */
  async function ogg(r, t, wantArt) {
    // gather the first two packets (identification, comments), page by page
    let p = 0, serial = -1;
    const packets = [];
    let cur = [], curLen = 0, cap = wantArt || t._wantLyrics ? (32 << 20) : (512 << 10), truncated = false;
    for (let guard = 0; guard < 4000 && packets.length < 2 && p + 27 <= r.size; guard++) {
      const h = await r.bytes(p, 27);
      if (ascii(h, 0, 4) !== 'OggS') break;
      const ser = le32(h, 14), nseg = h[26];
      const segs = await r.bytes(p + 27, nseg);
      let o = p + 27 + nseg;
      if (serial < 0) serial = ser;
      let bodyLen = 0;
      for (let i = 0; i < nseg; i++) bodyLen += segs[i];
      if (ser !== serial) { p = o + bodyLen; continue; }
      const body = curLen < cap ? await r.bytes(o, bodyLen) : null;
      let q = 0;
      for (let i = 0; i < nseg && packets.length < 2; i++) {
        const s = segs[i];
        if (body && curLen < cap) { cur.push(body.subarray(q, q + s)); curLen += s; } else truncated = truncated || packets.length === 1;
        q += s;
        if (s < 255) {
          const all = new Uint8Array(Math.min(curLen, cap)); let k = 0;
          for (const c of cur) { const take = Math.min(c.length, all.length - k); all.set(c.subarray(0, take), k); k += take; }
          packets.push(all); cur = []; curLen = 0;
        }
      }
      p = o + bodyLen;
    }
    if (!packets.length) return;
    const id = packets[0];
    let rate = 0, preskip = 0, opus = false;
    if (ascii(id, 1, 6) === 'vorbis') { rate = le32(id, 12); t.format = 'ogg'; }
    else if (ascii(id, 0, 8) === 'OpusHead') { opus = true; rate = 48000; preskip = le16(id, 10); t.format = 'opus'; }
    if (packets[1]) {
      const c = packets[1];
      const start = opus ? (ascii(c, 0, 8) === 'OpusTags' ? 8 : -1) : (ascii(c, 1, 6) === 'vorbis' ? 7 : -1);
      if (start >= 0) {
        let picB64 = null, sawPic = false;
        vorbisComments(c, start, t, s => { sawPic = true; if (s && !picB64) picB64 = s; });
        if (sawPic || truncated) t.art = { p: 1 };
        if (wantArt && picB64) {
          try {
            const raw = b64(ascii(picB64, 0, picB64.length));
            const pic = flacPicture(raw, 0);
            t._pic = { bytes: raw.subarray(pic.at, pic.at + pic.n), mime: pic.mime };
          } catch { }
        }
      }
    }
    // length: the granule position of the stream's last page
    if (rate) {
      const n = Math.min(r.size, 65536 * 2), tail = await r.bytes(r.size - n, n);
      for (let i = tail.length - 27; i >= 0; i--) {
        if (tail[i] === 0x4f && ascii(tail, i, 4) === 'OggS' && le32(tail, i + 14) === serial) {
          const g = le64(tail, i + 6);
          if (g > 0 && g < 1e15) { t.duration = Math.max(0, g - preskip) / rate; break; }
        }
      }
    }
  }
  function b64(s) {
    if (typeof atob === 'function') { const bin = atob(s.replace(/[^A-Za-z0-9+/=]/g, '')); const out = new Uint8Array(bin.length); for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i); return out; }
    return new Uint8Array(Buffer.from(s, 'base64'));
  }

  /* ---------- MP4 / M4A ---------- */
  const MP4 = { '©nam': 'title', '©ART': 'artist', 'aART': 'albumArtist', '©alb': 'album', '©day': 'year', '©gen': 'genre' };
  async function atoms(r, from, to, fn) {
    let p = from;
    for (let guard = 0; guard < 512 && p + 8 <= to; guard++) {
      const h = await r.bytes(p, 16);
      let size = be32(h, 0), hl = 8;
      const type = L1.decode(h.subarray(4, 8));
      if (size === 1) { size = be64(h, 8); hl = 16; }
      else if (size === 0) size = to - p;
      if (size < hl) break;
      const stop = await fn(type, p + hl, p + size);
      if (stop) return true;
      p += size;
    }
    return false;
  }
  async function mp4(r, t, wantArt) {
    t.format = 'm4a';
    await atoms(r, 0, r.size, async (type, o, end) => {
      if (type !== 'moov') return false;
      await atoms(r, o, end, async (type2, o2, end2) => {
        if (type2 === 'mvhd') {
          const b = await r.bytes(o2, 32);
          const v = b[0];
          const scale = v === 1 ? be32(b, 20) : be32(b, 12), dur = v === 1 ? be64(b, 24) : be32(b, 16);
          if (scale) t.duration = dur / scale;
        } else if (type2 === 'udta') {
          await atoms(r, o2, end2, async (type3, o3, end3) => {
            if (type3 !== 'meta') return false;
            const peek = await r.bytes(o3, 8);
            const inner = ascii(peek, 4, 4) === 'hdlr' ? o3 : o3 + 4;   // a full box (version + flags) or the QuickTime kind
            await atoms(r, inner, end3, async (type4, o4, end4) => {
              if (type4 !== 'ilst') return false;
              await atoms(r, o4, end4, async (key, o5, end5) => {
                // each item holds a 'data' atom: size, 'data', type (4), locale (4), value
                const dh = await r.bytes(o5, 16);
                if (ascii(dh, 4, 4) !== 'data') return false;
                const dlen = be32(dh, 0), kind = be32(dh, 8) & 0xffffff, vo = o5 + 16, vn = Math.min(dlen - 16, end5 - vo);
                if (vn <= 0) return false;
                if (key === '©lyr') {
                  if (t._wantLyrics && !t._lyrics && vn < (1 << 20)) t._lyrics = U8.decode(await r.bytes(vo, vn));
                } else if (key === 'covr') {
                  if (!t.art) {
                    const head = await r.bytes(vo, 4);
                    const m = kind === 14 ? 'image/png' : kind === 13 ? 'image/jpeg' : imgType(head, 0) || 'image/jpeg';
                    t.art = { o: vo, n: vn, m };
                    if (wantArt) t._pic = { bytes: await r.bytes(vo, vn), mime: m };
                  }
                } else if (key === 'trkn' || key === 'disk') {
                  const v = await r.bytes(vo, Math.min(vn, 8));
                  t[key === 'trkn' ? 'track' : 'disc'] = be16(v, 2);
                } else if (key === 'gnre') {
                  const v = await r.bytes(vo, 2);
                  if (!t.genre) t.genre = GENRES[be16(v, 0) - 1] || '';
                } else if (MP4[key] && vn < 4096) {
                  const v = clean(U8.decode(await r.bytes(vo, vn)));
                  const f = MP4[key];
                  if (f === 'year') t.year = year(v); else if (f === 'genre') t.genre = genre(v); else if (v) t[f] = v;
                }
                return false;
              });
              return true;
            });
            return true;
          });
        }
        return false;
      });
      return true;
    });
  }

  /* ---------- WAV ---------- */
  const RIFF = { INAM: 'title', IART: 'artist', IPRD: 'album', ICRD: 'year', IGNR: 'genre', ITRK: 'track', IPRT: 'track' };
  async function wav(r, t, wantArt) {
    t.format = 'wav';
    let byteRate = 0, dataLen = 0, p = 12;
    for (let guard = 0; guard < 256 && p + 8 <= r.size; guard++) {
      const h = await r.bytes(p, 12);
      const id = ascii(h, 0, 4), len = le32(h, 4), o = p + 8;
      if (id === 'fmt ') { const f = await r.bytes(o, 16); byteRate = le32(f, 8); }
      else if (id === 'data') dataLen = Math.min(len, r.size - o);
      else if (id === 'LIST' && ascii(h, 8, 4) === 'INFO' && len < (1 << 20)) {
        const b = await r.bytes(o + 4, len - 4);
        for (let q = 0; q + 8 <= b.length;) {
          const k = ascii(b, q, 4), n = le32(b, q + 4);
          const v = clean(U8.decode(b.subarray(q + 8, q + 8 + n)));
          const f = RIFF[k];
          if (f && v) { if (f === 'year') t.year = year(v); else if (f === 'track') t.track = num(v); else if (f === 'genre') t.genre = genre(v); else if (!t[f]) t[f] = v; }
          q += 8 + n + (n & 1);
        }
      } else if (id.toLowerCase() === 'id3 ') await id3v2(r, o, t, wantArt);
      p = o + len + (len & 1);
    }
    if (byteRate && dataLen) t.duration = dataLen / byteRate;
  }

  /* ---------- the front door ---------- */
  async function parse(read, size, wantArt, wantLyrics) {
    const r = reader(read, size);
    const t = blank('');
    t._wantLyrics = !!wantLyrics;
    const h = await r.bytes(0, 12);
    let at = 0;
    if (ascii(h, 0, 3) === 'ID3') {
      t.format = 'mp3';
      at = await id3v2(r, 0, t, wantArt);
      const next = await r.bytes(at, 12);
      // FLAC with an ID3 tag stuck on the front; otherwise MPEG audio (or ADTS AAC) follows
      if (ascii(next, 0, 4) === 'fLaC') { t.format = 'flac'; await flac(r, at, t, wantArt && !t._pic); }
      else if (next[0] === 0xff && (next[1] & 0xf6) === 0xf0) t.format = 'aac';
      else { await mpeg(r, at, t); await id3v1(r, t); }
    } else if (ascii(h, 0, 4) === 'fLaC') { t.format = 'flac'; await flac(r, 0, t, wantArt); }
    else if (ascii(h, 4, 4) === 'ftyp') await mp4(r, t, wantArt);
    else if (ascii(h, 0, 4) === 'OggS') await ogg(r, t, wantArt);
    else if (ascii(h, 0, 4) === 'RIFF' && ascii(h, 8, 4) === 'WAVE') await wav(r, t, wantArt);
    else if (h[0] === 0xff && (h[1] & 0xf6) === 0xf0) t.format = 'aac';
    else if (h[0] === 0x1a && h[1] === 0x45 && h[2] === 0xdf && h[3] === 0xa3) t.format = 'webm';
    else { t.format = 'mp3'; await mpeg(r, 0, t); await id3v1(r, t); }
    if (!isFinite(t.duration) || t.duration < 0) t.duration = 0;
    return t;
  }

  async function readTags(read, size) {
    const t = await parse(read, size, false);
    delete t._pic; delete t._lyrics; delete t._wantLyrics;
    return t;
  }
  // the lyrics kept inside the song (ID3 USLT or a LYRICS TXXX, Vorbis LYRICS / UNSYNCEDLYRICS, MP4 ©lyr), or null;
  // the text as stored: plain, or LRC with [mm:ss.xx] times
  async function readLyrics(read, size) {
    const t = await parse(read, size, false, true);
    const text = (t._lyrics || '').replace(/\r\n?/g, '\n').trim();
    return text ? text : null;
  }
  async function readArt(read, size) {
    const t = await parse(read, size, true);
    if (!t._pic || !t._pic.bytes || !t._pic.bytes.length) return null;
    return { bytes: t._pic.bytes, mime: t._pic.mime || imgType(t._pic.bytes, 0) || 'image/jpeg' };
  }

  const api = { readTags, readArt, readLyrics, GENRES };
  if (typeof module === 'object' && module.exports) module.exports = api;
  else root.StringTags = api;
})(typeof window !== 'undefined' ? window : globalThis);
