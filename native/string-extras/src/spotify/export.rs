//! Reading the `Streaming_History_Audio_*.json` files of Spotify's extended streaming history export.
use chrono::{DateTime, Local, Utc};
use serde::Deserialize;
use std::{
    collections::HashSet,
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
/// given directly. Sorted by file name, each file once however many ways it was reached. A path that cannot be
/// read is an error naming it: skipping it quietly would move the cursor past plays it holds.
pub fn find_history(paths: &[PathBuf]) -> io::Result<Vec<Source>> {
    let mut found = vec![];
    let mut seen = HashSet::new();
    for path in paths {
        collect(path, 1, &mut found, &mut seen).map_err(|e| named(path, e))?;
    }
    found.sort_by_cached_key(|s| (s.file_name().to_lowercase(), format!("{s:?}")));
    Ok(found)
}
fn named(path: &Path, e: io::Error) -> io::Error {
    let name = path
        .file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy();
    io::Error::new(e.kind(), format!("{name}: {e}"))
}
fn collect(
    path: &Path,
    level: usize,
    found: &mut Vec<Source>,
    seen: &mut HashSet<(PathBuf, String)>,
) -> io::Result<()> {
    if fs::metadata(path)?.is_dir() {
        if level > DEPTH {
            return Ok(());
        }
        for entry in fs::read_dir(path)? {
            let child = entry?.path();
            collect(&child, level + 1, found, seen).map_err(|e| named(&child, e))?;
        }
        return Ok(());
    }
    if is_zip(path) && level == 1 {
        let mut archive =
            zip::ZipArchive::new(BufReader::new(File::open(path)?)).map_err(zip_error)?;
        let real = fs::canonicalize(path)?;
        for i in 0..archive.len() {
            let entry = archive.by_index_raw(i).map_err(zip_error)?;
            let Ok(name) = entry.name().map(|n| n.into_owned()) else {
                continue;
            };
            if entry.is_file()
                && is_audio_name(name.rsplit(['/', '\\']).next().unwrap_or(""))
                && seen.insert((real.clone(), name.clone()))
            {
                found.push(Source::ZipEntry {
                    zip: path.to_owned(),
                    name,
                });
            }
        }
    } else if path
        .file_name()
        .is_some_and(|n| is_audio_name(&n.to_string_lossy()))
        && seen.insert((fs::canonicalize(path)?, String::new()))
    {
        found.push(Source::File(path.to_owned()));
    }
    Ok(())
}
fn zip_error(e: zip::result::ZipError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e)
}

/// Whether `path` is, or holds, a history file by `find_history`'s rules. It stops at the first one, and tells
/// folders from files by the folder listing, so a big music folder is walked without opening each file. What can't
/// be read counts as no history here; the import names it. Meant for a worker thread: it opens zips and walks folders.
pub fn is_history(path: &Path) -> bool {
    match fs::metadata(path) {
        Ok(m) if m.is_dir() => folder_has_history(path, 1),
        Ok(_) if is_zip(path) => zip_has_history(path),
        Ok(_) => path
            .file_name()
            .is_some_and(|n| is_audio_name(&n.to_string_lossy())),
        Err(_) => false,
    }
}
fn folder_has_history(dir: &Path, level: usize) -> bool {
    if level > DEPTH {
        return false;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    let mut folders = vec![];
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        // A link is followed, as `find_history` follows it.
        let folder = if kind.is_symlink() {
            fs::metadata(entry.path()).is_ok_and(|m| m.is_dir())
        } else {
            kind.is_dir()
        };
        if folder {
            folders.push(entry.path());
        } else if is_audio_name(&entry.file_name().to_string_lossy()) {
            return true;
        }
    }
    folders.iter().any(|f| folder_has_history(f, level + 1))
}
fn zip_has_history(path: &Path) -> bool {
    let Ok(mut archive) = File::open(path)
        .map_err(zip::result::ZipError::Io)
        .and_then(|f| zip::ZipArchive::new(BufReader::new(f)))
    else {
        return false;
    };
    (0..archive.len()).any(|i| {
        archive.by_index_raw(i).is_ok_and(|entry| {
            entry.is_file()
                && entry
                    .name()
                    .is_ok_and(|n| is_audio_name(n.rsplit(['/', '\\']).next().unwrap_or("")))
        })
    })
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
    // Exports overlap, and a file can be reached twice; a play is the same play when all three match.
    let mut seen = HashSet::new();
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
            if !seen.insert((ms, uri.clone(), played as i64)) {
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
