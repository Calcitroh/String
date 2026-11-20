// A stand-in for Ollama on 127.0.0.1, for String's self-test, so the test never loads a real model.
//   node tests\fake-ollama.js [port]        (default 47914; String's self-test: --ollama http://127.0.0.1:47914)
// It has one model, "fake-translator:1b", and "translates" each numbered line as "[EN] <line>", streamed in halves
// like a real model's reply, so String's live filling-in is tested too.
'use strict';
const http = require('http');
const port = +process.argv[2] || 47914;
const sleep = ms => new Promise(r => setTimeout(r, ms));
http.createServer(async (req, res) => {
  const u = new URL(req.url, 'http://x');
  res.setHeader('content-type', 'application/json');
  if (u.pathname === '/api/version') { res.end(JSON.stringify({ version: '0.0.0-fake' })); return; }
  if (u.pathname === '/api/tags') { res.end(JSON.stringify({ models: ['fake-translator:1b', 'cloud-alias', 'unverified-model'].map(name => ({ name })) })); return; }
  if (u.pathname === '/api/show') {
    let body = '';
    for await (const chunk of req) body += chunk;
    const model = JSON.parse(body).model;
    const info = model === 'cloud-alias' ? { remote_host: 'https://example.invalid', remote_model: 'provider', model_info: { family: 'test' } }
      : model === 'fake-translator:1b' ? { model_info: { 'general.architecture': 'test' } } : {};
    res.end(JSON.stringify(info)); return;
  }
  if (u.pathname === '/api/chat' && req.method === 'POST') {
    let body = '';
    for await (const c of req) body += c;
    let j = {};
    try { j = JSON.parse(body); } catch { }
    if (j.model !== 'fake-translator:1b') { res.statusCode = 404; res.end(JSON.stringify({ error: `model "${j.model}" not found` })); return; }
    const user = (j.messages || []).filter(m => m.role === 'user').pop() || { content: '' };
    const lines = user.content.split('\n').map(l => l.match(/^(\d+)\| (.*)$/)).filter(Boolean);
    res.setHeader('content-type', 'application/x-ndjson');
    const chunk = content => res.write(JSON.stringify({ model: j.model, message: { role: 'assistant', content }, done: false }) + '\n');
    for (const [, n, text] of lines) {
      const out = `${n}| [EN] ${text}\n`, half = Math.floor(out.length / 2);
      chunk(out.slice(0, half)); await sleep(25);
      chunk(out.slice(half)); await sleep(25);
    }
    res.end(JSON.stringify({ model: j.model, message: { role: 'assistant', content: '' }, done: true }) + '\n');
    return;
  }
  res.statusCode = 404;
  res.end('{}');
}).listen(port, '127.0.0.1', () => console.log('fake Ollama listening on ' + port));
