// Exercise the shipped page's actual cache readers, writers and hash.
'use strict';
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const assert = require('node:assert/strict');
const [mode, dir] = process.argv.slice(2);
const lines = ['音 🎵', '♪', '音 🎵', '空'];
const sidecar = '[00:02]音 🎵\n[00:03]空';
const song = {id:'track-🎵', path:path.join(dir,'song.mp3'), T:'Glass Orbit', A:'Test Signal', duration:24.49};
const native = {
  paths: async () => ({data:dir}),
  readText: async file => {
    if (file.endsWith('.lrc') && mode === 'write') return sidecar;
    return fs.existsSync(file) ? fs.readFileSync(file, 'utf8') : null;
  },
  writeText: async (file, text) => {
    assert.equal(mode, 'write');
    fs.mkdirSync(path.dirname(file), {recursive:true});
    fs.writeFileSync(file, text);
  },
  ollamaChat: async (_, {onLine}) => {
    assert.equal(mode, 'write');
    onLine(JSON.stringify({message:{content:'1| [EN] 音 🎵\n2| [EN] 空\n'}}));
    return {done:Promise.resolve(0)};
  }
};
const page = vm.createContext({N:{S:{set:{}}}, belt:{native}, setTimeout});
page.window = page;
for (const name of ['art','lyrics','translate']) {
  vm.runInContext(fs.readFileSync(path.resolve(__dirname, '../../../src/wwwroot', name + '.js'), 'utf8'), page);
}
(async () => {
  const r = await page.N.lyrics.get(song);
  assert.equal(r.source, 'lrc');
  assert.equal(r.text, sidecar);
  const t = await page.N.translate.run(song, lines, {lang:'English', model:'fake-translator:1b'});
  assert.equal(t.cached, mode === 'read');
  assert.deepEqual(Array.from(t.lines), ['[EN] 音 🎵', null, '[EN] 音 🎵', '[EN] 空']);
})().catch(error => { console.error(error); process.exitCode = 1; });
