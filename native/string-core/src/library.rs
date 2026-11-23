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
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Library {
    pub v: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saved: Option<i64>,
    pub tracks: Vec<Track>,
    #[serde(flatten)]
    pub extra: Fields,
    #[serde(skip)]
    scanned: bool,
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
