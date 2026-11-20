// Checks src\wwwroot\dupes.js: which songs the Duplicates page lists, which copy it keeps.
//   node tests\test-dupes.js
'use strict';
const Y = require('../src/wwwroot/dupes.js');
let pass = 0, fail = 0;
const check = (name, ok, extra) => { if (ok) pass++; else fail++; console.log(`${ok ? 'ok  ' : 'FAIL'} ${name}${ok || extra === undefined ? '' : '  ' + JSON.stringify(extra)}`); };
const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
for (const durations of [[60, 0, 360], [60, 62, 64]]) {
  const candidates = durations.map((duration, i) => ({ id: String(i), T: 'Overture', A: 'Band', duration }));
  const group = Y.duplicates(candidates)[0];
  check('a bridge cannot certify different recordings: ' + durations.join('/'), group && !group.sure && group.sets[group.indexOf(candidates[0])] !== group.sets[group.indexOf(candidates[2])]);
}

const dl = [
  { id: '1', T: 'Battle Theme', A: 'Toby', duration: 180, format: 'mp3', size: 4e6 },
  { id: '2', T: 'Battle Theme', A: 'Toby', duration: 181, format: 'flac', size: 30e6 },
  { id: '3', T: 'battle theme!', A: 'toby', duration: 180.5, format: 'mp3', size: 5e6 },
  { id: '4', T: 'Battle Theme', A: 'Toby', duration: 400, format: 'mp3', size: 9e6 },
  { id: '5', T: 'Battle Theme', A: 'Someone Else', duration: 180, format: 'mp3', size: 4e6 },
  { id: '6', T: 'Lonely', A: 'X', duration: 100, format: 'mp3', size: 1 },
];
const groups = Y.duplicates(dl);
check('one group: same song, same length, any spelling; another credit at that length is a maybe', groups.length === 1 && same(groups[0].map(t => t.id), ['2', '3', '1', '5']), groups.map(g => g.map(t => t.id)));
check('the sure copies share a set, the maybe stands apart', same(groups[0].sets, [0, 0, 0, 1]) && groups[0].sure === false, groups[0].sets);
check('the lossless copy first, then the higher bitrate', groups[0][0].format === 'flac' && groups[0][1].size === 5e6);

const messy = [
  { id: 'a1', path: 'C:\\M\\x\\Song.m4a', T: 'Glass Orbit', A: 'Test Signal', duration: 200, format: 'm4a', size: 3e6 },
  { id: 'a2', path: 'C:\\M\\y\\Song.m4a', T: 'Glass Orbit (Official Video)', A: 'Test Signal', duration: 201, format: 'm4a', size: 3e6 },
  { id: 'b1', path: 'C:\\M\\x\\b.m4a', T: 'Paper Lantern (From "Some Show")', A: 'Singer', duration: 165, format: 'm4a', size: 2e6 },
  { id: 'b2', path: 'C:\\M\\x\\c.m4a', T: 'Paper Lantern', A: 'Uploader', duration: 165, format: 'm4a', size: 2e6 },
  { id: 'c1', path: 'C:\\M\\x\\d.m4a', T: 'Static Bloom', A: 'Vorbis Band', duration: 269, format: 'm4a', size: 4e6 },
  { id: 'c2', path: 'C:\\M\\x\\e.m4a', T: 'Static Bloom - Long Ver.', A: 'Vorbis Band', duration: 269, format: 'm4a', size: 4e6 },
  { id: 'd1', path: 'C:\\M\\x\\f.m4a', T: 'Intro', A: 'Band A', duration: 60, format: 'mp3', size: 1e6 },
  { id: 'd2', path: 'C:\\M\\x\\g.m4a', T: 'Intro', A: 'Band B', duration: 95, format: 'mp3', size: 1e6 },
  { id: 'e1', path: 'C:\\M\\x\\h.m4a', T: 'Song feat. Someone', A: 'Band', duration: 180, format: 'mp3', size: 1e6 },
  { id: 'e2', path: 'C:\\M\\x\\i.m4a', T: 'Song', A: 'Band', duration: 180, format: 'mp3', size: 1e6 },
];
const mg = Y.duplicates(messy, { prefer: t => t.id === 'a2' ? 1 : 0 });
const gOf = id => mg.find(g => g.some(t => t.id === id));
check('"(Official Video)" makes no difference: sure', gOf('a1') && gOf('a1').sure && gOf('a1').length === 2);
check('identical copies: the preferred one (liked, in a playlist, the bigger folder) is kept', gOf('a1')[0].id === 'a2');
check('same name and length, other credit: a maybe', gOf('b1') && !gOf('b1').sure && gOf('b1').length === 2);
check('"Title - Long Ver." at the same length: a maybe', gOf('c1') && !gOf('c1').sure);
check('same name, different length: not a duplicate', !gOf('d1') && !gOf('d2'));
check('"feat." left off: a maybe', gOf('e1') && !gOf('e1').sure);
check('sure groups come first', mg[0].sure && !mg[mg.length - 1].sure, mg.map(g => g.sure));
const ap = Y.duplicates(messy, { apart: (a, b) => [a.id, b.id].sort().join() === 'b1,b2' });
check('"Not the same": that maybe pair is no longer listed; the rest still are', !ap.some(g => g.some(t => t.id === 'b1')) && ap.length === mg.length - 1);
const three = Y.duplicates([
  { id: 'v', path: 'C:\\M\\a.m4a', T: 'Static Bloom - Long Ver.', A: 'Vorbis Band', duration: 269, format: 'm4a', size: 4e6 },
  { id: 'x1', path: 'C:\\M\\b.m4a', T: 'Static Bloom', A: 'Vorbis Band', duration: 269, format: 'm4a', size: 4e6 },
  { id: 'x2', path: 'C:\\M\\c.m4a', T: 'Static Bloom', A: 'Vorbis Band', duration: 269, format: 'm4a', size: 4e6 },
])[0];
check('a maybe group keeps a copy from its biggest sure set', three && ['x1', 'x2'].includes(three[0].id) && same(three.sets.slice().sort(), [0, 0, 1]) && three.sets[0] === 0, three && three.map(t => t.id + ':' + three.sets[three.indexOf(t)]));

console.log(`\n${pass} of ${pass + fail} checks passed`);
process.exit(fail ? 1 : 0);
