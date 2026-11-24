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

/// How many songs on the same album must prove that two spellings name one artist.
const SAME_ARTIST_PROOF: usize = 2;

/// The library's songs by (normalized title, main artist) and by (normalized title, album), and its artist spellings.
pub struct Matcher {
    songs: HashMap<(String, String), Track>,
    recordings: HashMap<(String, String), Track>,
    artists: HashMap<String, String>,
}

impl Matcher {
    /// `songs` are in library order and carry the names and id String plays them under. The first of duplicates wins.
    pub fn new<S: Into<LibrarySong>>(songs: impl IntoIterator<Item = S>) -> Matcher {
        let mut by_key = HashMap::new();
        let mut recordings = HashMap::new();
        let mut artists = HashMap::new();
        for song in songs {
            let LibrarySong { track: song, album } = song.into();
            artists
                .entry(normalize_text(&song.2))
                .or_insert_with(|| song.2.clone());
            let title = normalize_title(&song.1);
            if title.is_empty() {
                continue;
            }
            if let Some(album) = album_key(&title, &album) {
                recordings
                    .entry((title.clone(), album))
                    .or_insert_with(|| song.clone());
            }
            by_key.entry((title, main_artist(&song.2))).or_insert(song);
        }
        Matcher {
            songs: by_key,
            recordings,
            artists,
        }
    }

    /// A play's library song: the same title by the same main artist, else the same title on the same album (the
    /// same recording, its artist spelled another way), else the same title by the library artist `aliases` says
    /// this spelling is.
    fn find(
        &self,
        history: &Listening,
        t: &Track,
        aliases: &HashMap<String, String>,
    ) -> Option<&Track> {
        let title = normalize_title(&t.1);
        // A title that is only punctuation or a credit would match any such song by the artist.
        if title.is_empty() {
            return None;
        }
        let artist = main_artist(&t.2);
        if let Some(song) = self.songs.get(&(title.clone(), artist.clone())) {
            return Some(song);
        }
        if let Some(song) = self.recording(history, t, &title) {
            return Some(song);
        }
        self.songs.get(&(title, aliases.get(&artist)?.clone()))
    }
    fn recording(&self, history: &Listening, t: &Track, title: &str) -> Option<&Track> {
        let album = album_key(title, history.album(&t.0)?)?;
        self.recordings.get(&(title.to_string(), album))
    }

    /// Spotify spellings (main artist) that name a library artist: at least `SAME_ARTIST_PROOF` different songs of
    /// theirs found on the same album under that artist, and more than under any other.
    fn aliases(&self, history: &Listening) -> HashMap<String, String> {
        let mut proof: HashMap<String, HashMap<String, HashSet<String>>> = HashMap::new();
        for t in history.tracks() {
            let title = normalize_title(&t.1);
            let artist = main_artist(&t.2);
            if title.is_empty() || self.songs.contains_key(&(title.clone(), artist.clone())) {
                continue;
            }
            if let Some(song) = self.recording(history, t, &title) {
                let library = main_artist(&song.2);
                if library != artist {
                    proof
                        .entry(artist)
                        .or_default()
                        .entry(library)
                        .or_default()
                        .insert(title);
                }
            }
        }
        proof
            .into_iter()
            .filter_map(|(spelling, found)| {
                let most = found.values().map(HashSet::len).max()?;
                let mut best = found.into_iter().filter(|(_, titles)| titles.len() == most);
                let (library, _) = best.next()?;
                (most >= SAME_ARTIST_PROOF && best.next().is_none()).then_some((spelling, library))
            })
            .collect()
    }

    pub fn resolve(&self, history: &Listening) -> (Listening, Resolved) {
        let aliases = self.aliases(history);
        let mut matched = HashSet::new();
        let out = history.remap(|t| {
            if let Some(song) = self.find(history, t, &aliases) {
                matched.insert(song.0.clone());
                return song.clone();
            }
            let artist = self
                .artists
                .get(&normalize_text(&t.2))
                .or_else(|| self.artists.get(aliases.get(&main_artist(&t.2))?))
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

/// An album that can tell recordings apart: named, and not a single named after its song.
fn album_key(title: &str, album: &str) -> Option<String> {
    let album = normalize_text(album);
    (!album.is_empty() && album != title && normalize_title(&album) != title).then_some(album)
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
