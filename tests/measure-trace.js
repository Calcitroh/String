// Called by measure.ps1 after the CPU samples. Connect only to this run's WebView.
'use strict';
const fs = require('node:fs');
const path = require('node:path');
const { createGzip } = require('node:zlib');
const { pipeline } = require('node:stream/promises');
const { once } = require('node:events');

async function main(run, phase) {
  const profile = path.join(run, 'data', 'webview', 'EBWebView');
  const [port, endpoint] = fs.readFileSync(path.join(profile, 'DevToolsActivePort'), 'utf8').trim().split(/\r?\n/);
  const origin = 'http://127.0.0.1:' + Number(port);
  const version = await (await fetch(origin + '/json/version')).json();
  if (new URL(version.webSocketDebuggerUrl).pathname !== endpoint) throw Error('Debug endpoint changed');
  const targets = await (await fetch(origin + '/json/list')).json();
  const pages = targets.filter(target => target.type === 'page');
  if (pages.length !== 1) throw Error('Expected one smoke page');
  const socket = new WebSocket(pages[0].webSocketDebuggerUrl);
  await once(socket, 'open');
  let seq = 0, complete;
  const pending = new Map();
  socket.onmessage = e => {
    const msg = JSON.parse(e.data);
    if (msg.method === 'Tracing.tracingComplete') complete(msg.params);
    if (!msg.id) return;
    const { resolve, reject } = pending.get(msg.id);
    pending.delete(msg.id);
    if (msg.error) reject(Error(msg.error.message));
    else resolve(msg.result);
  };
  function send(method, params = {}) {
    return new Promise((resolve, reject) => {
      pending.set(++seq, { resolve, reject });
      socket.send(JSON.stringify({ id: seq, method, params }));
    });
  }
  async function mark(name) {
    const { result } = await send('Runtime.evaluate', { expression:
      `performance.mark('sample-${name}'); [N.viz.frames, performance.now()]`, returnByValue: true });
    return result.value;
  }
  try {
    const ended = new Promise(resolve => { complete = resolve; });
    await send('Tracing.start', { categories: 'devtools.timeline,blink.user_timing,cc,gpu,viz,benchmark',
      transferMode: 'ReturnAsStream' });
    const begin = await mark('begin');
    await new Promise(resolve => setTimeout(resolve, 2500));
    const end = await mark('end');
    await send('Tracing.end');
    const { stream, dataLossOccurred } = await ended;
    if (dataLossOccurred) throw Error('Trace dropped events');
    async function* chunks() {
      for (;;) {
        const chunk = await send('IO.read', { handle: stream, size: 1024 * 1024 });
        yield Buffer.from(chunk.data, chunk.base64Encoded ? 'base64' : 'utf8');
        if (chunk.eof) return;
      }
    }
    await pipeline(chunks(), createGzip(), fs.createWriteStream(path.join(run, phase + '.trace.json.gz')));
    await send('IO.close', { handle: stream });
    fs.writeFileSync(path.join(run, phase + '.frames.json'), JSON.stringify({ frames: end[0] - begin[0], ms: end[1] - begin[1] }));
  } finally { socket.close(); }
}
const timeout = setTimeout(() => { console.error('Trace timed out'); process.exit(1); }, 25000);
main(...process.argv.slice(2)).catch(e => { console.error(e); process.exitCode = 1; }).finally(() => clearTimeout(timeout));
