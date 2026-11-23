//! Scan selected roots and loose files; keep the sparse library.json cache used by the desktop app.

mod scan;
use crate::{
    Fields,
    state::State,
    tags::{Tags, zero},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{io, path::PathBuf};

pub fn track_id(path: &str) -> String {
    path.to_lowercase()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub path: String,
    #[serde(default, skip_serializing_if = "zero")]
    pub size: u64,
    #[serde(default, skip_serializing_if = "zero")]
    pub mtime: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub added: Option<i64>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cover: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub art: Value,
    #[serde(default, skip_serializing_if = "zero")]
    pub read_error: u8,
    #[serde(flatten)]
    pub tags: Tags,
    #[serde(flatten)]
    pub extra: Fields,
}
impl Track {
    pub fn id(&self) -> String {
        track_id(&self.path)
    }
    /// Directory enumeration may skip a name with an unpaired code unit. Check the cached
    /// name itself before dropping it, including loose files passed in as native wide paths.
    #[cfg(windows)]
    fn selected_wide_path_exists(&self, roots: &[PathBuf], loose: &[PathBuf]) -> bool {
        use crate::data::text;
        use std::{
            ffi::OsString,
            os::windows::ffi::{OsStrExt, OsStringExt},
        };
        if !self.path.contains('\0') {
            return false;
        }
        let id = self.id();
        let key = |p: &PathBuf| match p.to_str() {
            Some(s) => track_id(s),
            None => track_id(&text::from_units(
                &p.as_os_str().encode_wide().collect::<Vec<_>>(),
            )),
        };
        if !loose.iter().any(|p| key(p) == id)
            && !roots.iter().any(|p| {
                id.strip_prefix(key(p).trim_end_matches(['\\', '/']))
                    .is_some_and(|suffix| suffix.starts_with(['\\', '/']))
            })
        {
            return false;
        }
        let path = OsString::from_wide(&text::units(&self.path));
        match std::fs::metadata(path) {
            Ok(meta) => meta.is_file(),
            Err(e) => e.kind() != io::ErrorKind::NotFound,
        }
    }
    #[cfg(not(windows))]
    fn selected_wide_path_exists(&self, _: &[PathBuf], _: &[PathBuf]) -> bool {
        false
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Library {
    pub v: u32,
    #[serde(
        default,
        deserialize_with = "saved_time",
        skip_serializing_if = "Option::is_none"
    )]
    pub saved: Option<i64>,
    #[serde(default, deserialize_with = "cached_tracks")]
    pub tracks: Vec<Track>,
    #[serde(flatten)]
    pub extra: Fields,
    #[serde(skip)]
    scanned: bool,
}

fn saved_time<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
    Ok(Value::deserialize(d)?.as_i64())
}

fn cached_tracks<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Track>, D::Error> {
    let rows = crate::data::array(Value::deserialize(d)?);
    let mut tracks = Vec::with_capacity(rows.len());
    for row in rows {
        let Some(path) = row["path"].as_str().filter(|s| !s.is_empty()) else {
            continue;
        };
        if let Ok(track) = Track::deserialize(&row) {
            tracks.push(track);
        } else {
            // Keep recognizable fields and extensions even if a drive is currently offline.
            // readError makes the next scan read this song again regardless of size and mtime.
            let mut repaired = serde_json::json!({"path":path});
            for (key, value) in crate::data::object(row) {
                let mut probe = serde_json::json!({"path":""});
                probe[&key] = value;
                if Track::deserialize(&probe).is_ok() {
                    repaired[&key] = probe[&key].take();
                }
            }
            repaired["readError"] = 1.into();
            if let Ok(track) = serde_json::from_value(repaired) {
                tracks.push(track);
            }
        }
    }
    Ok(tracks)
}
impl Default for Library {
    fn default() -> Self {
        Self {
            v: 1,
            saved: None,
            tracks: vec![],
            extra: Fields::new(),
            scanned: false,
        }
    }
}
#[derive(Debug)]
pub struct Issue {
    pub path: PathBuf,
    pub error: io::Error,
}
#[derive(Debug, Default)]
pub struct ScanReport {
    pub read: usize,
    pub reused: usize,
    pub removed: usize,
    pub covers_changed: usize,
    pub issues: Vec<Issue>,
}
impl ScanReport {
    pub fn changed(&self) -> bool {
        self.read + self.removed + self.covers_changed > 0
    }
}

impl Library {
    /// Build playback lookup from the shared cache and current display artist keys.
    /// Screens can order songs and supply album and artist groups in their displayed order.
    pub fn catalog(
        &self,
        state: &State,
        artist_key: impl Fn(&Track) -> String,
    ) -> crate::queue::Catalog {
        use crate::queue::{Catalog, Song};
        let mut catalog = Catalog::default();
        for t in &self.tracks {
            let id = t.id();
            let hidden = state.hidden(&id);
            if !hidden {
                catalog.songs.push(id.clone());
            }
            catalog.tracks.insert(
                id,
                Song {
                    artist_key: artist_key(t),
                    hidden,
                    unplayable: false,
                },
            );
        }
        catalog
    }
    /// Hidden tracks stay in the cache and remain addressable by saved playlists.
    pub fn visible<'a>(&'a self, state: &'a State) -> impl Iterator<Item = &'a Track> {
        self.tracks.iter().filter(|t| !state.hidden(&t.id()))
    }
}
