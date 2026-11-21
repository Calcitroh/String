// Checks src\wwwroot\tags.js against real files.
//   node tests\test-tags.js            makes test songs with FFmpeg (in test-runs\string-media) and checks them
//   node tests\test-tags.js <folder>   just prints what it reads from every song in that folder (no checks)
// FFmpeg: %FFMPEG% (the path to ffmpeg.exe), else ffmpeg on the PATH.
'use strict';
const fs = require('fs'), path = require('path'), { execFileSync } = require('child_process');
const T = require('../src/wwwroot/tags.js');
const ROOT = path.join(__dirname, '..');
const MEDIA = path.join(ROOT, 'test-runs', 'string-media');
const AUDIO = /\.(mp3|flac|m4a|m4b|aac|ogg|oga|opus|wav|weba|webm)$/i;

function readerFor(file) {
  const fd = fs.openSync(file, 'r'), size = fs.fstatSync(fd).size;
  let calls = 0, bytes = 0;
  const read = (o, n) => { calls++; bytes += n; const b = Buffer.alloc(n); fs.readSync(fd, b, 0, n, o); return Promise.resolve(new Uint8Array(b.buffer, b.byteOffset, n)); };
  return { read, size, close: () => fs.closeSync(fd), stats: () => ({ calls, bytes }) };
}
const walk = d => fs.readdirSync(d, { withFileTypes: true }).flatMap(e => e.isDirectory() ? walk(path.join(d, e.name)) : AUDIO.test(e.name) ? [path.join(d, e.name)] : []);

function makeMedia() {
  const F = process.env.FFMPEG || 'ffmpeg';
  if (require('child_process').spawnSync(F, ['-version']).status !== 0) throw new Error('needs FFmpeg: put ffmpeg on the PATH, or set FFMPEG to its path');
  fs.mkdirSync(path.join(MEDIA, 'Album One'), { recursive: true });
  fs.mkdirSync(path.join(MEDIA, 'Loose'), { recursive: true });
  const ff = args => execFileSync(F, ['-v', 'error', '-y', ...args]);
  const M = n => path.join(MEDIA, n);
  const src = d => ['-f', 'lavfi', '-i', `aevalsrc='0.35*sin(2*PI*110*t)*(0.55+0.45*sin(2*PI*2*t))+0.18*sin(2*PI*440*t)*lt(t-floor(t)\\,0.15)+0.12*sin(2*PI*660*t)':d=${d}`];
  const meta = o => Object.entries(o).flatMap(([k, v]) => ['-metadata', `${k}=${v}`]);
  ff(['-f', 'lavfi', '-i', 'gradients=s=320x320:c0=0x222222:c1=0xdddddd:x0=0:y0=0:x1=320:y1=320:d=1,format=rgb24', '-frames:v', '1', M('cover.png')]);
  ff(['-f', 'lavfi', '-i', 'color=c=0x444444:s=300x300,drawbox=x=40:y=40:w=220:h=220:color=0xeeeeee:t=6', '-frames:v', '1', M('Album One/cover.jpg')]);
  const sw = { title: '', artist: 'Test Signal', album: 'Synthetic Weather', album_artist: 'Test Signal', date: '2024' };
  const pic = ['-i', M('cover.png'), '-map', '0', '-map', '1', '-c:v', 'png', '-disposition:v', 'attached_pic'];
  ff([...src(24), ...pic, '-c:a', 'libmp3lame', '-b:a', '160k', '-id3v2_version', '3', ...meta({ ...sw, title: 'Glass Orbit', track: '1/3', genre: 'Electronic' }), M('glass orbit.mp3')]);
  ff([...src(18), ...pic, '-c:a', 'flac', ...meta({ ...sw, title: 'Low Tide', track: '2', LYRICS: LYRICS_SYNCED }), M('low tide.flac')]);
  ff([...src(15), ...pic, '-c:a', 'aac', '-b:a', '128k', ...meta({ ...sw, title: 'Northern Static', track: '3/3', lyrics: LYRICS_PLAIN }), M('northern static.m4a')]);
  ff([...src(12), '-c:a', 'libvorbis', ...meta({ title: 'Ogg Song', artist: 'Vorbis Band', album: 'Single Sessions', tracknumber: '1' }), M('Album One/01 ogg song.ogg')]);
  ff([...src(11), '-c:a', 'libopus', '-b:a', '96k', ...meta({ title: 'Opus Song', artist: 'Vorbis Band', album: 'Album One', tracknumber: '2' }), M('Album One/02 opus song.opus')]);
  ff([...src(9), '-c:a', 'pcm_s16le', M('Loose/Some Artist - Plain Wave.wav')]);
  ff([...src(10), '-c:a', 'libmp3lame', '-b:a', '128k', '-id3v2_version', '4', ...meta({ title: 'Downloaded Thing', artist: 'Uploader Name', lyrics: LYRICS_MP3 }), M('Loose/Downloaded Thing [Ab12Cd34Ef5].mp3')]);
  ff([...src(8), '-c:a', 'libmp3lame', '-b:a', '128k', '-write_xing', '0', '-id3v2_version', '0', M('Loose/098. no tags at all.mp3')]);
  // lyrics in a .lrc file next to a song (String reads these before the song's own)
  fs.writeFileSync(M('Loose/Some Artist - Plain Wave.lrc'), LRC_SIDECAR);
}
// the lyrics the test songs carry (String's self-test looks for these lines)
const LYRICS_SYNCED = '[00:01.00]Low tide line one\n[00:03.00]Low tide line two\n[00:05.00]Third line at five\n[00:08.00]Fourth line at eight\n[00:12.00]Last line at twelve';
const LYRICS_PLAIN = 'Northern static, plain words\nsecond plain line\n\nafter a gap';
const LYRICS_MP3 = 'Words inside an mp3';
const LRC_SIDECAR = '[ti:Plain Wave]\n[00:00.50]Sidecar line one\n[00:02.50]Sidecar line two\n[00:05.00]Sidecar line three\n';
const LYRICS = { 'low tide.flac': LYRICS_SYNCED, 'northern static.m4a': LYRICS_PLAIN, 'Downloaded Thing [Ab12Cd34Ef5].mp3': LYRICS_MP3 };

const EXPECT = {
  'glass orbit.mp3': { title: 'Glass Orbit', artist: 'Test Signal', album: 'Synthetic Weather', albumArtist: 'Test Signal', track: 1, year: 2024, genre: 'Electronic', duration: 24, art: 'image/png' },
  'low tide.flac': { title: 'Low Tide', album: 'Synthetic Weather', track: 2, year: 2024, duration: 18, art: 'image/png' },
  'northern static.m4a': { title: 'Northern Static', artist: 'Test Signal', albumArtist: 'Test Signal', track: 3, year: 2024, duration: 15, art: 'image/png' },
  '01 ogg song.ogg': { title: 'Ogg Song', artist: 'Vorbis Band', album: 'Single Sessions', track: 1, duration: 12, art: null },
  '02 opus song.opus': { title: 'Opus Song', track: 2, duration: 11, art: null },
  'Some Artist - Plain Wave.wav': { title: '', duration: 9, art: null },
  'Downloaded Thing [Ab12Cd34Ef5].mp3': { title: 'Downloaded Thing', artist: 'Uploader Name', duration: 10, art: null },
  '098. no tags at all.mp3': { title: '', duration: 8, art: null },
};

(async () => {
  const only = process.argv[2];
  if (!only) makeMedia();
  const files = walk(only || MEDIA);
  let fails = 0, checks = 0;
  for (const f of files) {
    const r = readerFor(f);
    const t0 = Date.now();
    let t, art;
    try { t = await T.readTags(r.read, r.size); art = t.art ? await T.readArt(r.read, r.size) : null; }
    catch (e) { console.log('THROWS', path.basename(f), e.stack); fails++; r.close(); continue; }
    const ms = Date.now() - t0, st = r.stats();
    r.close();
    const name = path.basename(f);
    console.log(`${name}\n   ${JSON.stringify(t)}\n   art: ${art ? art.mime + ' ' + art.bytes.length + ' bytes' : 'none'}   read ${st.calls} times, ${Math.round(st.bytes / 1024)} KB of ${Math.round(r.size / 1024)} KB, ${ms} ms`);
    const want = EXPECT[name];
    if (only || !want) continue;
    for (const [k, v] of Object.entries(want)) {
      checks++;
      let ok;
      if (k === 'duration') ok = Math.abs(t.duration - v) < 0.15;
      else if (k === 'art') ok = v === null ? !art : !!art && art.mime === v && art.bytes[0] === 0x89 && art.bytes[1] === 0x50;
      else ok = t[k] === v;
      if (!ok) { fails++; console.log(`   FAIL ${k}: wanted ${JSON.stringify(v)}, got ${JSON.stringify(k === 'art' ? art && art.mime : t[k])}`); }
    }
    // lyrics inside the file: exactly the text put there, and none where there are none
    checks++;
    const lr = readerFor(f), lyr = await T.readLyrics(lr.read, lr.size);
    lr.close();
    if ((lyr || null) !== (LYRICS[name] || null)) { fails++; console.log(`   FAIL lyrics: wanted ${JSON.stringify(LYRICS[name] || null)}, got ${JSON.stringify(lyr)}`); }
    if (t._lyrics !== undefined || t._wantLyrics !== undefined) { fails++; console.log('   FAIL readTags leaks lyrics fields'); }
    // a cover that can be cut straight from the file must be the same bytes readArt finds
    if (t.art && t.art.o) {
      checks++;
      const fd = fs.openSync(f, 'r'), b = Buffer.alloc(t.art.n); fs.readSync(fd, b, 0, t.art.n, t.art.o); fs.closeSync(fd);
      if (!art || Buffer.compare(b, Buffer.from(art.bytes)) !== 0) { fails++; console.log('   FAIL art offsets do not match readArt'); }
    }
  }
  if (!only) console.log(`\n${checks - fails} of ${checks} checks passed`);
  process.exit(fails ? 1 : 0);
})();
