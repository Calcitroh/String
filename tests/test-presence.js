// Tests the Discord helper (string-presence.exe, source presence\) against a fake Discord (a named pipe of our own,
// never the real discord-ipc-N, so a real Discord status is never touched) and a fake cover search on localhost.
//   dotnet publish presence\Presence.csproj -c Release -o app-new\presence
//   node tests\test-presence.js          (PRESENCE_EXE=<path> tests another copy)
'use strict';
const net = require('net'), http = require('http'), fs = require('fs'), path = require('path'), { spawn } = require('child_process');
const ROOT = path.join(__dirname, '..');
const EXE = process.env.PRESENCE_EXE || path.join(ROOT, 'app-new', 'presence', 'string-presence.exe');
const DIR = path.join(ROOT, 'test-runs', 'string-presence');
const FILE = path.join(DIR, 'presence.json');
const ID = '123456789012345678';
const sleep = ms => new Promise(r => setTimeout(r, ms));
let pass = 0, fail = 0;
const check = (name, ok, extra) => { if (ok) pass++; else fail++; console.log(`${ok ? 'ok  ' : 'FAIL'} ${name}${extra !== undefined ? '  ' + JSON.stringify(extra) : ''}`); };
async function waitFor(fn, ms) { const end = Date.now() + ms; while (Date.now() < end) { const v = fn(); if (v) return v; await sleep(50); } return null; }
function write(doc) { const tmp = FILE + '.tmp'; fs.writeFileSync(tmp, JSON.stringify(doc)); fs.renameSync(tmp, FILE); }
const playing = (o = {}) => { const now = Date.now(); return { app: 'String', state: 'playing', title: 'Glass Orbit', artist: 'Test Signal', album: 'Synthetic Weather', duration: 180, position: 10, at: now, startedAt: now - 10000, endsAt: now + 170000, covers: false, display: 'song', ...o }; };

/* ---------- a fake Discord ---------- */
function fakeDiscord(name, { refuse = false } = {}) {
  const d = { frames: [], sockets: [], handshakes: 0, pongs: [] };
  d.server = net.createServer(sock => {
    d.sockets.push(sock);
    let buf = Buffer.alloc(0);
    sock.on('data', chunk => {
      buf = Buffer.concat([buf, chunk]);
      while (buf.length >= 8) {
        const op = buf.readUInt32LE(0), len = buf.readUInt32LE(4);
        if (buf.length < 8 + len) break;
        const raw = buf.subarray(8, 8 + len).toString('utf8');
        buf = buf.subarray(8 + len);
        const j = JSON.parse(raw);
        if (op === 0) {
          d.handshakes++; d.hello = j;
          if (refuse) send(sock, 2, { code: 4000, message: 'Invalid Client ID' });
          else send(sock, 1, { cmd: 'DISPATCH', evt: 'READY', data: { v: 1, user: { id: '1', username: 'tester', global_name: 'Tester' } } });
        } else if (op === 1) { d.frames.push({ at: Date.now(), ...j }); send(sock, 1, { cmd: j.cmd, nonce: j.nonce, data: {} }); }
        else if (op === 4) d.pongs.push(j);
      }
    });
    sock.on('error', () => { });
  });
  d.server.listen('\\\\.\\pipe\\' + name);
  d.sets = () => d.frames.filter(f => f.cmd === 'SET_ACTIVITY');
  d.last = () => { const s = d.sets(); return s[s.length - 1]; };
  return d;
}
function send(sock, op, obj) {
  const body = Buffer.from(JSON.stringify(obj), 'utf8'), head = Buffer.alloc(8);
  head.writeUInt32LE(op, 0); head.writeUInt32LE(body.length, 4);
  sock.write(Buffer.concat([head, body]));
}
function run(args) {
  const p = spawn(EXE, args, { stdio: ['pipe', 'pipe', 'pipe'] });
  p.lines = [];
  let acc = '';
  p.stdout.on('data', c => { acc += c; let i; while ((i = acc.indexOf('\n')) >= 0) { p.lines.push(acc.slice(0, i).trim()); acc = acc.slice(i + 1); } });
  p.exited = new Promise(r => p.on('exit', code => r(code)));
  return p;
}

(async () => {
  if (!fs.existsSync(EXE)) { console.log('build it first: dotnet publish presence\\Presence.csproj -c Release -o app-new\\presence'); process.exit(1); }
  fs.rmSync(DIR, { recursive: true, force: true });
  fs.mkdirSync(DIR, { recursive: true });

  // a fake iTunes search that knows one song, and a fake Deezer search that knows another
  let searches = 0, deezerAsks = 0;
  const itunes = http.createServer((req, res) => {
    searches++;
    const u = new URL(req.url, 'http://x');
    res.setHeader('content-type', 'application/json');
    if (u.pathname === '/deezer') {
      deezerAsks++;
      const q = u.searchParams.get('q') || '';
      const data = /deezer only/i.test(q) ? [{ title: 'Deezer Only Song', artist: { name: 'Someone Else' }, album: { cover_xl: 'https://example.test/dz/wrong.jpg' } }, { title: 'Deezer Only Song (Live)', artist: { name: 'Test Signal' }, album: { cover_xl: 'https://example.test/dz/1000x1000.jpg' } }] : [];
      res.end(JSON.stringify({ data, total: data.length }));
      return;
    }
    const term = u.searchParams.get('term') || '';
    const results = /glass orbit/i.test(term) ? [{ artistName: 'Someone Else', trackName: 'Glass Orbit', artworkUrl100: 'https://example.test/wrong/100x100bb.jpg' }, { artistName: 'Test Signal', trackName: 'Glass Orbit (Remastered)', artworkUrl100: 'https://example.test/art/100x100bb.jpg' }] : [];
    res.end(JSON.stringify({ resultCount: results.length, results }));
  }).listen(0, '127.0.0.1');
  await new Promise(r => itunes.once('listening', r));
  const ITUNES = `http://127.0.0.1:${itunes.address().port}/search`, DEEZER = `http://127.0.0.1:${itunes.address().port}/deezer`;

  /* bad arguments */
  const bad = run(['not-a-number', FILE]);
  check('a bad application ID is refused (exit 2)', await bad.exited === 2 && /^error /.test(bad.lines[0] || ''), bad.lines);

  /* the normal life of it */
  const pipe = 'string-fake-discord-' + process.pid;
  const d = fakeDiscord(pipe);
  write({ app: 'String', state: 'paused', title: 'Glass Orbit', at: Date.now() });
  const p = run([ID, FILE, '--pipe', pipe, '--itunes', ITUNES, '--deezer', DEEZER]);
  check('connects and says who to', !!await waitFor(() => p.lines.includes('status connected Tester'), 6000), p.lines);
  check('handshake carries the application ID', d.hello && d.hello.v === 1 && d.hello.client_id === ID, d.hello);
  check('paused: the status is cleared', !!await waitFor(() => d.sets().length && !d.last().args.activity, 3000), d.last());
  check('pid is sent', d.last() && d.last().args.pid === p.pid);

  let doc = playing();
  write(doc);
  const a1 = await waitFor(() => { const f = d.last(); return f && f.args.activity ? f.args.activity : null; }, 6000);
  check('playing: Listening, the song, the artist', a1 && a1.type === 2 && a1.details === 'Glass Orbit' && a1.state === 'Test Signal' && a1.status_display_type === 2, a1);
  check('playing: a time bar from start to end', a1 && a1.timestamps && a1.timestamps.start === doc.startedAt && a1.timestamps.end === doc.endsAt, a1 && a1.timestamps);
  check('no cover without the setting', a1 && !a1.assets && searches === 0, { assets: a1 && a1.assets, searches });

  write(playing({ covers: true }));
  const a2 = await waitFor(() => { const f = d.last(); const a = f && f.args.activity; return a && a.assets && a.assets.large_image ? a : null; }, 12000);
  check('cover found, the right one, 600 px', a2 && a2.assets.large_image === 'https://example.test/art/600x600bb.jpg' && a2.assets.large_text === 'Synthetic Weather', a2 && a2.assets);
  await sleep(500);
  const cache = JSON.parse(fs.readFileSync(path.join(DIR, 'discord-covers.json'), 'utf8'));
  check('cover remembered on disk', cache['v2\ntest signal\nglass orbit'] === 'https://example.test/art/600x600bb.jpg', cache);
  check('Apple had it: Deezer not asked', deezerAsks === 0, deezerAsks);

  await sleep(4300);
  write(playing({ covers: true, title: 'Deezer Only Song' }));
  const a2b = await waitFor(() => { const a = d.last().args.activity; return a && a.details === 'Deezer Only Song' && a.assets && a.assets.large_image ? a : null; }, 12000);
  check('a song Apple lacks gets its cover from Deezer, the right one', a2b && a2b.assets.large_image === 'https://example.test/dz/1000x1000.jpg', a2b && a2b.assets);

  await sleep(4300);
  write(playing({ covers: true, title: 'Unknown Song', artist: 'Nobody' }));
  await waitFor(() => { const a = d.last().args.activity; return a && a.details === 'Unknown Song'; }, 7000);
  await sleep(1500);
  const a3 = d.last().args.activity;
  check('a song with no cover found shows without one', a3 && a3.details === 'Unknown Song' && !a3.assets, a3);
  const cache2 = JSON.parse(fs.readFileSync(path.join(DIR, 'discord-covers.json'), 'utf8'));
  check('and is remembered as having none (both asked)', cache2['v2\nnobody\nunknown song'] === '', cache2);

  // an artist of symbols only ("!!!") has nothing left once punctuation goes: it must not take the cover of whoever
  // Apple lists first for that title
  await sleep(4300);
  write(playing({ covers: true, title: 'Glass Orbit', artist: '!!!' }));
  await waitFor(() => { const a = d.last().args.activity; return a && a.state === '!!!'; }, 7000);
  await sleep(1500);
  const a5 = d.last().args.activity;
  check('a symbols-only artist doesn\'t get another artist\'s cover', a5 && a5.state === '!!!' && !a5.assets, a5);

  // rapid changes (skipping through songs): never more than Discord's limit, and the newest wins
  await sleep(4500);
  const before = d.sets().length, t0 = Date.now();
  for (let i = 1; i <= 6; i++) { write(playing({ title: 'Skip ' + i })); await sleep(250); }
  await sleep(6500);
  const burst = d.sets().slice(before);
  const tooFast = burst.some((f, i) => i > 0 && f.at - burst[i - 1].at < 4000);
  check('rate limit: at least 4 s between updates', !tooFast && burst.length <= 3, burst.map(f => [f.at - t0, f.args.activity && f.args.activity.details]));
  check('the newest song wins', d.last().args.activity && d.last().args.activity.details === 'Skip 6', d.last().args.activity);

  await sleep(4300);
  write(playing({ title: 'X', display: 'artist' }));
  const a4 = await waitFor(() => { const a = d.last().args.activity; return a && a.details && a.details.startsWith('X') ? a : null; }, 6000);
  check('one-letter title padded to Discord\'s minimum', a4 && a4.details.length === 2, a4 && a4.details);
  check('member list can show the artist instead', a4 && a4.status_display_type === 1, a4 && a4.status_display_type);

  await sleep(4300);
  write(playing({ startedAt: Date.now() - 300000, endsAt: Date.now() - 60000 }));
  check('a stale "playing" (song long over) clears', !!await waitFor(() => !d.last().args.activity, 6000), d.last());

  // Discord pings; it gets its pong
  send(d.sockets[d.sockets.length - 1], 3, { ping: 42 });
  check('answers a ping with a pong', !!await waitFor(() => d.pongs.some(x => x.ping === 42), 3000), d.pongs);

  // Discord restarts: the helper waits, reconnects and sends the current song again
  await sleep(4300);
  write(playing({ title: 'After Restart' }));
  await waitFor(() => d.last().args.activity && d.last().args.activity.details === 'After Restart', 6000);
  d.sockets[d.sockets.length - 1].destroy();
  check('notices Discord going away', !!await waitFor(() => p.lines.includes('status waiting'), 4000), p.lines.slice(-3));
  const n0 = d.sets().length;
  check('reconnects by itself', !!await waitFor(() => d.handshakes >= 2, 9000), d.handshakes);
  check('and sends the current song again', !!await waitFor(() => d.sets().length > n0 && d.last().args.activity && d.last().args.activity.details === 'After Restart', 6000), d.last());

  // String closes a helper's stdin right after starting it: that must not stop it
  p.stdin.end();
  await sleep(1500);
  check('keeps running when its stdin is closed (String does that)', p.exitCode === null && !!await waitFor(() => d.sets().length, 100));
  p.kill();

  // started by something that then dies (String crashing): it stops by itself
  const launcher = spawn(process.execPath, ['-e', `const c = require('child_process').spawn(${JSON.stringify(EXE)}, ${JSON.stringify([ID, FILE, '--pipe', 'string-no-such-pipe-' + process.pid])}, { stdio: 'ignore' }); console.log(c.pid); setInterval(() => { }, 1000);`], { stdio: ['ignore', 'pipe', 'ignore'] });
  const childPid = await new Promise(r => launcher.stdout.once('data', b => r(+String(b).trim())));
  await sleep(1200);
  const alive = pid => { try { process.kill(pid, 0); return true; } catch { return false; } };
  const wasAlive = alive(childPid);
  launcher.kill();
  const gone = await waitFor(() => !alive(childPid), 5000);
  check('stops when the program that started it goes away', wasAlive && !!gone, { wasAlive, gone: !!gone });
  if (!gone) try { process.kill(childPid); } catch { }

  /* Discord refusing the application ID */
  const pipe2 = 'string-fake-discord-refuse-' + process.pid;
  const d2 = fakeDiscord(pipe2, { refuse: true });
  const q = run([ID, FILE, '--pipe', pipe2]);
  const code2 = await Promise.race([q.exited, sleep(6000).then(() => 'still running')]);
  check('a refused application ID: says so and exits 3', code2 === 3 && q.lines.some(l => l.startsWith('error ')), { code2, lines: q.lines });
  if (code2 === 'still running') q.kill();

  /* no Discord at all */
  const r = run([ID, FILE, '--pipe', 'string-no-such-pipe-' + process.pid]);
  check('no Discord: waits', !!await waitFor(() => r.lines.includes('status waiting'), 4000), r.lines);
  r.kill();

  d.server.close(); d2.server.close(); itunes.close();
  console.log(`\n${pass} of ${pass + fail} checks passed`);
  process.exit(fail ? 1 : 0);
})();
