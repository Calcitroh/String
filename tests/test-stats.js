// Checks src\wwwroot\stats.js (listening time): periods, week starts, month and year edges, rankings, streaks.
//   node tests\test-stats.js
'use strict';
const S = require('../src/wwwroot/stats.js');
let pass = 0, fail = 0;
const check = (name, ok, extra) => { if (ok) pass++; else fail++; console.log(`${ok ? 'ok  ' : 'FAIL'} ${name}${ok || extra === undefined ? '' : '  ' + JSON.stringify(extra)}`); };
const at = (y, m, d, h = 12, min = 0) => new Date(y, m - 1, d, h, min);
const tr = (id, artist) => ({ id, title: id.toUpperCase(), artist });
const ticks = S.create(null), sampleDay = at(2026, 4, 1);
for (let i = 0; i < 240; i++) ticks.add(tr('song', 'Test'), 0.25, sampleDay);
check('240 quarter seconds count as 60 seconds', ticks.range(sampleDay, sampleDay).total === 60);
check('local dates change across midnight', S.calendarDay(at(2026, 3, 9, 0, 10)) - S.calendarDay(at(2026, 3, 8, 23, 50)) === 1);

function fill(weekStart) {
  const st = S.create(null, { weekStart });
  st.add(tr('t1', 'Artist A'), 20, at(2026, 11, 18, 15, 10)); for (let i = 0; i < 5; i++) st.add(tr('t1', 'Artist A'), 20, at(2026, 11, 18, 15, 11));   // 120 s today, 3 pm
  for (let i = 0; i < 3; i++) st.add(tr('t2', 'Artist B'), 20, at(2026, 11, 18, 9));       // 60 s today, 9 am
  for (let i = 0; i < 15; i++) st.add(tr('t1', 'Artist A'), 20, at(2026, 11, 17, 22));     // 300 s yesterday
  for (let i = 0; i < 5; i++) st.add(tr('t3', 'Artist C'), 20, at(2026, 11, 15, 10));      // 100 s on Sunday the 15th
  st.add(tr('t1', 'Artist A'), 25, at(2026, 11, 1, 0, 5)); st.add(tr('t1', 'Artist A'), 25, at(2026, 11, 1, 23, 59));   // 50 s on the 1st
  st.add(tr('t2', 'Artist B'), 20, at(2026, 10, 31, 23, 59)); st.add(tr('t2', 'Artist B'), 20, at(2026, 10, 31, 1));  // 40 s last month
  st.add(tr('t4', ''), 30, at(2026, 1, 5));                                                 // 30 s in January, no artist
  st.add(tr('t2', 'Artist B'), 20, at(2025, 12, 31, 23, 30));                               // 20 s last year
  return st;
}
const NOW = at(2026, 11, 18, 15, 30);   // a Wednesday

let st = fill(0);   // weeks start on Sunday
const tot = (kind, s = st) => { const p = s.period(kind, NOW); return [s.range(p.from, p.to).total, p.prevFrom ? s.range(p.prevFrom, p.prevTo).total : null]; };
check('today, and yesterday before it', JSON.stringify(tot('day')) === '[180,300]', tot('day'));
check('this week (Sunday start) and last week', JSON.stringify(tot('week')) === '[580,0]', tot('week'));
check('this month and last month', JSON.stringify(tot('month')) === '[630,40]', tot('month'));
check('this year and last year', JSON.stringify(tot('year')) === '[700,20]', tot('year'));
check('all time', st.range(st.period('all', NOW).from, NOW).total === 720, st.range(st.period('all', NOW).from, NOW).total);
const st1 = fill(1);   // weeks start on Monday: Sunday the 15th belongs to last week
check('this week (Monday start) and last week', JSON.stringify(tot('week', st1)) === '[480,100]', tot('week', st1));

const day = st.period('day', NOW), ds = st.series(day);
check('today by hour', ds.length === 24 && ds[15] === 120 && ds[9] === 60 && ds.reduce((a, b) => a + b, 0) === 180, ds);
check('the current hour is marked', day.current === 15);
const wk = st.period('week', NOW);
check('a week has 7 days, from Sunday', wk.units.length === 7 && wk.from.getDay() === 0 && wk.from.getDate() === 15 && wk.current === 3, { from: wk.from.toDateString(), current: wk.current });
const wk1 = st1.period('week', NOW);
check('Monday weeks start on Monday', wk1.from.getDay() === 1 && wk1.from.getDate() === 16 && wk1.current === 2, wk1.from.toDateString());
check('week by day', JSON.stringify(st.series(wk)) === '[100,0,300,180,0,0,0]', st.series(wk));
const mo = st.period('month', NOW);
check('November has 30 days', mo.units.length === 30 && mo.current === 17);
check('month by day', st.series(mo)[0] === 50 && st.series(mo)[17] === 180 && st.series(mo)[16] === 300, st.series(mo).filter(Boolean));
const yr = st.period('year', NOW);
check('year by month', yr.units.length === 12 && JSON.stringify(st.series(yr)) === '[30,0,0,0,0,0,0,0,0,40,630,0]', st.series(yr));
const dec = st.period('month', at(2026, 12, 31, 23));
check('December has 31 days; last month is November', dec.units.length === 31 && dec.prevFrom.getMonth() === 10 && dec.prevTo.getDate() === 30);
const jan = st.period('month', at(2026, 1, 10));
check('January\'s last month is December of the year before', jan.prevFrom.getFullYear() === 2025 && jan.prevFrom.getMonth() === 11 && st.range(jan.prevFrom, jan.prevTo).total === 20);
const all = st.period('all', NOW);
check('all time runs by month from the first day', all.units.length === 12 && all.from.getFullYear() === 2025 && all.from.getMonth() === 11, { n: all.units.length, from: all.from.toDateString() });
check('all time by month adds up', st.series(all).reduce((a, b) => a + b, 0) === 720, st.series(all));

const m = st.range(mo.from, mo.to);
check('top artists this month', m.artists.map(a => a.name + ':' + a.secs).join(',') === 'Artist A:470,Artist C:100,Artist B:60', m.artists);
check('top songs this month', m.tracks[0].id === 't1' && m.tracks[0].secs === 470 && m.tracks[0].title === 'T1' && m.tracks[0].artist === 'Artist A', m.tracks[0]);
check('a song with no artist still counts', st.range(at(2026, 1, 1), at(2026, 1, 31)).artists[0].name === '' && st.range(at(2026, 1, 1), at(2026, 1, 31)).total === 30);

check('streak: today and yesterday', st.streak(NOW) === 2, st.streak(NOW));
check('streak counts from yesterday before today\'s first minute', st.streak(at(2026, 11, 19, 8)) === 2, st.streak(at(2026, 11, 19, 8)));
check('streak broken by a day off', st.streak(at(2026, 11, 20, 8)) === 0);

check('nothing for a silly tick', !st.add(tr('t1', 'Artist A'), 0) && !st.add(tr('t1', 'Artist A'), -3) && !st.add(tr('t1', 'Artist A'), 400) && !st.add(null, 5));
const again = S.create(JSON.parse(JSON.stringify(st.doc)), { weekStart: 0 });
check('saved and loaded, the same totals', JSON.stringify(tot('month', again)) === '[630,40]' && JSON.stringify(tot('year', again)) === '[700,20]');
again.add(tr('t1', 'Renamed Artist'), 10, at(2026, 11, 18, 16));
check('a song renamed later keeps one row and its new name', again.doc.tracks.length === st.doc.tracks.length && again.doc.tracks[0][2] === 'Renamed Artist');
check('the file stays small', JSON.stringify(st.doc).length < 1200, JSON.stringify(st.doc).length);

// a duplicate's time goes to the copy that's kept: into a song with time of its own, and into one with none yet
{
  const d = S.create(null);
  d.add(tr('keep', 'A'), 20, at(2026, 11, 1)); d.add(tr('copy', 'A'), 10, at(2026, 11, 1)); d.add(tr('copy', 'A'), 5, at(2026, 11, 2));
  d.add(tr('lone', 'B'), 7, at(2026, 11, 2));
  const all2 = () => d.range(at(2026, 11, 1), at(2026, 11, 3));
  check('merge: the copy\'s time joins the kept song', d.merge('copy', 'keep') && all2().tracks[0].id === 'keep' && all2().tracks[0].secs === 35 && !all2().tracks.some(t => t.id === 'copy') && all2().total === 42, all2().tracks);
  check('merge: into a song never played, the copy\'s row is renamed', d.merge('lone', 'fresh') && all2().tracks.some(t => t.id === 'fresh' && t.secs === 7) && d.add(tr('fresh', 'B'), 3, at(2026, 11, 3)) && all2().tracks.find(t => t.id === 'fresh').secs === 10, all2().tracks);
  check('merge: nothing to merge', !d.merge('nope', 'keep') && !d.merge('keep', 'keep'));
}

// songs played on the phone (Navidrome's history): whole songs, at the time they were played, also counted as phone time
{
  const d = S.create(null);
  d.add(tr('pc', 'A'), 20, at(2026, 11, 3, 9));
  check('a phone play: the whole song at once', d.addPhone(tr('ph', 'B'), 214.5, at(2026, 11, 3, 21, 40)));
  const r = d.range(at(2026, 11, 3), at(2026, 11, 3));
  check('phone time counts in the day, apart as well', r.total === 234.5 && r.phone === 214.5, r);
  check('…in the hour it was played, for its artist and song', d.doc.days[S.dayKey(at(2026, 11, 3))].h[21] === 214.5 && r.artists[0].name === 'B' && r.tracks[0].id === 'ph');
  check('a day with no phone plays has no phone time', d.range(at(2026, 11, 4), at(2026, 11, 4)).phone === 0);
  check('nonsense from a phone is refused', !d.addPhone(tr('x', 'B'), 0, at(2026, 11, 3)) && !d.addPhone(tr('x', 'B'), 99999, at(2026, 11, 3)) && !d.addPhone(tr('x', 'B'), 100, new Date('nope')) && !d.addPhone(null, 100, at(2026, 11, 3)));
  check('a player tick still can\'t be a whole song', !d.add(tr('pc', 'A'), 200, at(2026, 11, 3)));
}

console.log(`\n${pass} of ${pass + fail} checks passed`);
process.exit(fail ? 1 : 0);
