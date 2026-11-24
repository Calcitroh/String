//! Reading the `Streaming_History_Audio_*.json` files of Spotify's extended streaming history export.
use chrono::{DateTime, Local, Utc};
use serde::Deserialize;
use std::{
    fs::{self, File},
    io::{self, BufReader, Read},
    path::{Path, PathBuf},
};
use string_core::{data::SpotifyHistory, stats::Track};

/// Where a history file lives: loose, or inside a zip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    File(PathBuf),
    ZipEntry { zip: PathBuf, name: String },
}
impl Source {
    fn file_name(&self) -> String {
        match self {
            Source::File(p) => p.file_name().map(|n| n.to_string_lossy().into_owned()),
            Source::ZipEntry { name, .. } => name.rsplit(['/', '\\']).next().map(str::to_owned),
        }
        .unwrap_or_default()
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Summary {
    pub added: u64,
    pub seconds: f64,
    /// The newest play the history now holds, in epoch milliseconds.
    pub through: i64,
    pub files: usize,
    /// Rows that are not songs, or are unusable.
    pub skipped: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("Couldn't read {file} ({reason}), so nothing was imported.")]
    Unreadable { file: String, reason: String },
    #[error("This export holds no plays newer than the ones already in Stats.")]
    NothingNew,
    #[error("That doesn't hold a Spotify listening history (Streaming_History_Audio files).")]
    NoHistory,
}

const DEPTH: usize = 4;

fn is_audio_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.starts_with("streaming_history_audio_") && name.ends_with(".json")
}
fn is_zip(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("zip"))
}

/// The history files in `paths`: the paths themselves, folders up to four levels down, and every entry of a zip
/// given directly. Sorted by file name. A path that cannot be read is an error only when nothing else was found.
pub fn find_history(paths: &[PathBuf]) -> io::Result<Vec<Source>> {
    let mut found = vec![];
    let mut first_error = None;
    for path in paths {
        if let Err(e) = collect(path, 1, &mut found) {
            first_error.get_or_insert(e);
        }
    }
    if found.is_empty()
        && let Some(e) = first_error
    {
        return Err(e);
    }
    found.sort_by_cached_key(|s| (s.file_name().to_lowercase(), format!("{s:?}")));
    Ok(found)
}
fn collect(path: &Path, level: usize, found: &mut Vec<Source>) -> io::Result<()> {
    if fs::metadata(path)?.is_dir() {
        if level > DEPTH {
            return Ok(());
        }
        let mut first_error = None;
        for entry in fs::read_dir(path)? {
            let result = entry.and_then(|e| collect(&e.path(), level + 1, found));
            if let Err(e) = result {
                first_error.get_or_insert(e);
            }
        }
        // One unreadable child must not hide the rest; find_history decides from what was found overall.
        return match first_error {
            Some(e) if found.is_empty() => Err(e),
            _ => Ok(()),
        };
    }
    if is_zip(path) && level == 1 {
        let mut archive =
            zip::ZipArchive::new(BufReader::new(File::open(path)?)).map_err(zip_error)?;
        for i in 0..archive.len() {
            let entry = archive.by_index_raw(i).map_err(zip_error)?;
            let Ok(name) = entry.name().map(|n| n.into_owned()) else {
                continue;
            };
            if entry.is_file() && is_audio_name(name.rsplit(['/', '\\']).next().unwrap_or("")) {
                found.push(Source::ZipEntry {
                    zip: path.to_owned(),
                    name,
                });
            }
        }
    } else if path
        .file_name()
        .is_some_and(|n| is_audio_name(&n.to_string_lossy()))
    {
        found.push(Source::File(path.to_owned()));
    }
    Ok(())
}
fn zip_error(e: zip::result::ZipError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e)
}

/// Meant for a worker thread: it opens zips and walks folders.
pub fn is_history(path: &Path) -> bool {
    find_history(&[path.to_owned()]).is_ok_and(|found| !found.is_empty())
}

/// Only what the app uses; serde skips every other field (addresses, countries, devices) without keeping it.
#[derive(Deserialize)]
struct Row {
    ts: Option<String>,
    ms_played: Option<f64>,
    master_metadata_track_name: Option<String>,
    master_metadata_album_artist_name: Option<String>,
    master_metadata_album_album_name: Option<String>,
    spotify_track_uri: Option<String>,
}

struct Play {
    ms: i64,
    local: chrono::NaiveDateTime,
    track: Track,
    album: String,
    secs: f64,
}

fn read_rows(source: &Source) -> Result<Vec<Row>, ImportError> {
    let unreadable = |reason: String| ImportError::Unreadable {
        file: source.file_name(),
        reason,
    };
    let mut bytes = vec![];
    match source {
        Source::File(path) => {
            File::open(path)
                .and_then(|mut f| f.read_to_end(&mut bytes))
                .map_err(|e| unreadable(e.to_string()))?;
        }
        Source::ZipEntry { zip, name } => {
            let mut archive = File::open(zip)
                .map_err(zip::result::ZipError::Io)
                .and_then(|f| zip::ZipArchive::new(BufReader::new(f)))
                .map_err(|e| unreadable(e.to_string()))?;
            let mut entry = archive
                .by_name(name)
                .map_err(|e| unreadable(e.to_string()))?;
            entry
                .read_to_end(&mut bytes)
                .map_err(|e| unreadable(e.to_string()))?;
        }
    }
    serde_json::from_slice(&bytes).map_err(|e| unreadable(e.to_string()))
}

/// Adds the plays newer than the history's cursor. Every source is parsed before `into` is touched, so a damaged
/// file leaves it as it was.
pub fn import(
    sources: &[Source],
    into: &mut SpotifyHistory,
    mut progress: impl FnMut(usize, usize),
) -> Result<Summary, ImportError> {
    if sources.is_empty() {
        return Err(ImportError::NoHistory);
    }
    let meta = into.meta();
    let mut plays = vec![];
    let mut skipped = 0u64;
    for (i, source) in sources.iter().enumerate() {
        for row in read_rows(source)? {
            let Some(at) = row
                .ts
                .as_deref()
                .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())
                .map(|t| t.with_timezone(&Utc))
            else {
                skipped += 1;
                continue;
            };
            let ms = at.timestamp_millis();
            if ms <= meta.through {
                continue;
            }
            let uri = row.spotify_track_uri.unwrap_or_default();
            let title = row.master_metadata_track_name.unwrap_or_default();
            let played = row.ms_played.unwrap_or(0.);
            if !(uri.starts_with("spotify:track:") || uri.starts_with("spotify:local:"))
                || title.is_empty()
                || played <= 0.
            {
                skipped += 1;
                continue;
            }
            plays.push(Play {
                ms,
                local: at.with_timezone(&Local).naive_local(),
                track: Track(
                    uri,
                    title,
                    row.master_metadata_album_artist_name.unwrap_or_default(),
                ),
                album: row.master_metadata_album_album_name.unwrap_or_default(),
                secs: played / 1000.,
            });
        }
        progress(i + 1, sources.len());
    }
    let mut summary = Summary {
        files: sources.len(),
        through: meta.through,
        ..Summary::default()
    };
    for play in &plays {
        if into
            .0
            .add_imported(&play.track, play.secs, play.local, &play.album)
        {
            summary.added += 1;
            summary.seconds += play.secs;
            summary.through = summary.through.max(play.ms);
        } else {
            skipped += 1;
        }
    }
    if summary.added == 0 {
        return Err(ImportError::NothingNew);
    }
    summary.skipped = skipped;
    let mut meta = meta;
    meta.through = summary.through;
    meta.plays += summary.added;
    meta.imported = Utc::now().timestamp_millis();
    meta.files = sources.len() as u64;
    into.set_meta(meta);
    Ok(summary)
}
