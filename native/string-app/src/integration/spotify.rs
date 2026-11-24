//! The imported Spotify history, read and matched to the library off the UI thread.

use crossbeam_channel::{Receiver, bounded};
use egui::Context;
use std::{
    hash::{DefaultHasher, Hash, Hasher},
    io, thread,
};
use string_core::{
    data::{DataFolder, Document, SpotifyHistory},
    stats::{Listening, Track},
};
use string_extras::spotify::{Matcher, Resolved};

/// `spotify.json` as read and matched to the library: what `Session::set_spotify` takes. The default is no layer.
#[derive(Default)]
pub struct SpotifyLayer {
    pub doc: Option<Document<SpotifyHistory>>,
    /// Why `spotify.json` couldn't be read.
    pub error: Option<String>,
    /// The history matched to the songs `songs` fingerprints; `None` when it holds no plays.
    pub view: Option<(Listening, Resolved)>,
    pub songs: u64,
}
impl SpotifyLayer {
    /// `doc` matched to `songs`, the library's from `Session::spotify_songs`.
    pub fn matched(doc: Document<SpotifyHistory>, songs: Vec<Track>) -> Self {
        let fingerprint = fingerprint(&songs);
        Self {
            view: resolve(&doc.value.0, songs),
            doc: Some(doc),
            error: None,
            songs: fingerprint,
        }
    }
    /// Reads `spotify.json` and matches it to `songs`. A missing file is no layer.
    pub fn read(folder: &DataFolder, songs: Vec<Track>) -> Self {
        match folder.read::<SpotifyHistory>() {
            Ok(doc) if doc.exists() => Self::matched(doc, songs),
            Ok(_) => Self::default(),
            Err(e) => Self {
                error: Some(format!("Couldn't read your Spotify history: {e}")),
                ..Self::default()
            },
        }
    }
}

/// Reads and matches `spotify.json` on a worker; the window repaints when it's in.
pub fn load(
    folder: DataFolder,
    songs: Vec<Track>,
    ctx: &Context,
) -> io::Result<Receiver<SpotifyLayer>> {
    let (tx, rx) = bounded(1);
    let ctx = ctx.clone();
    thread::Builder::new()
        .name("spotify history".into())
        .spawn(move || {
            let _ = tx.send(SpotifyLayer::read(&folder, songs));
            ctx.request_repaint();
        })?;
    Ok(rx)
}

pub(crate) fn resolve(history: &Listening, songs: Vec<Track>) -> Option<(Listening, Resolved)> {
    history
        .first_day()
        .is_some()
        .then(|| Matcher::new(songs).resolve(history))
}

/// Changes when any song's id, title or artist changes, or their order does.
pub(crate) fn fingerprint<'a>(songs: impl IntoIterator<Item = &'a Track>) -> u64 {
    fingerprint_names(songs.into_iter().map(|t| [&*t.0, &*t.1, &*t.2]))
}
pub(crate) fn fingerprint_names<'a>(songs: impl Iterator<Item = [&'a str; 3]>) -> u64 {
    let mut hasher = DefaultHasher::new();
    for names in songs {
        names.hash(&mut hasher);
    }
    hasher.finish()
}
