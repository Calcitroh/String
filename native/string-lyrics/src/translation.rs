use crate::{space, trim};
use serde_json::{Value, json};
use std::{collections::HashMap, fmt::Write};

pub const LANGUAGES: &[&str] = &[
    "English",
    "Spanish",
    "Portuguese",
    "French",
    "German",
    "Italian",
    "Japanese",
    "Korean",
    "Chinese (Simplified)",
    "Chinese (Traditional)",
    "Russian",
    "Ukrainian",
    "Polish",
    "Dutch",
    "Turkish",
    "Arabic",
    "Hindi",
    "Indonesian",
    "Vietnamese",
    "Thai",
    "Tagalog",
];

#[derive(Debug, Clone, Copy)]
pub struct TranslationOptions<'a> {
    pub lang: &'a str,
    pub model: &'a str,
}
#[derive(Debug, Default)]
pub struct Translation {
    pub lines: Vec<Option<String>>,
    pub cached: bool,
    pub same: bool,
    pub stopped: bool,
    pub error: Option<String>,
}
#[derive(Debug)]
pub struct Plan<'a> {
    pub uniq: Vec<&'a str>,
    pub map: Vec<Option<usize>>,
}

fn sounds(s: &str) -> bool {
    if s.chars().all(|c| space(c) || "♪~….\"'!?,-".contains(c)) {
        return true;
    }
    let lower = s.to_ascii_lowercase();
    let mut rest = lower.as_str();
    while !rest.is_empty() {
        if rest.starts_with("mm") {
            rest = rest.trim_start_matches('m');
        } else if let Some(next) = [
            "la", "na", "da", "oh", "ooh", "ah", "yeah", "hey", "woah", "whoa", "uh",
        ]
        .iter()
        .find_map(|s| rest.strip_prefix(s))
        {
            rest = next;
        } else {
            return false;
        }
        rest = rest.trim_start_matches(|c| space(c) || ",!.~-".contains(c));
    }
    true
}

pub fn plan<S: AsRef<str>>(lines: &[S]) -> Plan<'_> {
    let mut uniq = Vec::new();
    let mut index = HashMap::new();
    let map = lines
        .iter()
        .map(|s| {
            let t = trim(s.as_ref());
            if sounds(t) {
                return None;
            }
            Some(*index.entry(t).or_insert_with(|| {
                uniq.push(t);
                uniq.len() - 1
            }))
        })
        .collect();
    Plan { uniq, map }
}
impl Plan<'_> {
    pub(crate) fn spread(&self, out: &[Option<String>]) -> Vec<Option<String>> {
        self.map
            .iter()
            .map(|k| k.and_then(|i| out[i].clone()))
            .collect()
    }
}

pub fn messages(uniq: &[&str], lang: &str, title: &str, artist: &str) -> Value {
    let system = format!(
        "You translate song lyrics into {lang}. Translate each numbered line so it reads naturally in {lang} and keeps the meaning, feeling and imagery of the song: not word for word. Use the other lines for context. Keep names as they are. A line already in {lang}, or only sounds, stays as it is. Answer with exactly one line per numbered line, in the same order, each written as \"<number>| <translation>\", and nothing else: no notes, no romanization, no quotation marks."
    );
    let title = if title.is_empty() { "Untitled" } else { title };
    let mut user = format!("Song: \"{title}\"");
    if !artist.is_empty() {
        write!(user, " by {artist}").unwrap();
    }
    user.push_str("\n\n");
    for (i, line) in uniq.iter().enumerate() {
        if i > 0 {
            user.push('\n');
        }
        write!(user, "{}| {line}", i + 1).unwrap();
    }
    json!([{"role":"system","content":system},{"role":"user","content":user}])
}

pub fn thinks(model: &str) -> bool {
    let lower = model.to_ascii_lowercase();
    [
        "qwen3",
        "deepseek-r1",
        "magistral",
        "gpt-oss",
        "phi4-reasoning",
    ]
    .iter()
    .any(|s| lower.contains(s))
}
pub(crate) fn cloud_name(model: &str) -> bool {
    let lower = model.to_ascii_lowercase();
    [":cloud", "-cloud", "@cloud"]
        .iter()
        .any(|s| lower.ends_with(s))
}
pub fn pick_model<S: AsRef<str>>(models: &[S]) -> Option<&str> {
    const PREFS: &[&str] = &[
        "qwen3:14b",
        "qwen3:32b",
        "qwen3:8b",
        "qwen3",
        "qwen2.5",
        "gemma3",
        "llama3",
        "mistral",
    ];
    models
        .iter()
        .map(AsRef::as_ref)
        .filter(|m| !cloud_name(m))
        .min_by_key(|m| {
            let m = m.to_ascii_lowercase();
            PREFS
                .iter()
                .position(|prefix| m.starts_with(prefix))
                .unwrap_or(PREFS.len())
        })
}

fn skip(mut s: &str, predicate: impl Fn(char) -> bool, count: usize) -> &str {
    for _ in 0..count {
        let Some(c) = s.chars().next().filter(|&c| predicate(c)) else {
            break;
        };
        s = &s[c.len_utf8()..];
    }
    s
}
fn numbered(raw: &str) -> Option<(usize, Option<&str>)> {
    let s = skip(trim(raw), |c| c == '*', 4);
    let digits = skip(s, space, 4);
    let end = digits
        .bytes()
        .take(4)
        .take_while(u8::is_ascii_digit)
        .count();
    let index = digits[..end].parse::<usize>().ok()?.checked_sub(1)?;
    let s = skip(&digits[end..], space, 4);
    let s = skip(s, |c| c == '*', 4);
    let s = skip(s, space, 4);
    let words = trim(s.strip_prefix(['|', '.', ')', ':', '-', '–'])?);
    if words.is_empty() {
        return None;
    }
    let words = trim(words.trim_matches(['"', '“', '”', '\'']));
    Some((index, (!words.is_empty()).then_some(words)))
}

/// A chunk may end inside a number, a tag or a word. Only scan a raw line once,
/// when its newline arrives. Newlines inside thinking blocks disappear too.
#[derive(Default)]
pub(crate) struct ReplyParser {
    raw: String,
    visible: String,
    thinking: bool,
}
impl ReplyParser {
    pub(crate) fn push(&mut self, chunk: &str, mut emit: impl FnMut(usize, Option<&str>)) {
        let mut rest = chunk;
        while let Some((part, after)) = rest.split_once('\n') {
            self.raw.push_str(part);
            self.filter_line();
            if !self.thinking {
                self.emit(&mut emit);
            }
            rest = after;
        }
        self.raw.push_str(rest);
    }
    pub(crate) fn finish(&mut self, mut emit: impl FnMut(usize, Option<&str>)) {
        self.filter_line();
        self.emit(&mut emit);
    }
    fn filter_line(&mut self) {
        let mut s = self.raw.as_str();
        loop {
            let tag = if self.thinking { "</think>" } else { "<think>" };
            let Some((before, after)) = s.split_once(tag) else {
                break;
            };
            if !self.thinking {
                self.visible.push_str(before);
            }
            self.thinking = !self.thinking;
            s = after;
        }
        if !self.thinking {
            self.visible.push_str(s);
        }
        self.raw.clear();
    }
    fn emit(&mut self, emit: &mut impl FnMut(usize, Option<&str>)) {
        if let Some((i, words)) = numbered(&self.visible) {
            emit(i, words);
        }
        self.visible.clear();
    }
}
pub fn parse_reply(text: &str, count: usize) -> Vec<Option<String>> {
    let mut out = vec![None; count];
    let mut accept = |i: usize, words: Option<&str>| {
        if let Some(slot) = out.get_mut(i) {
            *slot = words.map(str::to_owned);
        }
    };
    let mut parser = ReplyParser::default();
    parser.push(text, &mut accept);
    parser.finish(accept);
    out
}
