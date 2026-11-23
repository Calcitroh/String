// Run the repository's fakes on ephemeral ports. Tests can also select error
// responses and inspect the request body without modifying the shared fixtures.
'use strict';
const http = require('node:http');
const path = require('node:path');
const [kind, scenario = 'normal', port = '0'] = process.argv.slice(2);
const seen = [];
let runner;
if (scenario === 'tree') {
  // A stand-in for a model runner. Its separate server lets the test observe
  // whether it survives application shutdown. The timeout also bounds failures.
  const child = require('node:child_process').spawn(process.execPath, ['-e',
    "const s=require('node:net').createServer(c=>c.end()); s.listen(0,'127.0.0.1',()=>console.log(s.address().port)); setTimeout(()=>s.close(),30000)"
  ], {windowsHide:true, stdio:['ignore','pipe','ignore']});
  runner = new Promise(resolve => child.stdout.once('data', s => resolve(Number(s.toString().trim()))));
}
const create = http.createServer;
http.createServer = handler => {
  const server = create(async (req, res) => {
    const url = new URL(req.url, 'http://127.0.0.1');
    if (url.pathname === '/_requests') return res.end(JSON.stringify(seen));
    if (url.pathname === '/_runner' && runner) return res.end(String(await runner));
    const request = { path: url.pathname };
    seen.push(request);
    if (kind === 'lrclib' && scenario !== 'normal') {
      if (scenario === 'search' && url.pathname === '/api/search') {
        return res.end(JSON.stringify([
          {name:'Glass Orbit',artistName:'Test Signal',duration:90,plainLyrics:'far'},
          {name:'Glass Orbit (Live)',artistName:'Test Signal',duration:25,syncedLyrics:'[00:01]match'}
        ]));
      }
      if (scenario === 'instrumental') return res.end('{"instrumental":true}');
      if (scenario === 'invalid') return res.end('broken');
      res.statusCode = scenario === 'search' ? 404 : Number(scenario);
      return res.end('{}');
    }
    if (kind === 'ollama' && scenario !== 'normal') {
      if (url.pathname === '/api/show') return res.end('{"model_info":{"family":"test"}}');
      if (url.pathname === '/api/chat') {
        let body = '';
        for await (const part of req) body += part;
        request.body = JSON.parse(body);
        const event = content => JSON.stringify({message:{content}}) + '\n';
        res.writeHead(200, {'content-type':'application/x-ndjson'});
        if (scenario === 'stall') {
          res.write(event('1| First\n'));
          const timeout = setTimeout(() => res.end(), 8000);
          res.on('close', () => clearTimeout(timeout));
          return; // leave the socket open until the client cancels
        }
        res.write('malformed event\n' + event('<thi') + event('nk>hidden\n') + event('</think>**1**) “ONE”\n2: Two\n'));
        if (scenario === 'four') res.write(event('3 - Three\n4– Four'));
        res.end('{"error":"fixture ended"}\n');
        return;
      }
    }
    return handler(req, res);
  });
  const listen = server.listen.bind(server);
  server.listen = () => listen(Number(port), '127.0.0.1', () => console.log(server.address().port));
  return server;
};
require(path.resolve(__dirname, '../../../tests', 'fake-' + kind + '.js'));
