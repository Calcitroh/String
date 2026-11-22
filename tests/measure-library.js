// A fixed library of 100 albums. Half have folder pictures, half have pictures in each song.
'use strict';
const fs = require('node:fs');
const path = require('node:path');
const { execFileSync } = require('node:child_process');
const base = path.join(__dirname, '..', 'test-runs', 'measure-media-v1');
const media = path.join(base, 'music');
const ff = args => execFileSync(process.env.FFMPEG || 'ffmpeg', ['-v', 'error', '-y', ...args]);
const num = i => String(i).padStart(3, '0');

function frame(name, data) {
  const head = Buffer.alloc(10);
  head.write(name);
  head.writeUInt32BE(data.length, 4);
  return Buffer.concat([head, data]);
}
function tag(album, song, cover) {
  const fields = { TALB: album, TIT2: 'Track ' + num(song), TPE1: 'Test signal', TRCK: String(song) };
  const frames = Object.entries(fields).map(([key, value]) => frame(key, Buffer.from('\0' + value)));
  if (cover) frames.push(frame('APIC', Buffer.concat([Buffer.from('\0image/jpeg\0\x03\0'), cover])));
  const data = Buffer.concat(frames), head = Buffer.from('ID3\x03\0\0\0\0\0\0');
  for (let i = 0; i < 4; i++) head[9 - i] = (data.length >>> (7 * i)) & 127;
  return Buffer.concat([head, data]);
}
function generate() {
  fs.mkdirSync(media, { recursive: true });
  if (fs.existsSync(path.join(base, 'complete'))) return;
  const tone = path.join(base, 'tone.mp3'), pic = path.join(base, 'picture.jpg');
  ff(['-f', 'lavfi', '-i', 'sine=frequency=220:sample_rate=22050:duration=75',
    '-c:a', 'libmp3lame', '-b:a', '16k', '-id3v2_version', '0', tone]);
  const audio = fs.readFileSync(tone);
  for (let i = 1; i <= 100; i++) {
    const album = 'Album ' + num(i), dir = path.join(media, album);
    fs.mkdirSync(dir, { recursive: true });
    const color = n => '0x' + ((i * n) & 0xffffff).toString(16).padStart(6, '0');
    ff(['-f', 'lavfi', '-i', 'gradients=s=1200x1200:c0=' + color(93857) + ':c1=' + color(734551) +
      ':x0=0:y0=0:x1=1200:y1=1200:seed=' + i + ',noise=alls=8:all_seed=' + i,
      '-frames:v', '1', '-q:v', '4', pic]);
    const cover = fs.readFileSync(pic);
    if (i % 2) fs.writeFileSync(path.join(dir, 'cover.jpg'), cover);
    for (let j = 1; j <= 12; j++) {
      fs.writeFileSync(path.join(dir, num(j) + '.mp3'), Buffer.concat([tag(album, j, i % 2 ? null : cover), audio]));
    }
  }
  fs.writeFileSync(path.join(base, 'complete'), '1200 tracks, 100 albums\n');
}
generate();
console.log(media);
