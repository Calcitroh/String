//! JavaScript sorts and slices front-coded paths in UTF-16 code units.

use crate::data::text;
use std::collections::HashMap;

pub(crate) type Entry = (usize, String, usize);

pub(crate) fn compress<'a>(
    ids: impl Iterator<Item = &'a str>,
) -> (Vec<Entry>, HashMap<&'a str, i64>) {
    let mut unique: Vec<_> = ids.collect();
    unique.sort_unstable_by(|a, b| {
        if a.contains('\0') || b.contains('\0') {
            text::units(a).cmp(&text::units(b))
        } else {
            a.encode_utf16().cmp(b.encode_utf16())
        }
    });
    unique.dedup();
    let mut prev = Vec::new();
    let mut entries = Vec::with_capacity(unique.len());
    let mut indices = HashMap::with_capacity(unique.len());
    for id in unique {
        indices.insert(id, entries.len() as i64);
        let s: Vec<_> = text::units(id);
        let front = s.iter().zip(&prev).take_while(|(a, b)| a == b).count();
        let back = s[front..]
            .iter()
            .rev()
            .zip(prev[front..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        entries.push((front, text::from_units(&s[front..s.len() - back]), back));
        prev = s;
    }
    (entries, indices)
}

pub(crate) fn expand(entries: Vec<serde_json::Value>) -> Vec<Option<String>> {
    let mut previous = Some(Vec::new());
    entries
        .into_iter()
        .map(|row| {
            previous = (|| {
                let (front, middle, back): Entry = serde_json::from_value(row).ok()?;
                // A self-contained entry starts a new chain even after a damaged row.
                let prev = if front == 0 && back == 0 {
                    &[][..]
                } else {
                    previous.as_deref()?
                };
                if front > prev.len() || back > prev.len() - front {
                    return None;
                }
                let mut next = prev[..front].to_vec();
                next.extend(text::units(&middle));
                next.extend_from_slice(&prev[prev.len() - back..]);
                Some(next)
            })();
            previous.as_deref().map(text::from_units)
        })
        .collect()
}
