use serde_json::Value;
use std::{collections::HashMap, path::Path, sync::Arc};
use string_core::{
    library::{Library, Track},
    queue::{Catalog, Group},
    state::State,
};
use windows::Win32::Globalization::{NormalizationKD, NormalizeString};

pub type Ids = Arc<[String]>;
pub fn norm(s: &str) -> String {
    let decomposed = if s.is_ascii() {
        s.into()
    } else {
        let src: Vec<u16> = s.encode_utf16().collect();
        let estimate = unsafe { NormalizeString(NormalizationKD, &src, None) };
        let mut dst = vec![0; estimate.unsigned_abs() as usize];
        let written = unsafe { NormalizeString(NormalizationKD, &src, Some(&mut dst)) };
        if written > 0 {
            String::from_utf16_lossy(&dst[..written as usize])
        } else {
            s.into()
        }
    };
    decomposed
        .replace(|c| ('\u{0300}'..='\u{036f}').contains(&c), "")
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
fn filename_title(name: &str) -> String {
    let mut s = Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(name)
        .trim();
    if let Some((rest, end)) = s.rsplit_once(['[', '('])
        && let Some(id) = end.strip_suffix([']', ')'])
        && ((id.len() == 11
            && id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-'))
            || (id.len() >= 6 && id.bytes().all(|c| c.is_ascii_digit())))
    {
        s = rest.trim_end();
    }
    if s.ends_with(')')
        && let Some((rest, d)) = s.rsplit_once('(')
        && d.len() == 2
        && d.as_bytes()[0].is_ascii_digit()
    {
        s = rest.trim_end();
    }
    let digits = s.bytes().take_while(u8::is_ascii_digit).count();
    if (1..=3).contains(&digits) {
        let tail = s[digits..].trim_start();
        if let Some(rest) = tail.strip_prefix(['-', '.', ')', '_'])
            && rest.starts_with(char::is_whitespace)
            && !rest.trim().is_empty()
        {
            s = rest.trim();
        }
    }
    s.replace('_', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
#[derive(Clone, Debug, Default, Hash, PartialEq, Eq)]
pub struct CoverSource {
    pub file: String,
    pub embedded: String,
    pub offset: Option<(u64, u64)>,
    pub fallback: String,
    pub seed: String,
    pub revision: i64,
}
#[derive(Clone, Debug)]
pub struct Song {
    pub id: String,
    pub path: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub artist_key: String,
    pub album_key: String,
    pub duration: f64,
    pub year: u32,
    pub format: String,
    pub added: i64,
    pub cover: Arc<CoverSource>,
    pub number: u32,
    pub disc: u32,
    search: String,
}
impl Song {
    pub fn matches(&self, normalized_terms: &str) -> bool {
        normalized_terms
            .split_whitespace()
            .all(|term| self.search.contains(term))
    }
    pub fn lyrics_song(&self) -> string_lyrics::Song<'_> {
        string_lyrics::Song {
            id: &self.id,
            path: Path::new(&self.path),
            title: &self.title,
            artist: &self.artist,
            album: &self.album,
            duration: self.duration,
        }
    }
    pub fn artist_name(&self) -> &str {
        if self.artist.is_empty() {
            "Unknown artist"
        } else {
            &self.artist
        }
    }
}
#[derive(Default)]
pub struct Models {
    pub by_id: HashMap<String, Song>,
    pub catalog: Catalog,
    pub songs: Ids,
    pub liked: Ids,
    pub playlists: Vec<Playlist>,
    pub duration: f64,
    pub recent_songs: Vec<String>,
    pub recent_albums: Vec<usize>,
}
pub struct Playlist {
    pub id: String,
    pub name: String,
    pub ids: Ids,
}
fn make(t: &Track, state: &State) -> Song {
    let id = t.id();
    let e = state
        .fields
        .get("edits")
        .and_then(|v| v.get(&id))
        .unwrap_or(&Value::Null);
    let tag = |k: &str, default: &str| {
        e[k].as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or(default)
            .to_owned()
    };
    let path = Path::new(&t.path);
    let raw_name = path.file_name().unwrap_or_default().to_string_lossy();
    let mut title = tag("title", &t.tags.title);
    if title.is_empty() {
        title = filename_title(&raw_name)
    }
    let artist = tag("artist", &t.tags.artist);
    let album = tag("album", &t.tags.album);
    let artist_key = norm(&artist);
    let dir = path.parent().unwrap_or(Path::new("")).to_string_lossy();
    let album_key = if album.is_empty() {
        String::new()
    } else {
        format!(
            "{}\u{1}{}",
            norm(&album),
            norm(if e["album"].as_str().is_some_and(|s| !s.is_empty()) {
                &artist
            } else if t.tags.album_artist.is_empty() {
                &dir
            } else {
                &t.tags.album_artist
            })
        )
    };
    let search = norm(&format!(
        "{title} {artist} {album} {} {} {raw_name}",
        t.tags.album_artist, t.tags.genre
    ));
    let cover = Arc::new(CoverSource {
        file: tag("art", ""),
        embedded: if t.art.is_object() {
            t.path.clone()
        } else {
            String::new()
        },
        offset: t.art["o"].as_u64().zip(t.art["n"].as_u64()),
        fallback: t.cover.clone(),
        seed: if album_key.is_empty() {
            id.clone()
        } else {
            album_key.clone()
        },
        revision: t.mtime,
    });
    Song {
        id,
        path: t.path.clone(),
        title,
        artist,
        album,
        artist_key,
        album_key,
        search,
        cover,
        year: t.tags.year,
        duration: t.tags.duration,
        format: t.tags.format.to_uppercase(),
        added: t.added.unwrap_or(t.mtime),
        number: t.tags.track,
        disc: t.tags.disc,
    }
}
impl Models {
    pub fn build(lib: &Library, state: &State) -> Self {
        let by_id: HashMap<_, _> = lib
            .tracks
            .iter()
            .map(|t| {
                let s = make(t, state);
                (s.id.clone(), s)
            })
            .collect();
        let mut catalog = lib.catalog(state, |t| by_id[&t.id()].artist_key.clone());
        let mut albums: HashMap<String, Group> = HashMap::new();
        let mut artists: HashMap<String, Group> = HashMap::new();
        let mut duration = 0.;
        for id in &catalog.songs {
            let s = &by_id[id];
            duration += s.duration;
            for (map, key, name) in [
                (&mut albums, &s.album_key, s.album.as_str()),
                (&mut artists, &s.artist_key, s.artist_name()),
            ] {
                if name.is_empty() {
                    continue;
                }
                map.entry(key.clone())
                    .or_insert_with(|| Group {
                        key: key.clone(),
                        name: name.into(),
                        ids: vec![],
                    })
                    .ids
                    .push(id.clone());
            }
        }
        catalog
            .songs
            .sort_by_cached_key(|id| norm(&by_id[id].title));
        for (map, out, album) in [
            (albums, &mut catalog.albums, true),
            (artists, &mut catalog.artists, false),
        ] {
            *out = map.into_values().collect();
            out.sort_by_cached_key(|g| (norm(&g.name), g.key.clone()));
            for g in out {
                g.ids.sort_by_cached_key(|id| {
                    let s = &by_id[id];
                    (
                        if album { String::new() } else { norm(&s.album) },
                        s.disc,
                        if s.number == 0 { 999 } else { s.number },
                        norm(&s.title),
                    )
                });
            }
        }
        let mut recent_songs = catalog.songs.clone();
        recent_songs.sort_by_key(|id| std::cmp::Reverse(by_id[id].added));
        recent_songs.truncate(14);
        let mut recent_albums: Vec<_> = (0..catalog.albums.len()).collect();
        recent_albums.sort_by_cached_key(|&i| {
            std::cmp::Reverse(
                catalog.albums[i]
                    .ids
                    .iter()
                    .map(|id| by_id[id].added)
                    .max()
                    .unwrap_or(0),
            )
        });
        recent_albums.truncate(14);
        let songs = catalog.songs.clone().into();
        let mut m = Self {
            by_id,
            catalog,
            songs,
            duration,
            recent_songs,
            recent_albums,
            ..Default::default()
        };
        m.sort_songs(state);
        m.refresh_lists(state);
        m
    }
    pub fn sort_songs(&mut self, state: &State) {
        let sort = state.fields.get("sort").unwrap_or(&Value::Null);
        let key = sort["key"].as_str().unwrap_or("title");
        let ordered_text = |s: &str| {
            if s.is_empty() {
                String::from("\u{ffff}")
            } else {
                norm(s)
            }
        };
        self.catalog.songs.sort_by_cached_key(|id| {
            let s = &self.by_id[id];
            let (text, n, disc, track) = match key {
                "added" => (String::new(), i64::MAX.saturating_sub(s.added), 0, 0),
                "duration" => (String::new(), (s.duration * 1000.) as i64, 0, 0),
                "album" => (ordered_text(&s.album), 0, s.disc, s.number),
                "artist" => (
                    format!("{}\u{1}{}", ordered_text(&s.artist), ordered_text(&s.album)),
                    0,
                    s.disc,
                    s.number,
                ),
                _ => (norm(&s.title), 0, 0, 0),
            };
            (
                text,
                n,
                disc,
                if track == 0 { 999 } else { track },
                norm(&s.title),
            )
        });
        if sort["dir"] == -1 {
            self.catalog.songs.reverse();
        }
        self.songs = self.catalog.songs.clone().into();
    }
    pub fn refresh_lists(&mut self, state: &State) {
        let hidden = state
            .fields
            .get("hidden")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|entries| entries.keys())
            .filter(|id| state.hidden(id) && self.by_id.contains_key(*id));
        self.liked = self
            .songs
            .iter()
            .chain(hidden)
            .filter(|id| {
                state
                    .fields
                    .get("liked")
                    .and_then(|v| v.get(*id))
                    .is_some_and(|v| !v.is_null() && v != false && v.as_i64() != Some(0))
            })
            .cloned()
            .collect();
        Arc::make_mut(&mut self.liked).sort_by_cached_key(|id| {
            std::cmp::Reverse(
                state
                    .fields
                    .get("liked")
                    .and_then(|v| v[id].as_i64())
                    .unwrap_or(0),
            )
        });
        self.playlists = state
            .fields
            .get("playlists")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|p| {
                Some(Playlist {
                    id: p["id"].as_str()?.into(),
                    name: p["name"].as_str()?.into(),
                    ids: p["items"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|v| v.as_str().or(v["id"].as_str()))
                        .filter(|id| self.by_id.contains_key(*id))
                        .map(str::to_owned)
                        .collect(),
                })
            })
            .collect();
    }
    pub fn search(&self, q: &str) -> Ids {
        let q = norm(q);
        if q.is_empty() {
            return Ids::default();
        }
        let mut result: Vec<_> = self
            .songs
            .iter()
            .filter(|id| self.by_id[*id].matches(&q))
            .cloned()
            .collect();
        result.sort_by_cached_key(|id| {
            let s = &self.by_id[id];
            let title = norm(&s.title);
            let rank = if title == q {
                0
            } else if title.starts_with(&q) {
                1
            } else if title.contains(&q) {
                2
            } else if s.artist_key.contains(&q) {
                3
            } else {
                4
            };
            (rank, title)
        });
        result.into()
    }
    pub fn context_ids(&self, ctx: &Value) -> Ids {
        let k = ctx["key"].as_str().unwrap_or("");
        match ctx["kind"].as_str() {
            Some("liked") => self.liked.clone(),
            Some("pl") => self
                .playlists
                .iter()
                .find(|p| p.id == k)
                .map(|p| p.ids.clone())
                .unwrap_or_default(),
            Some("album" | "artist") => {
                let groups = if ctx["kind"] == "album" {
                    &self.catalog.albums
                } else {
                    &self.catalog.artists
                };
                groups
                    .iter()
                    .find(|g| g.key == k)
                    .map(|g| g.ids.clone().into())
                    .unwrap_or_default()
            }
            Some("search") => self.search(k),
            _ => self.songs.clone(),
        }
    }
}
