use crate::{
    Line, cache,
    lrclib::{Error, Lrclib},
    parse_lrc, trim,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// The library ID and path, plus the effective metadata shown by the screen.
#[derive(Clone, Copy, Debug)]
pub struct Song<'a> {
    pub id: &'a str,
    pub path: &'a Path,
    pub title: &'a str,
    pub artist: &'a str,
    pub album: &'a str,
    pub duration: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Lrc,
    File,
    Lrclib,
    None,
}

#[derive(Clone, Debug)]
pub struct LyricsResult {
    pub source: Source,
    pub text: String,
    pub synced: bool,
    pub lines: Vec<Line>,
    pub instrumental: bool,
    pub online: bool,
    pub busy: bool,
    pub retry: bool,
}
impl LyricsResult {
    fn new(source: Source, text: String, online: bool, instrumental: bool) -> Self {
        let parsed = parse_lrc(&text);
        Self {
            source,
            text,
            synced: parsed.synced,
            lines: parsed.lines,
            instrumental,
            online,
            busy: false,
            retry: false,
        }
    }
    fn needs_online(&self, asked: bool) -> bool {
        !self.online
            && !asked
            && !self.synced
            && matches!(self.source, Source::None | Source::Lrc | Source::File)
    }
}

#[derive(Deserialize, Serialize)]
struct Record {
    v: u32,
    source: Source,
    #[serde(default)]
    text: String,
    #[serde(default)]
    instrumental: bool,
    #[serde(default)]
    online: bool,
    #[serde(default)]
    asked: bool,
    at: Option<i64>,
}
struct Memo {
    result: LyricsResult,
    expires: Option<Instant>,
    online_allowed: bool,
    asked: bool,
}
type Slot = Arc<Mutex<Option<Memo>>>;
struct Session {
    songs: HashMap<String, Slot>,
    last_sweep: Instant,
}
const RETRY_DELAY: Duration = Duration::from_secs(60);
impl Session {
    fn slot(&mut self, id: &str) -> Slot {
        let now = Instant::now();
        if now.duration_since(self.last_sweep) >= RETRY_DELAY {
            self.songs.retain(|_, slot| {
                // Workers hold a clone while looking up a song. An unshared slot is idle.
                Arc::strong_count(slot) > 1
                    || slot
                        .lock()
                        .unwrap()
                        .as_ref()
                        .is_some_and(|memo| memo.expires.is_none_or(|end| now < end))
            });
            self.last_sweep = now;
        }
        self.songs.entry(id.into()).or_default().clone()
    }
}
pub struct Lyrics {
    dir: PathBuf,
    service: Lrclib,
    session: Mutex<Session>,
}

impl Lyrics {
    pub fn new(data: impl AsRef<Path>) -> Self {
        Self::with_base_url(data, "https://lrclib.net")
    }
    pub fn with_base_url(data: impl AsRef<Path>, base: &str) -> Self {
        Self {
            dir: data.as_ref().join("lyrics"),
            service: Lrclib::new(base),
            session: Mutex::new(Session {
                songs: HashMap::new(),
                last_sweep: Instant::now(),
            }),
        }
    }
    /// `force` bypasses both caches, as for an explicit retry. On ordinary calls, concurrent
    /// requests for a song share one lookup. Transient failures can be retried after a minute.
    pub fn get(&self, song: &Song<'_>, allow_online: bool, force: bool) -> LyricsResult {
        let slot = self.session.lock().unwrap().slot(song.id);
        let mut current = slot.lock().unwrap();
        if !force
            && let Some(m) = current.as_ref()
            && m.expires.is_none_or(|d| Instant::now() < d)
            && !(allow_online && !m.online_allowed && m.result.needs_online(m.asked))
        {
            return m.result.clone();
        }
        let (result, asked) = self.find(song, allow_online, force);
        let temporary = result.retry || result.busy || result.source == Source::None;
        *current = Some(Memo {
            result: result.clone(),
            online_allowed: allow_online,
            asked,
            expires: temporary.then(|| Instant::now() + RETRY_DELAY),
        });
        result
    }
    /// Clear this session's memo for the song. Its shared disk cache stays available.
    pub fn forget(&self, id: &str) {
        let slot = self.session.lock().unwrap().songs.get(id).cloned();
        if let Some(slot) = slot {
            *slot.lock().unwrap() = None;
        }
    }

    fn find(&self, song: &Song<'_>, allow_online: bool, force: bool) -> (LyricsResult, bool) {
        const MONTH_MS: i64 = 30 * 86_400_000;
        let path = self.dir.join(format!(
            "{:x}-{}.json",
            cache::hash(song.id),
            song.duration.round() as i64
        ));
        if !force
            && let Some(c) = cache::read::<Record>(&path)
            && c.v == 1
        {
            let cached = LyricsResult::new(c.source, c.text, c.online, c.instrumental);
            let stale = c.source == Source::None
                && c.at
                    .is_some_and(|at| Utc::now().timestamp_millis().saturating_sub(at) > MONTH_MS);
            if !(stale || allow_online && cached.needs_online(c.asked)) {
                return (cached, c.asked);
            }
        }
        let mut found = local(song.path);
        let mut asked = false;
        if allow_online && !found.synced {
            match self.service.lookup(song) {
                Ok(hit) => {
                    asked = true;
                    if let Some(hit) = hit
                        && (hit.text().is_some() || hit.instrumental == Some(true))
                    {
                        let text = hit.text().unwrap_or("");
                        found =
                            LyricsResult::new(Source::Lrclib, text.into(), true, text.is_empty());
                    } else if found.source == Source::None {
                        found.online = true;
                    }
                }
                Err(e) => {
                    found.retry = found.source != Source::None;
                    found.busy = !found.retry && matches!(e, Error::Busy);
                    return (found, false);
                }
            }
        }
        let record = Record {
            v: 1,
            source: found.source,
            text: found.text.clone(),
            online: found.online,
            instrumental: found.instrumental,
            asked,
            at: Some(Utc::now().timestamp_millis()),
        };
        let _ = cache::write(&path, &record);
        (found, asked)
    }
}

fn local(path: &Path) -> LyricsResult {
    if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
        let stem = name
            .rsplit_once('.')
            .filter(|(_, ext)| !ext.is_empty())
            .map_or(name, |(s, _)| s);
        if let Some(text) = cache::read_text(&path.with_file_name(format!("{stem}.lrc"))) {
            let text = trim(&text);
            if !text.is_empty() {
                return LyricsResult::new(Source::Lrc, text.into(), false, false);
            }
        }
    }
    if let Ok(tags) = string_core::tags::read(path)
        && let Some(text) = tags.lyrics
    {
        return LyricsResult::new(Source::File, text, false, false);
    }
    LyricsResult::new(Source::None, String::new(), false, false)
}
