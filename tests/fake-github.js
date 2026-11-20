// A fake GitHub for String's updater in the self-test (tests\run-selftest.ps1): never the real one. It makes a release
// key of its own (the public half goes to <dir>\update-key.txt, for String's --update-key) and serves, from the zips
// run-selftest.ps1 made in <dir>:
//   /repos/good/releases/latest      v9.9.9, signed                    → installs
//   /repos/tampered/releases/latest  v9.9.9, a byte changed after signing → refused
//   /repos/liar/releases/latest      v9.9.9 outside, 9.9.8 inside, signed → refused
//   /repos/old/releases/latest       v0.0.1                            → not newer
//   /repos/none/releases/latest      404 (nothing released yet)
//   node tests\fake-github.js <dir> [port]
'use strict';
const http = require('http'), fs = require('fs'), path = require('path'), crypto = require('crypto');
const dir = process.argv[2], port = +(process.argv[3] || 47915);
const { privateKey, publicKey } = crypto.generateKeyPairSync('ec', { namedCurve: 'P-256' });
fs.writeFileSync(path.join(dir, 'update-key.txt'), publicKey.export({ type: 'spki', format: 'der' }).toString('base64'));
const sign = buf => crypto.sign('sha256', buf, { key: privateKey, dsaEncoding: 'ieee-p1363' }).toString('base64');
const good = fs.readFileSync(path.join(dir, 'good.zip')), liar = fs.readFileSync(path.join(dir, 'liar.zip'));
const tampered = Buffer.from(good); tampered[Math.floor(tampered.length / 2)] ^= 0xff;
const files = { 'good.zip': good, 'good.sig': sign(good), 'tampered.zip': tampered, 'liar.zip': liar, 'liar.sig': sign(liar) };
const base = `http://127.0.0.1:${port}`;
const release = (tag, zip, sig, size) => ({ tag_name: tag, draft: false, prerelease: false, body: 'Fake notes for ' + tag,
  assets: [{ name: 'String.zip', size, browser_download_url: `${base}/dl/${zip}` }, { name: 'String.zip.sig', size: 88, browser_download_url: `${base}/dl/${sig}` }] });
const feeds = {
  good: release('v9.9.9', 'good.zip', 'good.sig', good.length),
  tampered: release('v9.9.9', 'tampered.zip', 'good.sig', tampered.length),
  liar: release('v9.9.9', 'liar.zip', 'liar.sig', liar.length),
  old: release('v0.0.1', 'good.zip', 'good.sig', good.length),
};
http.createServer((req, res) => {
  const u = new URL(req.url, base);
  let m = u.pathname.match(/^\/repos\/(\w+)\/releases\/latest$/);
  if (m && feeds[m[1]]) { res.setHeader('content-type', 'application/json'); res.end(JSON.stringify(feeds[m[1]])); return; }
  m = u.pathname.match(/^\/dl\/([\w.]+)$/);
  if (m && files[m[1]] !== undefined) { const b = Buffer.from(files[m[1]]); res.setHeader('content-length', b.length); res.end(b); return; }
  res.statusCode = 404; res.end('{"message":"Not Found"}');
}).listen(port, '127.0.0.1');
