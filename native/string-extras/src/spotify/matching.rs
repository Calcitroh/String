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
    /// The whole credit, as `spelling`.
    credit: String,
    /// A credit that is one artist alone (no second name after `&`, `,`, `feat.` and the like).
    solo: bool,
    /// The album, as `spelling` ("" when unknown).
    album: String,
}

/// A Spotify song's keys. Only songs with a title that normalizes to something have them.
struct Play {
    /// `normalize_title`: featured artists and remaster notes gone.
    title: String,
    /// The whole title as written, as `spelling`, for song links.
    written: String,
    main: String,
    credit: String,
    solo: bool,
    album: String,
}

/// The library's songs indexed for matching, and its artist spellings.
pub struct Matcher {
    songs: Vec<Candidate>,
    /// (title, main artist) → songs, in library order.
    by_artist: HashMap<(String, String), Vec<usize>>,
    /// (title, whole credit) → songs, in library order.
    by_credit: HashMap<(String, String), Vec<usize>>,
    /// (whole title as written, whole credit) → songs, in library order: what a song link names.
    by_written: HashMap<(String, String), Vec<usize>>,
    /// Whole credit → the library's spelling.
    artists: HashMap<String, String>,
}

impl Matcher {
    /// `songs` are in library order and carry the names and id String plays them under. The first of duplicates wins.
    pub fn new<S: Into<LibrarySong>>(songs: impl IntoIterator<Item = S>) -> Matcher {
        let mut m = Matcher {
            songs: Vec::new(),
            by_artist: HashMap::new(),
            by_credit: HashMap::new(),
            by_written: HashMap::new(),
            artists: HashMap::new(),
        };
        for song in songs {
            let LibrarySong { track, album } = song.into();
            let credit = spelling(&track.2);
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
            m.by_written
                .entry((spelling(&track.1), credit.clone()))
                .or_default()
                .push(i);
            m.songs.push(Candidate {
                solo: normalize_text(&track.2) == main,
                album: spelling(&album),
                track,
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
        let main = main_artist(&t.2);
        Some(Play {
            written: spelling(&t.1),
            solo: normalize_text(&t.2) == main,
            album: spelling(history.album(&t.0).unwrap_or("")),
            credit: spelling(&t.2),
            main,
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

    /// A play's library song: the one a song link names; else, for an artist link, the same title by the library
    /// artist it names; else the same title by the same main artist (see `by_name`).
    fn find(&self, p: &Play, links: &Links) -> Option<&Track> {
        if let Some(to) = links.songs.get(&(p.written.clone(), p.credit.clone()))
            && let Some(song) = self
                .by_written
                .get(to)
                .and_then(|f| self.best(f, &to.1, &p.album))
        {
            return Some(song);
        }
        if let Some(credit) = links.artists.get(&p.credit)
            && let Some(song) = self
                .by_credit
                .get(&(p.title.clone(), credit.clone()))
                .and_then(|f| self.best(f, credit, &p.album))
        {
            return Some(song);
        }
        self.by_name(p)
    }

    /// The same title by the same main artist. A play credited to a collaboration takes its own credit or its lead
    /// artist alone, never another collaboration ("North & West" is not "North & South"). When the songs left are
    /// by more than one other credit, the album must choose, or nothing matches.
    fn by_name(&self, p: &Play) -> Option<&Track> {
        // A play with no artist name is never matched by artist.
        if p.main.is_empty() {
            return None;
        }
        let fits = |c: &Candidate| c.credit == p.credit || p.solo || c.solo;
        let found: Vec<usize> = self
            .by_artist
            .get(&(p.title.clone(), p.main.clone()))?
            .iter()
            .copied()
            .filter(|&i| fits(&self.songs[i]))
            .collect();
        let exact: Vec<usize> = found
            .iter()
            .copied()
            .filter(|&i| self.songs[i].credit == p.credit)
            .collect();
        if !exact.is_empty() {
            return self.best(&exact, &p.credit, &p.album);
        }
        let credits: HashSet<&str> = found
            .iter()
            .map(|&i| self.songs[i].credit.as_str())
            .collect();
        if credits.len() <= 1 {
            return self.best(&found, &p.credit, &p.album);
        }
        let on_album: Vec<usize> = found
            .iter()
            .copied()
            .filter(|&i| !p.album.is_empty() && self.songs[i].album == p.album)
            .collect();
        let credits: HashSet<&str> = on_album
            .iter()
            .map(|&i| self.songs[i].credit.as_str())
            .collect();
        (credits.len() == 1).then(|| &self.songs[on_album[0]].track)
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
            // An artist name that comes to nothing (an emoji) is never shown under a library artist.
            let credit = spelling(&t.2);
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

/// Links the person set by hand, as `spelling`. Only these join names that differ: automatic matching can't tell a
/// name written in another script from a label's or a compilation's credit, and a wrong join silently moves hours.
#[derive(Default)]
struct Links {
    /// (whole title as written, Spotify credit) → (whole title as written, library credit).
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
/// Both ends name the whole title and credit as written, so a link for a duet leaves the singer's solo songs alone,
/// and one for a "(feat. Ena)" version leaves the "(feat. Yui)" one alone. A link with other keys, or with an empty
/// name, is passed over.
fn links(history: &Listening) -> Links {
    let text = |v: &serde_json::Value, key: &str| -> Option<String> {
        let s = spelling(v.get(key)?.as_str()?);
        (!s.is_empty()).then_some(s)
    };
    let only = |v: &serde_json::Value, keys: &[&str]| {
        v.as_object()
            .is_some_and(|o| o.keys().all(|k| keys.contains(&k.as_str())))
    };
    let mut links = Links::default();
    let Some(list) = history.top_field(LINKS).and_then(|v| v.as_array()) else {
        return links;
    };
    for link in list {
        let Some(to) = link.get("as") else {
            continue;
        };
        let (Some(from_artist), Some(to_artist)) = (text(link, "artist"), text(to, "artist"))
        else {
            continue;
        };
        if only(link, &["artist", "as"]) && only(to, &["artist"]) {
            links.artists.insert(from_artist, to_artist);
        } else if only(link, &["title", "artist", "as"])
            && only(to, &["title", "artist"])
            && let (Some(from_title), Some(to_title)) = (text(link, "title"), text(to, "title"))
        {
            links
                .songs
                .insert((from_title, from_artist), (to_title, to_artist));
        }
    }
    links
}

/// A name as written, for comparing whole names: `normalize_text`, but the marks that join or split names stay as
/// words, so "River & Stone" is not "River Stone" and "Act I / II" is not "Act I + II".
fn spelling(s: &str) -> String {
    let mut words = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '&' => words.push_str(" and "),
            ',' => words.push_str(" comma "),
            '/' => words.push_str(" slash "),
            '+' => words.push_str(" plus "),
            ';' => words.push_str(" semicolon "),
            _ => words.push(c),
        }
    }
    normalize_text(&words)
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
    "acapella",
    "acoustic",
    "demo",
    "dub",
    "edit",
    "extended",
    "instrumental",
    "karaoke",
    "live",
    "mix",
    "orchestral",
    "remix",
    "reprise",
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
        // "(feat. Ena - Live)", "(with Ena, Remix)", "(feat. Ena – Reprise)": a version mark, or anything after a
        // dash or a slash, keeps the group: it's another recording.
        let version = inner
            .split(|c: char| !c.is_alphanumeric())
            .any(|w| VERSION_WORDS.contains(&w))
            || [" - ", " – ", " — ", " / "]
                .iter()
                .any(|d| inner.contains(d));
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
