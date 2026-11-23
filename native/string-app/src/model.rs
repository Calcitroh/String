use serde_json::Value;
use std::{cmp::Reverse, collections::HashMap, path::Path, sync::Arc};
use string_core::{
    library::{Library, Track},
    queue::{Catalog, Group},
    state::State,
};
use windows::Win32::Globalization::{NormalizationKD, NormalizeString};

mod collation;
pub type Ids = Arc<[String]>;
#[cfg(test)]
mod order_tests;
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
    let title = s
        .replace('_', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if title.is_empty() {
        name.to_owned()
    } else {
        title
    }
}

fn filename_number(name: &str) -> u32 {
    let js_space = |c: char| c == '\u{feff}' || (c.is_whitespace() && c != '\u{85}');
    let name = name.trim_start_matches(js_space);
    let digits = name.bytes().take_while(u8::is_ascii_digit).count();
    if !(1..=3).contains(&digits) {
        return 0;
    }
    let tail = &name[digits..];
    let rest = tail.trim_start_matches(js_space);
    // The separator can be a literal space or punctuation after optional whitespace.
    if tail[..tail.len() - rest.len()].contains(' ') || rest.starts_with(['-', '.', ')', '_']) {
        name[..digits].parse().unwrap_or(0)
    } else {
        0
    }
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
    library_index: usize,
}
impl Song {
    fn track_order(&self) -> u32 {
        if self.number == 0 { 999 } else { self.number }
    }
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
    pub album_info: HashMap<String, AlbumInfo>,
    title_order: Vec<String>,
    text_keys: HashMap<String, Box<[u8]>>,
}
pub struct AlbumInfo {
    pub artist: String,
    pub artist_key: String,
    pub year: u32,
    pub added: i64,
    pub duration: f64,
    pub first: usize,
}

impl AlbumInfo {
    // The library's first credit wins a tie. Read it before sorting the tracks on the album.
    fn from_group(
        group: &Group,
        songs: &HashMap<String, Song>,
        lib: &Library,
        state: &State,
    ) -> Self {
        let first = songs[&group.ids[0]].library_index;
        let mut names = HashMap::new();
        let mut year = 0;
        let mut added = 0;
        let mut duration = 0.;
        let edits = state.fields.get("edits");
        for id in &group.ids {
            let song = &songs[id];
            let original = &lib.tracks[song.library_index].tags.album_artist;
            let edited = edits
                .and_then(|e| e[id]["artist"].as_str())
                .is_some_and(|a| !a.is_empty());
            let credit = if edited || original.is_empty() {
                &song.artist
            } else {
                original
            };
            if !credit.is_empty() {
                names
                    .entry(credit.as_str())
                    .or_insert((0usize, Reverse(song.library_index)))
                    .0 += 1;
            }
            year = year.max(song.year);
            added = added.max(song.added);
            duration += song.duration;
        }
        let primary = names
            .iter()
            .max_by_key(|(_, count)| *count)
            .map_or("", |(&name, _)| name);
        let artist = if names.len() > 2 && lib.tracks[first].tags.album_artist.is_empty() {
            "Various artists"
        } else if primary.is_empty() {
            "Unknown artist"
        } else {
            primary
        };
        Self {
            artist: artist.into(),
            artist_key: norm(primary),
            year,
            added,
            duration,
            first,
        }
    }
}
pub struct Playlist {
    pub id: String,
    pub name: String,
    pub ids: Ids,
}
fn make(t: &Track, state: &State, library_index: usize) -> Song {
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
    let (dir, raw_name) = t.path.rsplit_once(['\\', '/']).unwrap_or(("", &t.path));
    let mut title = tag("title", &t.tags.title);
    if title.is_empty() {
        title = filename_title(raw_name)
    }
    let artist = tag("artist", &t.tags.artist);
    let album = tag("album", &t.tags.album);
    let artist_key = norm(&artist);
    let album_key = if album.is_empty() {
        String::new()
    } else {
        format!(
            "{}\u{1}{}",
            norm(&album),
            norm(if e["album"].as_str().is_some_and(|s| !s.is_empty()) {
                &artist
            } else if t.tags.album_artist.is_empty() {
                dir
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
        added: t.added.unwrap_or(0),
        number: if t.tags.track == 0 {
            filename_number(raw_name)
        } else {
            t.tags.track
        },
        disc: t.tags.disc,
        library_index,
    }
}
impl Models {
    pub fn build(lib: &Library, state: &State) -> Self {
        let by_id: HashMap<_, _> = lib
            .tracks
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let s = make(t, state, i);
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
        let album_info: HashMap<_, _> = albums
            .values()
            .map(|g| (g.key.clone(), AlbumInfo::from_group(g, &by_id, lib, state)))
            .collect();
        let text_keys = collation::keys(
            by_id
                .values()
                .flat_map(|s| [s.title.as_str(), s.artist.as_str(), s.album.as_str()])
                .chain(album_info.values().map(|a| a.artist.as_str()))
                .chain(["Unknown artist", "\u{ffff}"]),
        );
        catalog.albums = albums.into_values().collect();
        for g in &mut catalog.albums {
            g.ids.sort_by_cached_key(|id| {
                let s = &by_id[id];
                (s.disc, s.track_order(), &text_keys[&s.title])
            });
        }
        catalog.artists = artists.into_values().collect();
        catalog.artists.sort_by_cached_key(|g| {
            (
                g.key.is_empty(),
                &text_keys[&g.name],
                by_id[&g.ids[0]].library_index,
            )
        });
        for g in &mut catalog.artists {
            g.ids.sort_by_cached_key(|id| {
                let s = &by_id[id];
                (&text_keys[&s.album], s.track_order(), &text_keys[&s.title])
            });
        }
        catalog.songs.sort_by_cached_key(|id| {
            let s = &by_id[id];
            (&text_keys[&s.title], &text_keys[&s.artist])
        });
        let mut recent_songs = catalog.songs.clone();
        recent_songs.sort_by_key(|id| Reverse(by_id[id].added));
        recent_songs.truncate(14);
        let title_order = catalog.songs.clone();
        let mut m = Self {
            by_id,
            catalog,
            title_order,
            text_keys,
            duration,
            recent_songs,
            album_info,
            ..Default::default()
        };
        m.sort_albums(state);
        m.sort_songs(state);
        m.refresh_lists(state);
        m
    }
    pub fn sort_albums(&mut self, state: &State) {
        let mode = state.fields.get("albumSort").and_then(Value::as_str);
        let keys = &self.text_keys;
        self.catalog.albums.sort_by(|left, right| {
            let a = &self.album_info[&left.key];
            let b = &self.album_info[&right.key];
            let by_name = || keys[&left.name].cmp(&keys[&right.name]);
            match mode {
                Some("recent") => b.added.cmp(&a.added),
                Some("year") => b.year.cmp(&a.year).then_with(by_name),
                Some("name") => by_name(),
                _ => keys[&a.artist]
                    .cmp(&keys[&b.artist])
                    .then_with(|| a.year.cmp(&b.year))
                    .then_with(by_name),
            }
            .then_with(|| a.first.cmp(&b.first))
        });
    }
    pub fn sort_songs(&mut self, state: &State) {
        let sort = state.fields.get("sort").unwrap_or(&Value::Null);
        // Every stable page sort starts from library order (title, then artist).
        let ids = &mut self.catalog.songs;
        ids.clone_from(&self.title_order);
        let keys = &self.text_keys;
        let missing_last = |name: &str| &keys[if name.is_empty() { "\u{ffff}" } else { name }];
        match sort["key"].as_str() {
            Some("artist") => ids.sort_by_cached_key(|id| {
                let s = &self.by_id[id];
                (missing_last(&s.artist), &keys[&s.album], s.track_order())
            }),
            Some("album") => ids.sort_by_cached_key(|id| {
                let s = &self.by_id[id];
                (missing_last(&s.album), s.disc, s.track_order())
            }),
            Some("added") => ids.sort_by_cached_key(|id| Reverse(self.by_id[id].added)),
            Some("duration") => {
                ids.sort_by(|a, b| self.by_id[a].duration.total_cmp(&self.by_id[b].duration))
            }
            _ => {}
        }
        if sort["dir"].as_f64().is_some_and(|d| d < 0.) {
            ids.reverse();
        }
        self.songs = ids.clone().into();
    }
    pub fn refresh_lists(&mut self, state: &State) {
        self.liked = self
            .songs
            .iter()
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
                        .filter(|id| self.by_id.contains_key(*id) && !state.hidden(id))
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
            (rank, &self.text_keys[&s.title], s.library_index)
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
                let (groups, k) = if ctx["kind"] == "album" {
                    (&self.catalog.albums, self.catalog.resolve_album_key(k))
                } else {
                    (&self.catalog.artists, k)
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
