//! Matching a Spotify history to the library's songs and artists. Nothing here is stored: Stats resolves again
//! whenever the library changes, so a song added later picks up its old Spotify plays.

use std::collections::{HashMap, HashSet};
use string_core::stats::{Listening, Track};

/// How much of a history is the person's own songs, in seconds (the layer stores seconds, not play counts).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Resolved {
    pub matched_seconds: f64,
    pub seconds: f64,
}

/// The library's songs by (normalized title, main artist), and its artist spellings.
pub struct Matcher {
    songs: HashMap<(String, String), Track>,
    artists: HashMap<String, String>,
}

impl Matcher {
    /// `songs` are in library order and carry the names and id String plays them under. The first of duplicates wins.
    pub fn new(songs: impl IntoIterator<Item = Track>) -> Matcher {
        let mut by_key = HashMap::new();
        let mut artists = HashMap::new();
        for song in songs {
            artists
                .entry(normalize_text(&song.2))
                .or_insert_with(|| song.2.clone());
            let title = normalize_title(&song.1);
            if !title.is_empty() {
                by_key.entry((title, main_artist(&song.2))).or_insert(song);
            }
        }
        Matcher {
            songs: by_key,
            artists,
        }
    }

    pub fn resolve(&self, history: &Listening) -> (Listening, Resolved) {
        let mut matched = HashSet::new();
        let out = history.remap(|t| {
            let title = normalize_title(&t.1);
            // A title that is only punctuation or a credit would match any such song by the artist.
            let found = (!title.is_empty())
                .then(|| self.songs.get(&(title, main_artist(&t.2))))
                .flatten();
            if let Some(song) = found {
                matched.insert(song.0.clone());
                return song.clone();
            }
            let artist = self.artists.get(&normalize_text(&t.2)).unwrap_or(&t.2);
            Track(t.0.clone(), t.1.clone(), artist.clone())
        });
        let ids: Vec<bool> = out.tracks().map(|t| matched.contains(&t.0)).collect();
        let mut resolved = Resolved {
            matched_seconds: 0.,
            seconds: 0.,
        };
        for day in out.days().values() {
            for (&i, &secs) in &day.t {
                resolved.seconds += secs;
                if ids.get(i) == Some(&true) {
                    resolved.matched_seconds += secs;
                }
            }
        }
        (out, resolved)
    }
}

/// Lower case, common accents folded, punctuation and repeated spaces collapsed.
fn normalize_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = false;
    for c in s.chars().flat_map(char::to_lowercase) {
        let folded = match c {
            'à'..='å' => "a",
            'ç' => "c",
            'è'..='ë' => "e",
            'ì'..='ï' => "i",
            'ñ' => "n",
            'ò'..='ö' | 'ø' => "o",
            'ù'..='ü' => "u",
            'ý' | 'ÿ' => "y",
            'ß' => "ss",
            '\'' | '’' => continue,
            _ => "",
        };
        if !folded.is_empty() {
            out.push_str(folded);
            space = false;
        } else if c.is_alphanumeric() {
            out.push(c);
            space = false;
        } else if !space && !out.is_empty() {
            out.push(' ');
            space = true;
        }
    }
    out.truncate(out.trim_end().len());
    out
}

/// A title without its featured artists and remaster notes. Live, remix and edit marks stay: other recordings.
pub fn normalize_title(s: &str) -> String {
    let mut text = s.to_lowercase();
    while let Some(cut) = removable_group(&text) {
        text.replace_range(cut.0..cut.1, " ");
    }
    if let Some(dash) = text.rfind(" - ")
        && is_remaster_note(&text[dash + 3..])
    {
        text.truncate(dash);
    }
    normalize_text(&text)
}

/// "2011 Remaster", "Remastered Version", "The Digital Remaster 2009": only these words, so "Live, Remastered" stays.
fn is_remaster_note(note: &str) -> bool {
    let mut words = note
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty());
    let mut remaster = false;
    let ok = words.all(|w| match w {
        "remaster" | "remastered" => {
            remaster = true;
            true
        }
        "version" | "edition" | "digital" | "the" => true,
        _ => w.len() == 4 && w.bytes().all(|b| b.is_ascii_digit()),
    });
    ok && remaster
}

/// The byte range of the first bracketed group that is a credit or a remaster note.
fn removable_group(text: &str) -> Option<(usize, usize)> {
    for (start, open) in text.char_indices().filter(|(_, c)| matches!(c, '(' | '[')) {
        let close = if open == '(' { ')' } else { ']' };
        let Some(len) = text[start..].find(close) else {
            continue;
        };
        let inner = text[start + 1..start + len].trim();
        let credit = ["feat.", "feat ", "ft.", "ft ", "featuring ", "with "]
            .iter()
            .any(|p| inner.starts_with(p));
        if credit || is_remaster_note(inner) {
            return Some((start, start + len + 1));
        }
    }
    None
}

/// The first credited artist, normalized: the part before `, `, ` & `, ` feat. `, ` ft. ` or ` x `.
pub fn main_artist(s: &str) -> String {
    let lower = s.to_lowercase();
    let end = [", ", " & ", " feat. ", " ft. ", " x "]
        .iter()
        .filter_map(|sep| lower.find(sep))
        .min()
        .unwrap_or(lower.len());
    normalize_text(&lower[..end])
}
