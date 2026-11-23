//! JavaScript sorts and slices front-coded paths in UTF-16 code units.

use serde_json::value::RawValue;
use std::collections::HashMap;
use std::fmt::Write;

pub(crate) type Entry = (usize, Box<RawValue>, usize);

fn quoted(units: &[u16]) -> Box<RawValue> {
    let mut s = String::from("\"");
    for ch in char::decode_utf16(units.iter().copied()) {
        match ch {
            Ok(c @ ('"' | '\\')) => {
                s.push('\\');
                s.push(c);
            }
            Ok(c) if c >= ' ' => s.push(c),
            Ok(c) => write!(s, "\\u{:04x}", c as u32).unwrap(),
            Err(e) => write!(s, "\\u{:04x}", e.unpaired_surrogate()).unwrap(),
        }
    }
    s.push('"');
    RawValue::from_string(s).expect("encoded JSON string")
}

// RawValue has already checked JSON syntax. Keep escaped surrogate halves until the path is whole.
fn units(raw: &RawValue) -> Result<Vec<u16>, String> {
    let inner = raw
        .get()
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .ok_or("expected path string")?;
    let mut out = Vec::new();
    let mut chars = inner.chars();
    while let Some(mut c) = chars.next() {
        if c == '\\' {
            c = match chars.next() {
                Some('u') => {
                    let hex: String = chars.by_ref().take(4).collect();
                    out.push(u16::from_str_radix(&hex, 16).map_err(|e| e.to_string())?);
                    continue;
                }
                Some('b') => '\x08',
                Some('f') => '\x0c',
                Some('n') => '\n',
                Some('r') => '\r',
                Some('t') => '\t',
                Some(c) => c,
                None => return Err("incomplete escape".into()),
            };
        }
        out.extend_from_slice(c.encode_utf16(&mut [0; 2]));
    }
    Ok(out)
}

pub(crate) fn compress<'a>(
    ids: impl Iterator<Item = &'a str>,
) -> (Vec<Entry>, HashMap<&'a str, i64>) {
    let mut unique: Vec<_> = ids.collect();
    unique.sort_unstable_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
    unique.dedup();
    let mut prev = Vec::new();
    let mut entries = Vec::with_capacity(unique.len());
    let mut indices = HashMap::with_capacity(unique.len());
    for id in unique {
        indices.insert(id, entries.len() as i64);
        let s: Vec<_> = id.encode_utf16().collect();
        let front = s.iter().zip(&prev).take_while(|(a, b)| a == b).count();
        let back = s[front..]
            .iter()
            .rev()
            .zip(prev[front..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        entries.push((front, quoted(&s[front..s.len() - back]), back));
        prev = s;
    }
    (entries, indices)
}

pub(crate) fn expand(entries: &[Entry]) -> Result<Vec<String>, String> {
    let mut prev = Vec::new();
    let mut paths = Vec::with_capacity(entries.len());
    for (front, mid, back) in entries {
        if *front > prev.len() || *back > prev.len() - front {
            return Err("invalid shared path length".into());
        }
        let mut next = prev[..*front].to_vec();
        next.extend(units(mid)?);
        next.extend_from_slice(&prev[prev.len() - back..]);
        paths.push(String::from_utf16(&next).map_err(|e| e.to_string())?);
        prev = next;
    }
    Ok(paths)
}
