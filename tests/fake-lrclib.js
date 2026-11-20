// A stand-in for LRCLIB on 127.0.0.1, for String's self-test, so the test never asks the real lrclib.net.
//   node tests\fake-lrclib.js [port]        (default 47913; String's self-test: --lrclib http://127.0.0.1:47913)
// It knows one song (Glass Orbit by Test Signal) with synced lyrics; /api/_seen says what was asked and with which
// User-Agent, so the self-test can check String says who it is.
'use strict';
const http = require('http');
const port = +process.argv[2] || 47913;
const GLASS = {
  id: 1, name: 'Glass Orbit', trackName: 'Glass Orbit', artistName: 'Test Signal', albumName: 'Synthetic Weather', duration: 24,
  instrumental: false, plainLyrics: 'Glass orbit online one\nGlass orbit online two\nGlass orbit online three',
  syncedLyrics: '[00:02.00]Glass orbit online one\n[00:06.00]Glass orbit online two\n[00:10.00]Glass orbit online three',
};
const seen = [];
http.createServer((req, res) => {
  const u = new URL(req.url, 'http://x');
  res.setHeader('content-type', 'application/json');
  if (u.pathname === '/api/_seen') { res.end(JSON.stringify(seen)); return; }
  seen.push({ path: u.pathname, query: Object.fromEntries(u.searchParams), ua: req.headers['user-agent'] || '', client: req.headers['lrclib-client'] || '' });
  const track = u.searchParams.get('track_name') || '', artist = u.searchParams.get('artist_name') || '';
  if (u.pathname === '/api/get') {
    if (/^glass orbit$/i.test(track) && /^test signal$/i.test(artist)) { res.end(JSON.stringify(GLASS)); return; }
    res.statusCode = 404;
    res.end(JSON.stringify({ code: 404, name: 'TrackNotFound', message: 'Failed to find specified track' }));
    return;
  }
  if (u.pathname === '/api/search') { res.end('[]'); return; }
  res.statusCode = 404;
  res.end('{}');
}).listen(port, '127.0.0.1', () => console.log('fake LRCLIB listening on ' + port));
