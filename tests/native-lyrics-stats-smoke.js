// From the project root: build string-app, then run node tests/native-lyrics-stats-smoke.js [release].
'use strict';
const assert = require('node:assert/strict');
const fs = require('node:fs');
const net = require('node:net');
const path = require('node:path');
const {spawn, execFileSync} = require('node:child_process');
const {once} = require('node:events');
const root = path.resolve(__dirname, '..');
const media = path.join(root, 'test-runs', 'string-media');
const children = [];

async function fake(script) {
  const socket = net.createServer().listen(0, '127.0.0.1');
  await once(socket, 'listening');
  const {port} = socket.address();
  await new Promise(resolve => socket.close(resolve));
  const child = spawn(process.execPath, [path.join(__dirname, script), String(port)], {
    windowsHide: true, stdio: ['ignore', 'pipe', 'inherit']
  });
  children.push(child);
  await new Promise((resolve, reject) => {
    child.once('error', reject);
    child.once('exit', code => reject(new Error(script + ' stopped: ' + code)));
    child.stdout.on('data', chunk => {
      if (chunk.toString().includes('listening on ' + port)) resolve();
    });
  });
  return port;
}

async function main() {
  const exe = path.join(root, 'native', 'target', process.argv[2] === 'release' ? 'release' : 'debug', 'String.exe');
  assert(fs.existsSync(exe), 'Build string-app before running this check');
  const files = ['glass orbit.mp3', 'low tide.flac', 'northern static.m4a', 'Album One/01 ogg song.ogg', 'Album One/02 opus song.opus'].map(p => path.join(media, p));
  if (!files.every(p => fs.existsSync(p))) {
    execFileSync(process.execPath, [path.join(__dirname, 'test-tags.js')], {cwd: root, windowsHide: true});
  }
  const parent = path.join(root, 'test-runs', 'lyrics-stats');
  fs.mkdirSync(parent, {recursive: true});
  const run = fs.mkdtempSync(path.join(parent, 'run-'));
  const data = path.join(run, 'profile'), out = path.join(run, 'out');
  fs.mkdirSync(data);
  execFileSync(process.execPath, [path.join(__dirname, 'fake-navidrome.js'), path.join(data, 'navidrome.db'), media], {windowsHide: true});
  fs.writeFileSync(path.join(data, 'state.json'), JSON.stringify({v: 1, files}));
  const lyrics = await fake('fake-lrclib.js');
  const ollama = await fake('fake-ollama.js');
  const app = spawn(exe, ['--smoke-test', '--data', data, '--out', out], {
    cwd: root, stdio: 'inherit', windowsHide: true,
    env: {...process.env, STRING_SMOKE_LRCLIB_PORT: String(lyrics), STRING_SMOKE_OLLAMA_PORT: String(ollama)}
  });
  console.log(out);
  const [code] = await once(app, 'exit');
  const results = JSON.parse(fs.readFileSync(path.join(out, 'results.json'), 'utf8'));
  assert.equal(code, 0, JSON.stringify(results));
  assert(results.passed);
  if (results.checks.includes('host-library')) {
    const state = JSON.parse(fs.readFileSync(path.join(data, 'state.json'), 'utf8'));
    assert.equal(state.phone.plays, 4);
    assert.equal(state.set.phoneUser, 'u1');
  }
  const seen = await (await fetch('http://127.0.0.1:' + lyrics + '/api/_seen')).json();
  assert.equal(seen.length, 1, JSON.stringify(seen));
  assert.equal(seen[0].query.track_name, 'Glass Orbit');
  assert.equal(seen[0].query.artist_name, 'Test Signal');
  assert(fs.readdirSync(path.join(data, 'translations')).some(p => p.endsWith('.json')));
  for (const name of ['15-lyrics-synced', '15b-lyrics-plain', '16-lyrics-ask', '17-lyrics-online', '18-lyrics-translated', '20-stats-today', '21-stats-week', '22-stats-table', '30-stats-phone']) {
    assert(results.checks.includes(name), name);
    assert(fs.statSync(path.join(out, name + '.png')).size > 0, name);
  }
  console.log(results.checks.length + ' checks, one fixture lookup, translation cached.');
}

main().catch(err => { console.error(err); process.exitCode = 1; })
  .finally(() => { for (const child of children) child.kill(); });
