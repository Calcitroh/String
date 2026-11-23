use crate::{space, trim};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Line {
    /// Playback time in seconds, or None for plain text.
    pub t: Option<f64>,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParsedLyrics {
    pub synced: bool,
    pub lines: Vec<Line>,
}

fn number(s: &str, max: usize) -> Option<u32> {
    if s.is_empty() || s.len() > max || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

fn timestamp(s: &str) -> Option<u32> {
    let (min, rest) = s.split_once(':')?;
    let (sec, fraction) = rest
        .split_once(['.', ':'])
        .map_or((rest, None), |(s, f)| (s, Some(f)));
    let ms = match fraction {
        Some(f) => number(f, 3)? * 10u32.pow(3 - f.len() as u32),
        None => 0,
    };
    Some(number(min, 3)? * 60_000 + number(sec, 2)? * 1000 + ms)
}

fn words(s: &str) -> String {
    let mut plain = String::with_capacity(s.len());
    let mut rest = s;
    while let Some((before, after)) = rest.split_once('<') {
        plain.push_str(before);
        // At most "mmm:ss.fff" followed by '>'. Never search the rest of a long line.
        if let Some(end) = after.bytes().take(11).position(|b| b == b'>')
            && timestamp(&after[..end]).is_some()
        {
            rest = &after[end + 1..];
        } else {
            plain.push('<');
            rest = after;
        }
    }
    plain.push_str(rest);
    plain
        .split(space)
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn parse_lrc(text: &str) -> ParsedLyrics {
    let mut timed = Vec::new();
    let mut plain = Vec::new();
    let mut timed_rows = 0;
    let mut untimed_words = false;
    let mut offset = 0.0;
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    for raw in text.split('\n') {
        let mut rest = raw;
        let mut tagged = false;
        let mut times = Vec::new();
        while let Some((tag, after)) = rest
            .trim_start_matches(space)
            .strip_prefix('[')
            .and_then(|s| s.split_once(']'))
        {
            tagged = true;
            rest = after;
            if let Some(t) = timestamp(trim(tag)) {
                times.push(t);
            } else if let Some((key, value)) = tag.split_once(':')
                && trim(key).eq_ignore_ascii_case("offset")
            {
                let value = trim(value);
                let digits = value.strip_prefix(['+', '-']).unwrap_or(value);
                if !digits.is_empty()
                    && digits.bytes().all(|c| c.is_ascii_digit())
                    && let Ok(ms) = value.parse::<f64>()
                {
                    offset = ms;
                }
            }
        }
        let words = words(rest);
        if !times.is_empty() {
            timed_rows += 1;
            timed.extend(times.into_iter().map(|t| (t, words.clone())));
        } else if tagged && words.is_empty() {
            continue;
        } else {
            untimed_words |= !words.is_empty();
        }
        plain.push(words);
    }
    if timed_rows >= 2 || (timed_rows == 1 && !untimed_words) {
        let mut lines: Vec<_> = timed
            .into_iter()
            .map(|(t, text)| Line {
                t: Some((f64::from(t) - offset).max(0.) / 1000.),
                text,
            })
            .collect();
        lines.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap());
        let first = lines
            .iter()
            .position(|l| !l.text.is_empty())
            .unwrap_or(lines.len() - 1);
        lines.drain(..first);
        ParsedLyrics {
            synced: true,
            lines,
        }
    } else {
        let mut lines = Vec::<Line>::new();
        for text in plain {
            if text.is_empty() && lines.last().is_none_or(|l| l.text.is_empty()) {
                continue;
            }
            lines.push(Line { t: None, text });
        }
        if lines.last().is_some_and(|l| l.text.is_empty()) {
            lines.pop();
        }
        ParsedLyrics {
            synced: false,
            lines,
        }
    }
}

/// Last line that has started, or None during the intro. Use with synced lines.
pub fn line_at(lines: &[Line], seconds: f64) -> Option<usize> {
    lines
        .partition_point(|l| l.t.unwrap_or(0.) <= seconds)
        .checked_sub(1)
}
