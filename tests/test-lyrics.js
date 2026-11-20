// Checks src\wwwroot\lyrics.js's text rules: reading LRC, finding the current line, picking an LRCLIB match.
//   node tests\test-lyrics.js
'use strict';
const Y = require('../src/wwwroot/lyrics.js');
let pass = 0, fail = 0;
const check = (name, ok, extra) => { if (ok) pass++; else fail++; console.log(`${ok ? 'ok  ' : 'FAIL'} ${name}${ok || extra === undefined ? '' : '  ' + JSON.stringify(extra)}`); };

const lrc = `[ar: Test Signal]
[ti: Glass Orbit]
[length: 00:24]

[00:00.00]
[00:01.50]First line
[00:04.20]<00:04.20>Second <00:04.80>line with word times
[00:08.00][00:16.00]A chorus, sung twice
[00:12.345]Third line, three decimals
[00:20]No decimals
`;
const p = Y.parseLrc(lrc);
check('synced', p.synced === true);
check('times in order, a repeated line twice', JSON.stringify(p.lines.map(l => l.t)) === '[1.5,4.2,8,12.345,16,20]', p.lines.map(l => l.t));
check('word times and tags dropped', p.lines[1].text === 'Second line with word times' && !p.lines.some(l => /ar:|ti:/.test(l.text)), p.lines.map(l => l.text));
check('the repeated chorus', p.lines.filter(l => l.text === 'A chorus, sung twice').length === 2);
check('a lone blank first line is dropped', p.lines[0].text === 'First line');

const off = Y.parseLrc('[offset:+500]\n[00:02.00]a\n[00:03.00]b');
const oneLine = Y.parseLrc('[00:05.00]Only line');
check('one timed lyric is displayed', oneLine.synced && oneLine.lines[0].text === 'Only line' && oneLine.lines[0].t === 5);
check('mixed plain and timed lyrics keep both lines', Y.parseLrc('[00:05.00]one\ntwo').lines.map(x => x.text).join('|') === 'one|two');
check('a + offset makes lines come sooner', off.lines[0].t === 1.5 && off.lines[1].t === 2.5, off.lines);
const crlf = Y.parseLrc('[00:01.00]one\r\n[00:02.00]two\r\n');
check('Windows line ends', crlf.synced && crlf.lines.length === 2 && crlf.lines[1].text === 'two');

const plain = Y.parseLrc('Verse one\nline two\n\n\n\nChorus\n[ar: Someone]');
check('plain lyrics', plain.synced === false && plain.lines.map(l => l.text).join('|') === 'Verse one|line two||Chorus', plain.lines);
check('one timed line is not "synced"', Y.parseLrc('[00:05.00]only one\nand plain').synced === false);
check('nothing', Y.parseLrc('').lines.length === 0 && Y.parseLrc(null).lines.length === 0);

check('line before the first', Y.lineAt(p.lines, 0.5) === -1);
check('line at a start time', Y.lineAt(p.lines, 4.2) === 1);
check('line between', Y.lineAt(p.lines, 10) === 2);
check('line after the last', Y.lineAt(p.lines, 300) === p.lines.length - 1);

const results = [
  { trackName: 'Glass Orbit', artistName: 'Someone Else', duration: 240, plainLyrics: 'x' },
  { trackName: 'Glass Orbit (Remastered 2011)', artistName: 'Test Signal', duration: 181, plainLyrics: 'plain' },
  { trackName: 'Glass Orbit', artistName: 'Test Signal', duration: 182, syncedLyrics: '[00:01.00]a\n[00:02.00]b' },
  { trackName: 'Glass Orbit', artistName: 'Test Signal', duration: 300, syncedLyrics: '[00:01.00]far' },
];
const best = Y.pickBest(results, { title: 'Glass Orbit', artist: 'Test Signal', duration: 180 });
check('picks the same song, synced, with the closest length', best === results[2], best);
check('nothing when the length is far off', Y.pickBest([results[3]], { title: 'Glass Orbit', artist: 'Test Signal', duration: 180 }) === null);
check('nothing for another title', Y.pickBest([{ trackName: 'Low Tide', artistName: 'Test Signal', duration: 180, plainLyrics: 'x' }], { title: 'Glass Orbit', artist: 'Test Signal', duration: 180 }) === null);
check('no artist known: the length decides', Y.pickBest(results, { title: 'Glass Orbit', artist: '', duration: 240 }) === results[0]);
check('another artist and no length: not trusted', Y.pickBest([results[0]], { title: 'Glass Orbit', artist: 'Test Signal', duration: 0 }) === null);
check('titles match without "(feat. …)" and punctuation', Y.norm('Low Tide (feat. Somebody) - Remastered!') === 'low tide remastered' && Y.norm('Café  Déjà-vu') === 'cafe deja vu', [Y.norm('Low Tide (feat. Somebody) - Remastered!'), Y.norm('Café  Déjà-vu')]);

console.log(`\n${pass} of ${pass + fail} checks passed`);
process.exit(fail ? 1 : 0);
