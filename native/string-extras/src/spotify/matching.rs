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

/// How many different songs on one album must prove that two spellings name one artist.
const SAME_ARTIST_PROOF: usize = 2;

/// Album names many artists share: never evidence that two credits are one recording.
const GENERIC_ALBUMS: &[&str] = &[
    "album",
    "anthology",
    "b sides",
    "best",
    "collection",
    "compilation",
    "covers",
    "deluxe",
    "deluxe edition",
    "demo",
    "demos",
    "ep",
    "essentials",
    "favorites",
    "favourites",
    "greatest hits",
    "hits",
    "instrumentals",
    "live",
    "music",
    "new album",
    "ost",
    "original soundtrack",
    "rarities",
    "remixes",
    "single",
    "singles",
    "songs",
    "soundtrack",
    "the best",
    "the collection",
    "the essentials",
    "the hits",
    "the soundtrack",
    "tracks",
    "unknown",
    "unknown album",
    "untitled",
    "various",
    "various artists",
];

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
    /// The album as evidence: `album_key`, so never generic, self-titled or a single named after the song.
    album: Option<String>,
}

/// The library's songs indexed for matching, and its artist spellings.
pub struct Matcher {
    songs: Vec<Candidate>,
    /// (title, main artist) → songs, in library order.
    by_artist: HashMap<(String, String), Vec<usize>>,
    /// (title, whole credit) → songs, in library order.
    by_credit: HashMap<(String, String), Vec<usize>>,
    /// (title, album) → songs, for albums that can tell recordings apart.
    by_album: HashMap<(String, String), Vec<usize>>,
    /// Normalized credit → the library's spelling.
    artists: HashMap<String, String>,
    /// Main artist → the library's spelling: a credit that is only that artist if there is one, else the first.
    mains: HashMap<String, String>,
}

/// What a history proves about its spellings, worked out once per `resolve`.
struct Evidence {
    /// Hand-set links: (title, Spotify credit) → (title, library credit).
    links: Links,
    /// (Spotify main artist, album) → the one library main artist that album proves it is.
    albums: HashMap<(String, String), String>,
    /// Spotify main artist → the library credit (normalized, whole) it is everywhere.
    aliases: HashMap<String, String>,
}

impl Matcher {
    /// `songs` are in library order and carry the names and id String plays them under. The first of duplicates wins.
    pub fn new<S: Into<LibrarySong>>(songs: impl IntoIterator<Item = S>) -> Matcher {
        let mut m = Matcher {
            songs: Vec::new(),
            by_artist: HashMap::new(),
            by_credit: HashMap::new(),
            by_album: HashMap::new(),
            artists: HashMap::new(),
            mains: HashMap::new(),
        };
        for song in songs {
            let LibrarySong { track, album } = song.into();
            let credit = normalize_text(&track.2);
            let main = main_artist(&track.2);
            if !credit.is_empty() {
                m.artists
                    .entry(credit.clone())
                    .or_insert_with(|| track.2.clone());
            }
            if !main.is_empty() {
                let spelling = m
                    .mains
                    .entry(main.clone())
                    .or_insert_with(|| track.2.clone());
                // A credit that is only this artist names them best.
                if credit == main && normalize_text(spelling) != main {
                    track.2.clone_into(spelling);
                }
            }
            let title = normalize_title(&track.1);
            if title.is_empty() {
                continue;
            }
            let i = m.songs.len();
            m.by_artist
                .entry((title.clone(), main.clone()))
                .or_default()
                .push(i);
            m.by_credit
                .entry((title.clone(), credit.clone()))
                .or_default()
                .push(i);
            if let Some(key) = album_key(&title, &album, &track.2) {
                m.by_album.entry((title, key)).or_default().push(i);
            }
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
        let album = history.album(&t.0).unwrap_or("");
        Some(Play {
            album: album_key(&title, album, &t.2),
            main: main_artist(&t.2),
            credit: normalize_text(&t.2),
            title,
        })
    }

    /// Of `found`, the song with this credit on this album, else with this credit, else on this album, else the
    /// first (copies of one song: library order decides).
    fn best(&self, found: &[usize], credit: &str, album: Option<&str>) -> Option<&Track> {
        let on_album = |c: &Candidate| album.is_some_and(|a| a == c.album);
        let pick =
            |test: &dyn Fn(&Candidate) -> bool| found.iter().find(|&&i| test(&self.songs[i]));
        pick(&|c| c.credit == credit && on_album(c))
            .or_else(|| pick(&|c| c.credit == credit))
            .or_else(|| pick(&on_album))
            .or(found.first())
            .map(|&i| &self.songs[i].track)
    }

    /// A play's library song: the one a hand-set link names; else the same title by the same main artist; else the
    /// same title on an album that proves this spelling is a library artist; else the same title by the library
    /// artist this spelling is everywhere.
    fn find(&self, p: &Play, ev: &Evidence) -> Option<&Track> {
        if let Some((title, credit)) = ev.links.get(&(p.title.clone(), p.credit.clone())) {
            let found = self
                .by_credit
                .get(&(title.clone(), credit.clone()))
                .or_else(|| self.by_artist.get(&(title.clone(), main_artist(credit))));
            if let Some(song) = found.and_then(|f| self.best(f, credit, None)) {
                return Some(song);
            }
        }
        // A play with no artist name is never matched by artist.
        if p.main.is_empty() {
            return None;
        }
        if let Some(found) = self.by_artist.get(&(p.title.clone(), p.main.clone())) {
            return self.best(found, &p.credit, p.album.as_deref());
        }
        if let Some(album) = &p.album
            && let Some(library) = ev.albums.get(&(p.main.clone(), album.clone()))
            && let Some(found) = self.by_album.get(&(p.title.clone(), album.clone()))
            && let Some(&i) = found.iter().find(|&&i| self.songs[i].main == *library)
        {
            return Some(&self.songs[i].track);
        }
        let credit = ev.aliases.get(&p.main)?;
        let found = self.by_credit.get(&(p.title.clone(), credit.clone()))?;
        self.best(found, credit, p.album.as_deref())
    }

    /// What `history` proves, only ever about two names written in different scripts (a name in Japanese, the
    /// library's in Latin letters, or the other way round). A Spotify spelling is a library artist on an album when
    /// at least `SAME_ARTIST_PROOF` of its songs there have titles the library has on that album by that artist, and
    /// by no other artist. It is that artist everywhere when the songs it shares with the library, on any albums
    /// that can tell recordings apart, are at least `SAME_ARTIST_PROOF` different titles, all by that one artist,
    /// and the spelling isn't a library artist's own name. Plays a hand-set link covers are left out: the person
    /// decided those.
    fn evidence(&self, history: &Listening) -> Evidence {
        let links = links(history);
        let mut groups: HashMap<(String, String), HashSet<String>> = HashMap::new();
        for t in history.tracks() {
            let Some(p) = Self::play(history, t) else {
                continue;
            };
            if p.main.is_empty() || links.contains_key(&(p.title.clone(), p.credit.clone())) {
                continue;
            }
            if let Some(album) = p.album {
                groups.entry((p.main, album)).or_default().insert(p.title);
            }
        }
        let mut albums = HashMap::new();
        // Spelling → every library artist it shares a title with on some album → (those titles, their credits).
        type Shared = HashMap<String, (HashSet<String>, HashSet<String>)>;
        let mut heard: HashMap<String, Shared> = HashMap::new();
        for ((spelling, album), titles) in groups {
            // Library main artist → (titles shared on this album, their whole credits).
            let mut shared: HashMap<&str, (HashSet<&str>, HashSet<&str>)> = HashMap::new();
            for title in &titles {
                let found = self.by_album.get(&(title.clone(), album.clone()));
                for &i in found.into_iter().flatten() {
                    let c = &self.songs[i];
                    if !c.main.is_empty() && c.main != spelling {
                        let entry = shared.entry(&c.main).or_default();
                        entry.0.insert(title);
                        entry.1.insert(&c.credit);
                    }
                }
            }
            let all = heard.entry(spelling.clone()).or_default();
            for (m, (t, c)) in &shared {
                let entry = all.entry(m.to_string()).or_default();
                entry.0.extend(t.iter().map(|t| t.to_string()));
                entry.1.extend(c.iter().map(|c| c.to_string()));
            }
            let proven: Vec<&str> = shared
                .iter()
                .filter(|&(m, (t, _))| t.len() >= SAME_ARTIST_PROOF && other_script(&spelling, m))
                .map(|(m, _)| *m)
                .collect();
            if let [library] = proven[..] {
                albums.insert((spelling, album), library.to_string());
            }
        }
        let mut aliases = HashMap::new();
        for (spelling, shared) in &heard {
            // One library artist across every album, in another script, with enough different songs.
            let [(library, (titles, credits))] = shared.iter().collect::<Vec<_>>()[..] else {
                continue;
            };
            let own_name = self.mains.contains_key(spelling) || self.artists.contains_key(spelling);
            if titles.len() < SAME_ARTIST_PROOF || !other_script(spelling, library) || own_name {
                continue;
            }
            // The credit it is: the only one that proved it, else the library artist's own name among them
            // ("Rin Aoba" over "Rin Aoba feat. Mia"), never a duet's first name alone.
            let credit = match credits.len() {
                1 => credits.iter().next().cloned(),
                _ => credits.contains(library).then(|| library.clone()),
            };
            if let Some(credit) = credit {
                aliases.insert(spelling.clone(), credit);
            }
        }
        Evidence {
            links,
            albums,
            aliases,
        }
    }

    pub fn resolve(&self, history: &Listening) -> (Listening, Resolved) {
        let ev = self.evidence(history);
        let mut matched = HashSet::new();
        let out = history.remap(|t| {
            let p = Self::play(history, t);
            if let Some(song) = p.as_ref().and_then(|p| self.find(p, &ev)) {
                matched.insert(song.0.clone());
                return song.clone();
            }
            // An artist name that normalizes to nothing (an emoji) is never shown under a library artist.
            let credit = normalize_text(&t.2);
            let artist = (!credit.is_empty())
                .then(|| self.artists.get(&credit))
                .flatten()
                .or_else(|| self.artists.get(ev.aliases.get(&main_artist(&t.2))?))
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
/// (title, whole credit) of a Spotify song → (title, whole credit) of the library song it counts as, normalized.
type Links = HashMap<(String, String), (String, String)>;

/// Links set by hand in the history: `"sameAs": [{ "title", "artist", "as": { "title", "artist" } }]`, for a
/// recording that is the same song by another name (a re-upload, a cover the person counts as the original). Kept by
/// imports like any top-level key; a link that isn't whole is passed over. Both ends name the whole credit, so a link
/// for a duet leaves the singer's solo song alone.
fn links(history: &Listening) -> Links {
    let key = |v: &serde_json::Value| -> Option<(String, String)> {
        let title = normalize_title(v.get("title")?.as_str()?);
        let credit = normalize_text(v.get("artist")?.as_str()?);
        (!title.is_empty() && !credit.is_empty()).then_some((title, credit))
    };
    let Some(list) = history.top_field(LINKS).and_then(|v| v.as_array()) else {
        return Links::new();
    };
    list.iter()
        .filter_map(|link| Some((key(link)?, key(link.get("as")?)?)))
        .collect()
}

/// An album that can tell recordings apart: named, not a name many artists share ("Greatest Hits", "Soundtrack"),
/// not named after the artist, and not a single named after its song ("Home - 2011 Remaster" on "Home").
fn album_key(title: &str, album: &str, artist: &str) -> Option<String> {
    let key = normalize_text(album);
    let generic = GENERIC_ALBUMS.contains(&key.as_str())
        || [
            "best of ",
            "the best of ",
            "greatest hits ",
            "the very best ",
        ]
        .iter()
        .any(|p| key.starts_with(p));
    let self_titled = key == normalize_text(artist) || key == main_artist(artist);
    let single = key == title || normalize_title(album) == title;
    (!key.is_empty() && !generic && !self_titled && !single).then_some(key)
}

/// Which letters a name is written in.
#[derive(Clone, Copy, PartialEq)]
enum Script {
    Latin,
    Other,
    Mixed,
    NoLetters,
}
fn script(name: &str) -> Script {
    let (mut latin, mut other) = (false, false);
    for c in name.chars().filter(|c| c.is_alphabetic()) {
        if c.is_ascii_alphabetic() || ('\u{c0}'..='\u{24f}').contains(&c) {
            latin = true;
        } else {
            other = true;
        }
    }
    match (latin, other) {
        (true, false) => Script::Latin,
        (false, true) => Script::Other,
        (true, true) => Script::Mixed,
        (false, false) => Script::NoLetters,
    }
}
/// One name in Latin letters, the other in another script (alone or with some Latin letters).
fn other_script(a: &str, b: &str) -> bool {
    use Script::*;
    matches!(
        (script(a), script(b)),
        (Latin, Other | Mixed) | (Other | Mixed, Latin)
    )
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
        // "(feat. Ena - Live)": the dash carries a version mark, so the group stays.
        if (credit && !inner.contains(" - ")) || is_remaster_note(inner) {
            return Some((start, start + len + 1));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn album_keys_leave_out_shared_self_titled_and_single_names() {
        assert_eq!(
            album_key("home", "Night Ferry OST", "North").as_deref(),
            Some("night ferry ost")
        );
        for (title, album, artist) in [
            ("home", "Greatest Hits", "North"),
            ("home", "Best of North", "North"),
            ("home", "North", "North"),
            ("home", "North", "North & South"),
            ("home", "Home - 2011 Remaster", "North"),
            ("home", "Home (feat. Q)", "North"),
            ("home", "Home", "North"),
            ("home", "", "North"),
        ] {
            assert_eq!(album_key(title, album, artist), None, "{album}");
        }
    }

    #[test]
    fn scripts_tell_japanese_names_from_latin_ones() {
        assert!(other_script("青葉 凛", "rin aoba"));
        assert!(other_script("灯台音楽 lhm", "lighthouse music"));
        assert!(!other_script("big label music", "studio band"));
        assert!(!other_script("南", "北"));
        assert!(!other_script("", "north"));
    }
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
