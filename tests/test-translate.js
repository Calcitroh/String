// Checks src\wwwroot\translate.js's text rules: which lines are sent, the request, reading the streamed reply.
//   node tests\test-translate.js
'use strict';
const X = require('../src/wwwroot/translate.js');
let pass = 0, fail = 0;
const check = (name, ok, extra) => { if (ok) pass++; else fail++; console.log(`${ok ? 'ok  ' : 'FAIL'} ${name}${ok || extra === undefined ? '' : '  ' + JSON.stringify(extra)}`); };

const lines = ['君の名前を', '♪', '', 'La la la', 'Oh, oh!', '君の名前を', '  空を見て  ', 'Yeah yeah'];
const p = X.plan(lines);
check('each line once, sounds and blanks left out', JSON.stringify(p.uniq) === JSON.stringify(['君の名前を', '空を見て']), p.uniq);
check('every lyric line maps back', JSON.stringify(p.map) === '[0,-1,-1,-1,-1,0,1,-1]', p.map);

const m = X.messages(p.uniq, { lang: 'English', title: 'Song Title', artist: 'Someone' });
check('a system rule and the numbered lines', m.length === 2 && m[0].role === 'system' && /into English/.test(m[0].content) && m[1].content.endsWith('1| 君の名前を\n2| 空を見て'), m[1].content);
check('the request carries only the song and its lines', m[1].content.startsWith('Song: "Song Title" by Someone') && !/\\|path|mp3/i.test(m[1].content));

const reply = '<think>\n</think>\n\n1| Your name\n2. Looking at the sky\n';
check('reads the numbered reply, ignoring <think>', JSON.stringify(X.parseReply(reply, 2)) === '["Your name","Looking at the sky"]', X.parseReply(reply, 2));
check('other numberings and stray quotes', JSON.stringify(X.parseReply('**1**) "Your name"\n2 - sky\n3| ignored', 2)) === '["Your name","sky"]', X.parseReply('**1**) "Your name"\n2 - sky\n3| ignored', 2));
check('a line still missing stays empty', JSON.stringify(X.parseReply('2| only two', 3)) === '[null,"only two",null]');
check('an unfinished <think> hides everything', JSON.stringify(X.parseReply('<think>1| no', 1)) === '[null]');
check('think blocks between lines go, the lines stay', JSON.stringify(X.parseReply('1| a\n<think>x</think>\n2| b\n<think>y', 2)) === '["a","b"]', X.parseReply('1| a\n<think>x</think>\n2| b\n<think>y', 2));
// odd model output must not freeze the page: walls of spaces, quotes and unclosed tags, read in a moment
const wall = ' '.repeat(60000);
const odd = ['1' + wall + 'x', '2|' + wall + 'word' + wall + '!', '3| ' + '"'.repeat(40000) + 'q' + '"'.repeat(40000), '<think>'.repeat(20000), '*'.repeat(30000) + '4| z'].join('\n');
const t0 = Date.now();
const got = X.parseReply(odd, 4);
const ms = Date.now() - t0;
check('a pathological reply is read quickly', ms < 300, { ms });
check('…and correctly', got[0] === null && got[1] === 'word' + wall + '!' && got[2] === 'q' && got[3] === null, got.map(x => x && x.length));

check('thinking models are asked not to', X.thinks('qwen3:14b') && X.thinks('deepseek-r1:8b') && !X.thinks('llama3.1:8b'));
check('Qwen 3 14B is the default when it is there', X.pickModel(['llama3.1:8b', 'qwen3:32b', 'qwen3:14b', 'qwen3:8b']) === 'qwen3:14b');
check('else the next best', X.pickModel(['llama3.1:8b', 'qwen3:8b']) === 'qwen3:8b' && X.pickModel(['phi3:mini']) === 'phi3:mini' && X.pickModel([]) === '');

console.log(`\n${pass} of ${pass + fail} checks passed`);
process.exit(fail ? 1 : 0);
