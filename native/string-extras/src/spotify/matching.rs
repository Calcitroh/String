//! Matching a Spotify history to the library's songs and artists. Nothing here is stored: Stats resolves again
//! whenever the library changes, so a song added later picks up its old Spotify plays.
//!
//! A wrong match silently moves hours to the wrong song or artist, so matching is conservative: names must agree,
//! anything ambiguous stays a Spotify song, and names that differ join only through links the person sets by hand.

use std::cmp::Reverse;
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

/// A credit's or a title's keys, the same for a library song and a Spotify play.
struct Keys {
    /// `normalize_title`: featured artists and remaster notes gone.
    title: String,
    /// The title as written, remaster notes gone, as `spelling` ("home feat cora", not "home feat ben").
    written: String,
    /// The guest a "(feat. …)" or "(with …)" names, normalized; empty when there is none.
    guest: String,
    /// `main_artist` of the credit.
    main: String,
    /// The whole credit, as `spelling`.
    credit: String,
    /// The first of two or more names the credit joins ("north" for "North, West"); empty for one artist alone.
    lead: String,
    /// A credit that is one artist alone, with nothing that joins or marks names but a hyphen ("Jay-Z").
    solo: bool,
    /// The album, as `spelling` ("" when unknown).
    album: String,
}
impl Keys {
    fn of(title: &str, artist: &str, album: &str) -> Keys {
        let main = main_artist(artist);
        let lead = lead_artist(artist);
        let marked = artist.contains(|c: char| MARKS.contains(&c) && c != '-');
        Keys {
            title: normalize_title(title),
            written: spelling(&without_remaster(title)),
            guest: guest(title),
            solo: !marked && lead.is_empty() && normalize_text(artist) == main,
            credit: spelling(artist),
            album: spelling(album),
            main,
            lead,
        }
    }
    /// Two versions naming different guests are different recordings ("(feat. Ena)", "(feat. Yui)").
    fn guests_differ(&self, other: &Keys) -> bool {
        !self.guest.is_empty() && !other.guest.is_empty() && self.guest != other.guest
    }
}

/// One library song with the keys matching compares.
struct Candidate {
    track: Track,
    keys: Keys,
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
            let keys = Keys::of(&track.1, &track.2, &album);
            if !keys.credit.is_empty() {
                m.artists
                    .entry(keys.credit.clone())
                    .or_insert_with(|| track.2.clone());
            }
            if keys.title.is_empty() {
                continue;
            }
            let i = m.songs.len();
            m.by_artist
                .entry((keys.title.clone(), keys.main.clone()))
                .or_default()
                .push(i);
            m.by_credit
                .entry((keys.title.clone(), keys.credit.clone()))
                .or_default()
                .push(i);
            m.by_written
                .entry((spelling(&track.1), keys.credit.clone()))
                .or_default()
                .push(i);
            m.songs.push(Candidate { track, keys });
        }
        m
    }

    /// A Spotify song's keys, when its title normalizes to something: a title that is only punctuation or a credit
    /// would match any such song by the artist.
    fn play(history: &Listening, t: &Track) -> Option<Keys> {
        let keys = Keys::of(&t.1, &t.2, history.album(&t.0).unwrap_or(""));
        (!keys.title.is_empty()).then_some(keys)
    }

    /// Of `found`, the song that agrees most with `p`: the same whole credit first, then the same album (the live
    /// album's copy for a live play), then the same title as written ("(feat. Cora)" over "(feat. Ben)"); library
    /// order between copies that agree as much. Songs naming another guest than the play are never chosen.
    fn best(&self, found: &[usize], credit: &str, p: &Keys) -> Option<&Track> {
        let score = |c: &Keys| {
            4 * u8::from(c.credit == credit)
                + 2 * u8::from(!p.album.is_empty() && c.album == p.album)
                + u8::from(c.written == p.written)
        };
        found
            .iter()
            .enumerate()
            .filter(|&(_, &i)| !self.songs[i].keys.guests_differ(p))
            .max_by_key(|&(n, &i)| (score(&self.songs[i].keys), Reverse(n)))
            .map(|(_, &i)| &self.songs[i].track)
    }

    /// A play's library song: the one a song link names; else, for an artist link, the same title by the library
    /// artist it names; else the same title by the same artist (see `by_name`).
    fn find(&self, p: &Keys, played: &str, links: &Links) -> Option<&Track> {
        if let Some(to) = links.songs.get(&(spelling(played), p.credit.clone()))
            && let Some(found) = self.by_written.get(to)
        {
            // A song link is the person's own decision: the song it names, on the play's album if it has copies.
            let named = Keys {
                title: String::new(),
                written: to.0.clone(),
                guest: String::new(),
                main: String::new(),
                credit: to.1.clone(),
                lead: String::new(),
                solo: false,
                album: p.album.clone(),
            };
            if let Some(song) = self.best(found, &to.1, &named) {
                return Some(song);
            }
        }
        // An artist link to a name the library has decides: the play is theirs or no one's. One to a name the
        // library lacks is passed over.
        if let Some(credit) = links.artists.get(&p.credit)
            && self.artists.contains_key(credit)
        {
            return self
                .by_credit
                .get(&(p.title.clone(), credit.clone()))
                .and_then(|f| self.best(f, credit, p));
        }
        self.by_name(p)
    }

    /// The same title by the same artist: the whole credit first, then the main artist. A play credited to two or
    /// more names takes its own credit or its lead artist alone, never another credit ("North & West" is not
    /// "North & South", "North+" is not "North"). When the songs left are by more than one credit, the album must
    /// choose, or nothing matches.
    fn by_name(&self, p: &Keys) -> Option<&Track> {
        // A play with no artist name is never matched by artist.
        if p.credit.is_empty() {
            return None;
        }
        if let Some(found) = self.by_credit.get(&(p.title.clone(), p.credit.clone())) {
            return self.best(found, &p.credit, p);
        }
        if p.main.is_empty() {
            return None;
        }
        let fits = |c: &Keys| {
            p.solo || (c.solo && !p.lead.is_empty() && normalize_text(&c.credit) == p.lead)
        };
        let found: Vec<usize> = self
            .by_artist
            .get(&(p.title.clone(), p.main.clone()))?
            .iter()
            .copied()
            .filter(|&i| fits(&self.songs[i].keys) && !self.songs[i].keys.guests_differ(p))
            .collect();
        let credits = |set: &[usize]| -> HashSet<&str> {
            set.iter()
                .map(|&i| self.songs[i].keys.credit.as_str())
                .collect()
        };
        if credits(&found).len() <= 1 {
            return self.best(&found, &p.credit, p);
        }
        let on_album: Vec<usize> = found
            .iter()
            .copied()
            .filter(|&i| !p.album.is_empty() && self.songs[i].keys.album == p.album)
            .collect();
        if credits(&on_album).len() == 1 {
            return self.best(&on_album, &p.credit, p);
        }
        None
    }

    pub fn resolve(&self, history: &Listening) -> (Listening, Resolved) {
        let links = links(history);
        let mut matched = HashSet::new();
        let out = history.remap(|t| {
            let p = Self::play(history, t);
            if let Some(song) = p.as_ref().and_then(|p| self.find(p, &t.1, &links)) {
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

/// Marks that tell names apart when whole names are compared: they join or split credits ("River & Stone" is not
/// "River Stone", "North+" is not "North Plus", "North-X-West" is not "North x West") or change a note ("C#" is not
/// "C").
const MARKS: &[char] = &['&', ',', '/', '+', ';', '-', '#', '\u{266d}', '\u{266f}'];
/// Marks a title keeps even where featured artists and punctuation fold away: "Prelude in C#" is not "Prelude in C".
const NOTE_MARKS: &[char] = &['#', '\u{266d}', '\u{266f}'];

/// A name as written, for comparing whole names: `normalize_text`, but each of `MARKS` stays, as a word of its own
/// whatever the spacing around it ("River&Stone" is "River & Stone").
fn spelling(s: &str) -> String {
    fold(s, MARKS)
}

/// Lower case, common accents folded, punctuation and repeated spaces collapsed.
fn normalize_text(s: &str) -> String {
    fold(s, &[])
}

/// `normalize_text`, keeping each of `marks` as a word of its own. A sharp sign is "#".
fn fold(s: &str, marks: &[char]) -> String {
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
        if marks.contains(&c) {
            if !space && !out.is_empty() {
                out.push(' ');
            }
            out.push(if c == '\u{266f}' { '#' } else { c });
            out.push(' ');
            space = true;
        } else if !folded.is_empty() {
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

/// A title without its featured artists and remaster notes. Live, remix and edit marks stay: other recordings. So do
/// sharps and flats.
pub fn normalize_title(s: &str) -> String {
    let mut text = s.to_lowercase();
    while let Some(cut) = removable_group(&text, true) {
        text.replace_range(cut.0..cut.1, " ");
    }
    fold(&strip_remaster_suffix(&text), NOTE_MARKS)
}

/// A title as written without its remaster notes ("Home (feat. Cora) - 2011 Remaster" is "Home (feat. Cora)").
fn without_remaster(s: &str) -> String {
    let mut text = s.to_lowercase();
    while let Some(cut) = removable_group(&text, false) {
        text.replace_range(cut.0..cut.1, " ");
    }
    strip_remaster_suffix(&text).to_string()
}

fn strip_remaster_suffix(text: &str) -> &str {
    match text.rfind(" - ") {
        Some(dash) if is_remaster_note(&text[dash + 3..]) => &text[..dash],
        _ => text,
    }
}

/// The guest a "(feat. …)" or "(with …)" group names, normalized; "" when there is none.
fn guest(title: &str) -> String {
    let text = title.to_lowercase();
    for (start, open) in text.char_indices().filter(|(_, c)| matches!(c, '(' | '[')) {
        let close = if open == '(' { ')' } else { ']' };
        let Some(len) = text[start..].find(close) else {
            continue;
        };
        let inner = text[start + 1..start + len].trim();
        if let Some(name) = CREDITS.iter().find_map(|p| inner.strip_prefix(p)) {
            return normalize_text(name);
        }
    }
    String::new()
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

/// How a featured-artist group begins.
const CREDITS: &[&str] = &["feat.", "feat ", "ft.", "ft ", "featuring ", "with "];

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
    "unplugged",
    "version",
];

/// The byte range of the first bracketed group that is a remaster note, or (with `credits`) a featured artist.
fn removable_group(text: &str, credits: bool) -> Option<(usize, usize)> {
    for (start, open) in text.char_indices().filter(|(_, c)| matches!(c, '(' | '[')) {
        let close = if open == '(' { ')' } else { ']' };
        let Some(len) = text[start..].find(close) else {
            continue;
        };
        let inner = text[start + 1..start + len].trim();
        let credit = credits && CREDITS.iter().any(|p| inner.starts_with(p));
        // "(feat. Ena - Live)", "(with Ena, Remix)", "(feat. Ena—Reprise)": a version mark, or anything after a long
        // dash or a slash, keeps the group: it's another recording.
        let version = inner
            .split(|c: char| !c.is_alphanumeric())
            .any(|w| VERSION_WORDS.contains(&w))
            || inner.contains(['\u{2013}', '\u{2014}', '/'])
            || inner.contains(" - ");
        if (credit && !version) || is_remaster_note(inner) {
            return Some((start, start + len + 1));
        }
    }
    None
}

/// The first credited artist, normalized: the part before `, `, ` & `, ` feat. `, ` ft. ` or ` x `. A name with
/// such a mark inside it ("Glass/House", "North+") stays whole.
pub fn main_artist(s: &str) -> String {
    let lower = s.to_lowercase();
    let end = [", ", " & ", " feat. ", " ft. ", " x "]
        .iter()
        .filter_map(|sep| lower.find(sep))
        .min()
        .unwrap_or(lower.len());
    normalize_text(&lower[..end])
}

/// The first of two or more names a credit joins, normalized: "north" for "North, West", "North & West", "North
/// feat. West" or "North/West"; "" for one artist alone, and for a mark with nothing on one side ("North+", "&Nova").
fn lead_artist(s: &str) -> String {
    let lower = s.to_lowercase();
    let cut = [
        ",", "&", "/", ";", "+", " feat. ", " feat ", " ft. ", " x ", " with ",
    ]
    .iter()
    .filter_map(|sep| lower.find(sep).map(|at| (at, sep.len())))
    .min();
    match cut {
        Some((at, len)) => {
            let (first, rest) = (
                normalize_text(&lower[..at]),
                normalize_text(&lower[at + len..]),
            );
            if first.is_empty() || rest.is_empty() {
                String::new()
            } else {
                first
            }
        }
        None => String::new(),
    }
}
