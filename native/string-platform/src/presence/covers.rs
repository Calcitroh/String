//! Covers for Discord, only when the person turned them on, and then only the song's and artist's names leave the PC:
//! Apple's iTunes search, then Deezer's. A search's song counts only if its artist and title are the person's (set apart
//! from case and punctuation, or one inside the other). Remembered in `discord-covers.json` ("v2\n<artist>\n<title>",
//! the newest 3000): a cover found, or "" once both searches said no. A lookup that failed (offline) isn't remembered:
//! it's tried again, but not for 30 s.

use serde::de::{Deserializer, MapAccess, Visitor};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fmt::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use std::{fs, thread};

const KEPT: usize = 3000;
/// A failed lookup is tried again after this long (the presence looks ten times a second).
const RETRY: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub(crate) struct Covers {
    shared: Arc<Shared>,
}

struct Shared {
    cache: Mutex<Cache>,
    saved: Mutex<u64>,
    file: PathBuf,
    itunes: String,
    deezer: String,
}

#[derive(Default)]
struct Cache {
    found: HashMap<String, (u64, String)>,
    next: u64,
    looking: HashSet<String>,
    failed: HashMap<String, Instant>,
    version: u64,
}

impl Covers {
    pub fn load(file: &Path, itunes: &str, deezer: &str) -> Covers {
        let mut cache = Cache::default();
        if let Ok(text) = fs::read_to_string(file) {
            for (k, v) in parse(&text) {
                cache.insert(k, v);
            }
        }
        Covers {
            shared: Arc::new(Shared {
                cache: Mutex::new(cache),
                saved: Mutex::new(0),
                file: file.to_path_buf(),
                itunes: itunes.to_string(),
                deezer: deezer.to_string(),
            }),
        }
    }

    /// The cover's address if known ("" when none was found). Otherwise "" for now, and a lookup starts on a thread of
    /// its own (one per song at a time), which calls `done` when it's over.
    pub fn get(&self, artist: &str, title: &str, done: impl FnOnce() + Send + 'static) -> String {
        let key = key(artist, title);
        {
            let mut c = lock(&self.shared.cache);
            if let Some((_, url)) = c.found.get(&key) {
                return url.clone();
            }
            if c.failed.get(&key).is_some_and(|t| t.elapsed() < RETRY) {
                return String::new();
            }
            if !c.looking.insert(key.clone()) {
                return String::new();
            }
        }
        let shared = self.shared.clone();
        let (artist, title) = (artist.to_string(), title.to_string());
        let mine = key.clone();
        let started = thread::Builder::new()
            .name("string-cover".into())
            .spawn(move || {
                let (found, apple, deezer) = lookup(&shared, &artist, &title);
                let mut save = None;
                {
                    let mut c = lock(&shared.cache);
                    c.looking.remove(&key);
                    if !found.is_empty() || (apple && deezer) {
                        c.failed.remove(&key);
                        c.insert(key, found);
                        c.version += 1;
                        save = Some((c.json(), c.version));
                    } else {
                        c.failed.insert(key, Instant::now());
                    }
                }
                // written after letting go of the cache, so a slow disk holds up nothing
                if let Some((json, version)) = save {
                    shared.save(&json, version);
                }
                done();
            });
        if started.is_err() {
            lock(&self.shared.cache).looking.remove(&mine);
        }
        String::new()
    }
}

impl Cache {
    fn insert(&mut self, key: String, url: String) {
        if let Some(entry) = self.found.get_mut(&key) {
            entry.1 = url;
        } else {
            self.found.insert(key, (self.next, url));
            self.next += 1;
        }
    }

    /// The newest 3000, oldest first.
    fn json(&self) -> String {
        let mut all: Vec<_> = self.found.iter().collect();
        all.sort_by_key(|(_, (seq, _))| *seq);
        let mut out = String::from("{");
        for (i, (k, (_, v))) in all.iter().skip(all.len().saturating_sub(KEPT)).enumerate() {
            if i > 0 {
                out.push(',');
            }
            let _ = write!(
                out,
                "{}:{}",
                Value::from(k.as_str()),
                Value::from(v.as_str())
            );
        }
        out.push('}');
        out
    }
}

impl Shared {
    /// One write at a time, and never an older list over a newer one.
    fn save(&self, json: &str, version: u64) {
        let mut saved = lock(&self.saved);
        if version <= *saved {
            return;
        }
        let mut tmp = self.file.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        if fs::write(&tmp, json).is_ok() && fs::rename(&tmp, &self.file).is_ok() {
            *saved = version;
        }
    }

    fn json(&self, url: &str) -> Option<Value> {
        let mut response = string_http::HTTP
            .get(url, Duration::from_secs(6), 10, |r| {
                r.config().user_agent("String/1.0").build()
            })
            .ok()?;
        serde_json::from_str(&response.body_mut().read_to_string().ok()?).ok()
    }
}

/// Asks Apple, then (if Apple had nothing) Deezer: the cover found ("" if none), and whether each one answered.
fn lookup(shared: &Shared, artist: &str, title: &str) -> (String, bool, bool) {
    let (na, nt) = (normalize(artist), normalize(title));
    let same = |a: Option<&str>, t: Option<&str>| matches(a, artist, &na) && matches(t, title, &nt);
    let (mut apple, mut deezer) = (false, false);
    let mut found = String::new();
    let url = format!(
        "{}?media=music&entity=song&limit=5&term={}",
        shared.itunes,
        escape(&format!("{artist} {title}"))
    );
    if let Some(j) = shared.json(&url) {
        apple = true;
        for r in j["results"].as_array().into_iter().flatten() {
            let art = r["artworkUrl100"]
                .as_str()
                .filter(|a| a.starts_with("https://"));
            if let Some(art) =
                art.filter(|_| same(r["artistName"].as_str(), r["trackName"].as_str()))
            {
                found = art.replace("100x100bb", "600x600bb");
                break;
            }
        }
    }
    if found.is_empty() {
        // exact first (artist:"…" track:"…"), then loose words; a failed ask ends both
        for q in [
            format!("artist:\"{artist}\" track:\"{title}\""),
            format!("{artist} {title}"),
        ] {
            let Some(j) = shared.json(&format!("{}?limit=5&q={}", shared.deezer, escape(&q)))
            else {
                break;
            };
            deezer = true;
            for r in j["data"].as_array().into_iter().flatten() {
                let album = &r["album"];
                let art = album["cover_xl"]
                    .as_str()
                    .or(album["cover_big"].as_str())
                    .filter(|a| a.starts_with("https://"));
                if let Some(art) =
                    art.filter(|_| same(r["artist"]["name"].as_str(), r["title"].as_str()))
                {
                    found = art.to_string();
                    break;
                }
            }
            if !found.is_empty() {
                break;
            }
        }
    }
    (found, apple, deezer)
}

/// "v2\n<artist>\n<title>", trimmed and in lower case.
pub(crate) fn key(artist: &str, title: &str) -> String {
    format!("v2\n{}\n{}", lower(artist.trim()), lower(title.trim()))
}

/// Lower case one character for one, as .NET's ToLowerInvariant (the 1.x keys were made with it).
fn lower(s: &str) -> String {
    s.chars()
        .map(|c| {
            let mut l = c.to_lowercase();
            match (l.next(), l.next()) {
                (Some(x), None) => x,
                _ if c == '\u{130}' => 'i',
                _ => c,
            }
        })
        .collect()
}

/// Letters and digits only, in lower case, every run of anything else one space.
pub(crate) fn normalize(s: &str) -> String {
    let mut out = String::new();
    let mut gap = false;
    for c in lower(s).chars() {
        if c.is_alphabetic() || c.is_numeric() {
            if gap && !out.is_empty() {
                out.push(' ');
            }
            gap = false;
            out.push(c);
        } else {
            gap = true;
        }
    }
    out
}

/// One of the search's names against the person's: the same once punctuation and case are set aside, or one inside the
/// other ("Remastered" and the like on either side are fine). A name of symbols only ("!!!") has nothing left after
/// that and would match anything: then the search's has to be the same symbols.
pub(crate) fn matches(theirs: Option<&str>, mine: &str, normalized_mine: &str) -> bool {
    let theirs = theirs.unwrap_or("");
    if normalized_mine.is_empty() {
        let mine = mine.trim();
        return !mine.is_empty() && upper(theirs.trim()) == upper(mine);
    }
    let r = normalize(theirs);
    !r.is_empty() && (r.contains(normalized_mine) || normalized_mine.contains(&r))
}

fn upper(s: &str) -> String {
    s.chars()
        .map(|c| {
            let mut u = c.to_uppercase();
            match (u.next(), u.next()) {
                (Some(x), None) => x,
                _ => c,
            }
        })
        .collect()
}

/// .NET's Uri.EscapeDataString: everything but letters, digits and `-._~` as %XX of its UTF-8 bytes.
pub(crate) fn escape(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// The covers file's entries in the file's order (oldest first); a value that isn't text counts as "".
fn parse(text: &str) -> Vec<(String, String)> {
    struct Entries;
    impl<'de> Visitor<'de> for Entries {
        type Value = Vec<(String, String)>;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("an object")
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut out = Vec::new();
            while let Some((k, v)) = map.next_entry::<String, Value>()? {
                out.push((k, v.as_str().unwrap_or("").to_string()));
            }
            Ok(out)
        }
    }
    let mut de = serde_json::Deserializer::from_str(text);
    de.deserialize_map(Entries).unwrap_or_default()
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_string_1x_keys() {
        assert_eq!(
            key(" Test Signal ", "Glass Orbit"),
            "v2\ntest signal\nglass orbit"
        );
        assert_eq!(
            key("ΣΟΦΟΣ", "İstanbul"),
            "v2\nσοφοσ\nistanbul",
            "one character for one, no final sigma"
        );
    }

    #[test]
    fn names_match_like_string_1x() {
        assert_eq!(
            normalize("  Glass Orbit (Remastered) "),
            "glass orbit remastered"
        );
        assert_eq!(normalize("AC/DC"), "ac dc");
        assert_eq!(normalize("!!!"), "");
        let n = normalize("Glass Orbit");
        assert!(matches(Some("Glass Orbit (Remastered)"), "Glass Orbit", &n));
        assert!(matches(Some("glass-orbit"), "Glass Orbit", &n));
        assert!(
            matches(Some("Orbit"), "Glass Orbit", &n),
            "one inside the other"
        );
        assert!(!matches(Some("Someone Else"), "Glass Orbit", &n));
        assert!(!matches(None, "Glass Orbit", &n));
        assert!(!matches(Some("!!!"), "Glass Orbit", &n));
        // symbols only: the same symbols or nothing
        assert!(matches(Some(" !!! "), "!!!", ""));
        assert!(!matches(Some("Test Signal"), "!!!", ""));
        assert!(!matches(Some(""), "   ", ""));
    }

    #[test]
    fn escaped_like_uri_escape_data_string() {
        assert_eq!(
            escape("Test Signal Glass Orbit"),
            "Test%20Signal%20Glass%20Orbit"
        );
        assert_eq!(
            escape(r#"artist:"A&B" track:"x+y""#),
            "artist%3A%22A%26B%22%20track%3A%22x%2By%22"
        );
        assert_eq!(escape("Björk-_.~"), "Bj%C3%B6rk-_.~");
    }

    #[test]
    fn a_failed_lookup_waits_before_asking_again() {
        // nothing listens on port 9 here: both searches fail at once
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("discord-covers.json");
        let covers = Covers::load(&file, "http://127.0.0.1:9/none", "http://127.0.0.1:9/none");
        let (tx, rx) = std::sync::mpsc::channel();
        let t = tx.clone();
        assert_eq!(covers.get("A", "B", move || t.send(()).unwrap()), "");
        rx.recv_timeout(Duration::from_secs(10))
            .expect("the lookup ends");
        assert_eq!(covers.get("A", "B", move || tx.send(()).unwrap()), "");
        assert!(
            rx.recv_timeout(Duration::from_millis(500)).is_err(),
            "not asked again straight away"
        );
        assert!(!file.exists(), "a failed lookup isn't remembered");
    }

    #[test]
    fn file_keeps_order_and_the_newest_3000() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("discord-covers.json");
        fs::write(
            &file,
            r#"{"v2\nb\nb":"https://x.test/b.jpg","v2\na\na":"","v2\nc\nc":7}"#,
        )
        .unwrap();
        let covers = Covers::load(&file, "http://127.0.0.1:9/none", "http://127.0.0.1:9/none");
        assert_eq!(
            covers.get("B", "b", || panic!("known: no lookup")),
            "https://x.test/b.jpg"
        );
        assert_eq!(covers.get("a", "A", || panic!("known: no lookup")), "");
        let mut c = lock(&covers.shared.cache);
        assert_eq!(
            c.json(),
            r#"{"v2\nb\nb":"https://x.test/b.jpg","v2\na\na":"","v2\nc\nc":""}"#
        );
        for i in 0..3100 {
            c.insert(
                format!("v2\nartist\n{i}"),
                format!("https://x.test/{i}.jpg"),
            );
        }
        let kept = parse(&c.json());
        assert_eq!(kept.len(), 3000);
        assert_eq!(kept[0].0, "v2\nartist\n100");
        assert_eq!(kept[2999].0, "v2\nartist\n3099");
        assert!(parse("[1,2]").is_empty() && parse("{").is_empty());
    }
}
