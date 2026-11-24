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

/// A library song as matching sees it: the names and id String plays it under, and its album.
#[derive(Debug, Clone, PartialEq)]
pub struct LibrarySong {
    pub track: Track,
    pub album: String,
}
impl From<Track> for LibrarySong {
    fn from(track: Track) -> Self {
        LibrarySong {
            track,
            album: String::new(),
        }
    }
}

/// One library song with the keys matching compares.
struct Candidate {
    track: Track,
    /// `main_artist` of its credit.
    main: String,
    /// The whole credit, normalized.
    credit: String,
    /// The album, normalized ("" when unknown).
    album: String,
}

/// A Spotify song's keys. Only songs with a title that normalizes to something have them.
struct Play {
    title: String,
    main: String,
    credit: String,
    album: String,
}

/// The library's songs indexed for matching, and its artist spellings.
pub struct Matcher {
    songs: Vec<Candidate>,
    /// (title, main artist) → songs, in library order.
    by_artist: HashMap<(String, String), Vec<usize>>,
    /// (title, whole credit) → songs, in library order.
    by_credit: HashMap<(String, String), Vec<usize>>,
    /// Normalized credit → the library's spelling.
    artists: HashMap<String, String>,
}

impl Matcher {
    /// `songs` are in library order and carry the names and id String plays them under. The first of duplicates wins.
    pub fn new<S: Into<LibrarySong>>(songs: impl IntoIterator<Item = S>) -> Matcher {
        let mut m = Matcher {
            songs: Vec::new(),
            by_artist: HashMap::new(),
            by_credit: HashMap::new(),
            artists: HashMap::new(),
        };
        for song in songs {
            let LibrarySong { track, album } = song.into();
            let credit = normalize_text(&track.2);
            if !credit.is_empty() {
                m.artists
                    .entry(credit.clone())
                    .or_insert_with(|| track.2.clone());
            }
            let title = normalize_title(&track.1);
            if title.is_empty() {
                continue;
            }
            let main = main_artist(&track.2);
            let i = m.songs.len();
            m.by_artist
                .entry((title.clone(), main.clone()))
                .or_default()
                .push(i);
            m.by_credit
                .entry((title, credit.clone()))
                .or_default()
                .push(i);
            m.songs.push(Candidate {
                album: normalize_text(&album),
                track,
                main,
                credit,
            });
        }
        m
    }

    fn play(history: &Listening, t: &Track) -> Option<Play> {
        let title = normalize_title(&t.1);
        // A title that is only punctuation or a credit would match any such song by the artist.
        if title.is_empty() {
            return None;
        }
        Some(Play {
            album: normalize_text(history.album(&t.0).unwrap_or("")),
            main: main_artist(&t.2),
            credit: normalize_text(&t.2),
            title,
        })
    }

    /// Of `found`, the song with this whole credit on this album, else with this credit, else on this album, else
    /// the first (copies of one song: library order decides).
    fn best(&self, found: &[usize], credit: &str, album: &str) -> Option<&Track> {
        let on_album = |c: &Candidate| !album.is_empty() && c.album == album;
        let pick =
            |test: &dyn Fn(&Candidate) -> bool| found.iter().find(|&&i| test(&self.songs[i]));
        pick(&|c| c.credit == credit && on_album(c))
            .or_else(|| pick(&|c| c.credit == credit))
            .or_else(|| pick(&on_album))
            .or(found.first())
            .map(|&i| &self.songs[i].track)
    }

    /// Songs with `title` by exactly the library credit `credit`: a link to "North" never lands on "North & South".
    fn by_named_credit(&self, title: &str, credit: &str) -> Option<&Vec<usize>> {
        self.by_credit.get(&(title.to_string(), credit.to_string()))
    }

    /// A play's library song: the one a song link names; else, for an artist link, the same title by the library
    /// artist it names; else the same title by the same main artist, unless the play is a collaboration and every
    /// such song is another one.
    fn find(&self, p: &Play, links: &Links) -> Option<&Track> {
        if let Some((title, credit)) = links.songs.get(&(p.title.clone(), p.credit.clone())) {
            let found = self.by_named_credit(title, credit);
            if let Some(song) = found.and_then(|f| self.best(f, credit, "")) {
                return Some(song);
            }
        }
        if let Some(credit) = links.artists.get(&p.credit) {
            let found = self.by_named_credit(&p.title, credit);
            return found.and_then(|f| self.best(f, credit, &p.album));
        }
        // A play with no artist name is never matched by artist.
        if p.main.is_empty() {
            return None;
        }
        let found = self.by_artist.get(&(p.title.clone(), p.main.clone()))?;
        // "North & West" is not "North & South": a collaboration matches its own credit or its lead artist alone.
        let other_duet = |&i: &usize| {
            let c = &self.songs[i];
            c.credit != p.credit && c.credit != c.main
        };
        if p.credit != p.main && found.iter().all(other_duet) {
            return None;
        }
        self.best(found, &p.credit, &p.album)
    }

    pub fn resolve(&self, history: &Listening) -> (Listening, Resolved) {
        let links = links(history);
        let mut matched = HashSet::new();
        let out = history.remap(|t| {
            let p = Self::play(history, t);
            if let Some(song) = p.as_ref().and_then(|p| self.find(p, &links)) {
                matched.insert(song.0.clone());
                return song.clone();
            }
            // An artist name that normalizes to nothing (an emoji) is never shown under a library artist.
            let credit = normalize_text(&t.2);
            let artist = links
                .artists
                .get(&credit)
                .and_then(|to| self.artists.get(to))
                .or_else(|| {
                    (!credit.is_empty())
                        .then(|| self.artists.get(&credit))
                        .flatten()
                })
                .unwrap_or(&t.2);
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

/// The history's own top-level key for links set by hand.
const LINKS: &str = "sameAs";

/// Links the person set by hand, normalized. Only these join names that differ: automatic matching can't tell a name
/// written in another script from a label's or a compilation's credit, and a wrong join silently moves hours.
#[derive(Default)]
struct Links {
    /// (title, Spotify credit) → (title, library credit).
    songs: HashMap<(String, String), (String, String)>,
    /// Spotify credit → library credit.
    artists: HashMap<String, String>,
}

/// `"sameAs"` in the history's top-level object, kept by imports like any top-level key:
/// - `{ "title", "artist", "as": { "title", "artist" } }` counts one song as a library song (a re-upload, or a cover
///   the person counts as the original);
/// - `{ "artist", "as": { "artist" } }` counts every song credited so as the library artist's (a name written in
///   another script).
///
/// Both ends name the whole credit, so a link for a duet leaves the singer's solo songs alone. A link that isn't
/// whole is passed over.
fn links(history: &Listening) -> Links {
    let credit = |v: &serde_json::Value| -> Option<String> {
        let credit = normalize_text(v.get("artist")?.as_str()?);
        (!credit.is_empty()).then_some(credit)
    };
    let title = |v: &serde_json::Value| -> Option<String> {
        let title = normalize_title(v.get("title")?.as_str()?);
        (!title.is_empty()).then_some(title)
    };
    let mut links = Links::default();
    let Some(list) = history.top_field(LINKS).and_then(|v| v.as_array()) else {
        return links;
    };
    for link in list {
        let (Some(from), Some(to)) = (credit(link), link.get("as").and_then(credit)) else {
            continue;
        };
        match (
            link.get("title"),
            link.get("as").and_then(|to| to.get("title")),
        ) {
            (None, None) => {
                links.artists.insert(from, to);
            }
            (Some(_), Some(_)) => {
                if let (Some(a), Some(b)) = (title(link), link.get("as").and_then(title)) {
                    links.songs.insert((a, from), (b, to));
                }
            }
            _ => {}
        }
    }
    links
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
            // Japanese voicing marks change a kana (カ, ガ): kept, unlike accents, which fold.
            '\u{3099}' | '\u{309a}' => {
                out.push(c);
                space = false;
                continue;
            }
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

/// Words that make a recording another one: a credit group holding one stays in the title.
const VERSION_WORDS: &[&str] = &[
    "acoustic",
    "demo",
    "edit",
    "instrumental",
    "live",
    "mix",
    "remix",
    "version",
];

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
        // "(feat. Ena - Live)", "(with Ena, Remix)": a version mark keeps the group, it's another recording.
        let version = inner
            .split(|c: char| !c.is_alphanumeric())
            .any(|w| VERSION_WORDS.contains(&w));
        if (credit && !version) || is_remaster_note(inner) {
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
