/* String translations: the lyrics of the song playing, translated by a model on this PC through Ollama (nothing
   leaves the computer). The whole song goes in one request, so the model sees the context and can translate meaning
   rather than words; repeated lines (a chorus) go once. The reply streams in as "N| text" lines, which show up under
   the lyrics as they arrive. Results are kept in the data folder's translations\, per lyrics, language and model.
   The text rules (plan, messages, parseReply) are pure and run in Node: tests\test-translate.js. */
(function (root) {
  'use strict';
  const SOUNDS = /^[\s♪~…."'!?,-]*$|^((la|na|da|oh|ooh|ah|yeah|hey|woah|whoa|mm+|uh)[\s,!.~-]*)+$/i;

  // the lines worth translating, each once, in order; map[i] = which one lyric line i is (-1: none, e.g. ♪)
  function plan(lines) {
    const uniq = [], index = new Map(), map = [];
    for (const raw of lines) {
      const t = String(raw || '').trim();
      if (!t || SOUNDS.test(t)) { map.push(-1); continue; }
      let k = index.get(t);
      if (k === undefined) { k = uniq.length; uniq.push(t); index.set(t, k); }
      map.push(k);
    }
    return { uniq, map };
  }

  function messages(uniq, { lang, title, artist }) {
    const system = `You translate song lyrics into ${lang}. Translate each numbered line so it reads naturally in ${lang} ` +
      `and keeps the meaning, feeling and imagery of the song: not word for word. Use the other lines for context. Keep ` +
      `names as they are. A line already in ${lang}, or only sounds, stays as it is. Answer with exactly one line per ` +
      `numbered line, in the same order, each written as "<number>| <translation>", and nothing else: no notes, no ` +
      `romanization, no quotation marks.`;
    const head = `Song: "${title || 'Untitled'}"${artist ? ` by ${artist}` : ''}\n\n`;
    return [{ role: 'system', content: system }, { role: 'user', content: head + uniq.map((t, i) => `${i + 1}| ${t}`).join('\n') }];
  }

  // A model's <think>…</think> blocks dropped (and an unfinished one, to the end), by searching, not a pattern: a reply
  // full of unclosed tags would otherwise be scanned again for each one
  function dropThinking(s) {
    let out = '', i = 0;
    for (;;) {
      const a = s.indexOf('<think>', i);
      if (a < 0) return out + s.slice(i);
      out += s.slice(i, a);
      const b = s.indexOf('</think>', a + 7);
      if (b < 0) return out;
      i = b + 8;
    }
  }
  const QUOTES = '"“”\'';
  function unquote(s) {
    let a = 0, b = s.length;
    while (a < b && QUOTES.includes(s[a])) a++;
    while (b > a && QUOTES.includes(s[b - 1])) b--;
    return s.slice(a, b);
  }
  // "N| text" lines (also "N. text", "N) text", "N: text") into an array of n translations; a model's <think> is dropped.
  // Every pattern here takes time in step with the line (bounded spaces, the rest trimmed by hand), so an odd line from
  // the model (thousands of spaces, a wall of quotes) can't freeze the page.
  const LINE_HEAD = /^\*{0,4}\s{0,4}(\d{1,4})\s{0,4}\*{0,4}\s{0,4}[|.):\-–]/;
  function parseReply(text, n) {
    const out = new Array(n).fill(null);
    for (const raw of dropThinking(String(text || '')).split('\n')) {
      const line = raw.trim();
      const m = LINE_HEAD.exec(line);
      if (!m) continue;
      const i = +m[1] - 1;
      const words = line.slice(m[0].length).trim();
      if (i >= 0 && i < n && words) out[i] = unquote(words).trim() || null;
    }
    return out;
  }

  // a model that thinks before it answers (Qwen 3 and the like) is asked not to: lyrics need no reasoning
  const thinks = model => /qwen3|deepseek-r1|magistral|gpt-oss|phi4-reasoning/i.test(model || '');

  // the downloaded model to use when none was chosen: Qwen 3 14B first (a good balance on a big graphics card)
  function pickModel(models) {
    models = (models || []).filter(name => !/(?:[:-]cloud|@cloud)$/i.test(name));
    const prefs = [/^qwen3:14b/i, /^qwen3:32b/i, /^qwen3:8b/i, /^qwen3/i, /^qwen2\.5/i, /^gemma3/i, /^llama3/i, /^mistral/i];
    for (const p of prefs) { const m = (models || []).find(x => p.test(x)); if (m) return m; }
    return (models || [])[0] || '';
  }

  const api = { plan, messages, parseReply, thinks, pickModel };
  if (typeof module === 'object' && module.exports) { module.exports = api; return; }
  root.StringTranslate = api;

  /* ---------- in the page ---------- */
  const N = root.N;
  let dataDir = '', job = null;

  async function cachePath(text, lang, model) {
    dataDir = dataDir || (await belt.native.paths()).data;
    const key = N.art.hash(`${model}\n${lang}\n${text}`).toString(16) + '-' + text.length;
    return `${dataDir}\\translations\\${key}.json`;
  }

  N.translate = {
    LANGS: ['English', 'Spanish', 'Portuguese', 'French', 'German', 'Italian', 'Japanese', 'Korean', 'Chinese (Simplified)', 'Chinese (Traditional)',
      'Russian', 'Ukrainian', 'Polish', 'Dutch', 'Turkish', 'Arabic', 'Hindi', 'Indonesian', 'Vietnamese', 'Thai', 'Tagalog'],
    pickModel,
    models: () => belt.native.ollamaModels(),
    stop() { if (job) { job.stopped = true; if (job.chat) job.chat.kill(); job = null; } },
    // translates these lyric lines; onUpdate(perLine) as the reply streams; resolves to { lines, cached, same, error }
    async run(t, lines, { lang, model, onUpdate }) {
      N.translate.stop();
      const me = { stopped: false, chat: null };
      job = me;
      const { uniq, map } = plan(lines);
      const spread = got => map.map(k => (k < 0 ? null : got[k]));
      if (!uniq.length) return { lines: spread([]), cached: false, same: true };
      const text = uniq.join('\n'), path = await cachePath(text, lang, model);
      try {
        const c = JSON.parse(await belt.native.readText(path) || 'null');
        if (c && c.v === 1 && Array.isArray(c.out) && c.out.length === uniq.length) {
          if (job === me) job = null;
          onUpdate && onUpdate(spread(c.out));
          return { lines: spread(c.out), cached: true, same: c.same };
        }
      } catch { }
      let reply = '', error = '', parsedUpTo = 0;
      const request = { model, messages: messages(uniq, { lang, title: t.T, artist: t.A }), stream: true, keep_alive: '3m', options: { temperature: .2, num_ctx: 8192 } };
      if (thinks(model)) request.think = false;
      try {
        me.chat = await belt.native.ollamaChat(request, {
          onLine: line => {
            if (me.stopped) return;
            let j;
            try { j = JSON.parse(line); } catch { return; }
            if (j.error) { error = String(j.error); return; }
            if (j.message && j.message.content) {
              reply += j.message.content;
              // only whole lines (the last one may still be growing), and only when a new one has come in
              const end = reply.lastIndexOf('\n') + 1;
              if (end > parsedUpTo && onUpdate) { parsedUpTo = end; onUpdate(spread(parseReply(reply.slice(0, end), uniq.length))); }
            }
          },
        });
        const code = await me.chat.done;
        if (me.stopped) return { lines: [], stopped: true };
        if (code !== 0 && !error) error = 'The translation stopped';
      } catch (e) { error = e.message; }
      if (job === me) job = null;
      const got = parseReply(reply, uniq.length);
      onUpdate && onUpdate(spread(got));
      const filled = got.filter(Boolean).length;
      if (error && !filled) return { lines: spread(got), error };
      const same = filled > 0 && got.every((x, i) => !x || x.toLowerCase() === uniq[i].toLowerCase());
      if (filled >= uniq.length * .8) {
        try { await belt.native.writeText(path, JSON.stringify({ v: 1, model, lang, same, out: got, at: Date.now() })); } catch { }
      }
      return { lines: spread(got), cached: false, same, error: filled < uniq.length ? error || `${uniq.length - filled} lines came back empty` : '' };
    },
  };
})(typeof window !== 'undefined' ? window : globalThis);
