//! Reading the `Streaming_History_Audio_*.json` files of Spotify's extended streaming history export.
use chrono::{DateTime, Local, Utc};
use serde::Deserialize;
use std::{
    collections::HashSet,
    fs::{self, File},
    io::{self, BufReader, Read},
    path::{Path, PathBuf},
};
use string_core::{
    data::SpotifyHistory,
    library::{is_song_name, scan_enters},
    stats::Track,
};

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
    #[error("Your saved Spotify history's import record is damaged, so nothing was imported.")]
    Damaged,
    #[error(
        "That holds more than 5,000,000 plays, more than String can import, so nothing was imported."
    )]
    TooMany,
}

/// How many folders below a folder given the history search goes.
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
        collect(path, 0, &mut found, &mut seen).map_err(|e| named(path, e))?;
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
/// `depth`: how many folders below the path given `path` is.
fn collect(
    path: &Path,
    depth: usize,
    found: &mut Vec<Source>,
    seen: &mut HashSet<(PathBuf, String)>,
) -> io::Result<()> {
    if fs::metadata(path)?.is_dir() {
        if depth > DEPTH {
            return Ok(());
        }
        for entry in fs::read_dir(path)? {
            let child = entry?.path();
            collect(&child, depth + 1, found, seen).map_err(|e| named(&child, e))?;
        }
        return Ok(());
    }
    if is_zip(path) && depth == 0 {
        let mut archive = open_zip(path)?;
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
/// A zip whose files can be told apart: the archive keeps one file of any name, so a second file of the same name
/// would go unread, and its plays be passed for good.
fn open_zip(path: &Path) -> io::Result<zip::ZipArchive<BufReader<File>>> {
    let archive = zip::ZipArchive::new(BufReader::new(File::open(path)?)).map_err(zip_error)?;
    if archive.has_duplicate_names() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "it holds two files with the same name",
        ));
    }
    Ok(archive)
}

/// What a dropped path holds: a history file by `find_history`'s rules, and songs by the library scan's.
/// `unreadable`: part of what `find_history` would read can't be (a zip, a folder, or the path itself, gone since it
/// was dropped), so only the import can tell.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Holds {
    pub history: bool,
    pub music: bool,
    pub unreadable: bool,
}

/// Whether `path` is, or holds, a history file by `find_history`'s rules. It stops at the first one; see `holds`.
pub fn is_history(path: &Path) -> bool {
    look(path, false).history
}
/// Whether `path` is, or holds, a history file, looking as deep as `find_history` does, and whether it is or holds
/// songs, looking where the library scan does; it stops once both are known. It tells folders from files by the folder
/// listing, so a big music folder is walked without opening each file. Meant for a worker thread: it opens zips and
/// walks folders.
pub fn holds(path: &Path) -> Holds {
    look(path, true)
}
fn look(path: &Path, songs: bool) -> Holds {
    let mut found = Holds::default();
    match fs::metadata(path) {
        Ok(m) if m.is_dir() => look_in(path, 0, songs, songs, &mut found),
        Ok(_) if is_zip(path) => match zip_has_history(path) {
            Ok(history) => found.history = history,
            Err(_) => found.unreadable = true,
        },
        Ok(_) => {
            if let Some(name) = path.file_name() {
                note(&name.to_string_lossy(), true, songs, &mut found);
            }
        }
        // Gone since it was dropped counts too: it may come back, with its plays.
        Err(_) => found.unreadable = true,
    }
    found
}
fn note(name: &str, history: bool, songs: bool, found: &mut Holds) {
    if history && is_audio_name(name) {
        found.history = true;
    } else if songs && is_song_name(name) {
        found.music = true;
    }
}
fn known(want_songs: bool, found: Holds) -> bool {
    found.history && (found.music || !want_songs)
}
/// Lists `dir`, `depth` folders below the path given: for history while `find_history` would, and for songs while
/// `songs` (the scan goes there) and none are found yet.
fn look_in(dir: &Path, depth: usize, songs: bool, want_songs: bool, found: &mut Holds) {
    let history = depth <= DEPTH;
    let songs = songs && !found.music;
    if !history && !songs {
        return;
    }
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => {
            found.unreadable |= history;
            return;
        }
    };
    let mut folders = vec![];
    for entry in entries {
        let Ok(entry) = entry else {
            found.unreadable |= history;
            continue;
        };
        let Ok(kind) = entry.file_type() else {
            found.unreadable |= history;
            continue;
        };
        // A link is followed, as `find_history` and the scan follow it; `find_history` refuses one it can't follow.
        let folder = if kind.is_symlink() {
            match fs::metadata(entry.path()) {
                Ok(m) => m.is_dir(),
                Err(_) => {
                    found.unreadable |= history;
                    continue;
                }
            }
        } else {
            kind.is_dir()
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        if folder {
            let deeper = songs && u8::try_from(depth).is_ok_and(|d| scan_enters(&name, d));
            if depth < DEPTH || deeper {
                folders.push((entry.path(), deeper));
            }
        } else {
            note(&name, history, songs, found);
            if known(want_songs, *found) {
                return;
            }
        }
    }
    for (folder, songs) in folders {
        look_in(&folder, depth + 1, songs, want_songs, found);
        if known(want_songs, *found) {
            return;
        }
    }
}
fn zip_has_history(path: &Path) -> io::Result<bool> {
    let mut archive = open_zip(path)?;
    for i in 0..archive.len() {
        let entry = archive.by_index_raw(i).map_err(zip_error)?;
        if entry.is_file()
            && entry
                .name()
                .is_ok_and(|n| is_audio_name(n.rsplit(['/', '\\']).next().unwrap_or("")))
        {
            return Ok(true);
        }
    }
    Ok(false)
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

/// The rows of `source`, refused when it unpacks to more than `limit` bytes.
fn read_rows(source: &Source, limit: u64) -> Result<Vec<Row>, ImportError> {
    let unreadable = |reason: String| ImportError::Unreadable {
        file: source.file_name(),
        reason,
    };
    let too_big = || unreadable("it's far larger than a Spotify history file".into());
    let mut bytes = vec![];
    match source {
        Source::File(path) => {
            let file = File::open(path).map_err(|e| unreadable(e.to_string()))?;
            if file
                .metadata()
                .map_err(|e| unreadable(e.to_string()))?
                .len()
                > limit
            {
                return Err(too_big());
            }
            file.take(limit.saturating_add(1))
                .read_to_end(&mut bytes)
                .map_err(|e| unreadable(e.to_string()))?;
        }
        Source::ZipEntry { zip, name } => {
            let mut archive = open_zip(zip).map_err(|e| unreadable(e.to_string()))?;
            let entry = archive
                .by_name(name)
                .map_err(|e| unreadable(e.to_string()))?;
            if entry.size() > limit {
                return Err(too_big());
            }
            // The size a zip states can be false: what it unpacks to is counted too.
            entry
                .take(limit.saturating_add(1))
                .read_to_end(&mut bytes)
                .map_err(|e| unreadable(e.to_string()))?;
        }
    }
    if bytes.len() as u64 > limit {
        return Err(too_big());
    }
    serde_json::from_slice(&bytes).map_err(|e| unreadable(e.to_string()))
}

/// The most one import takes: a history file's size once unpacked, and the plays it adds. Spotify's files are about
/// 12 MB each, and years of listening are a few hundred thousand plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub file_bytes: u64,
    pub plays: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            file_bytes: 256 << 20,
            plays: 5_000_000,
        }
    }
}

/// Adds the plays newer than the history's cursor. Every source is parsed before `into` is touched, so a damaged
/// file leaves it as it was.
pub fn import(
    sources: &[Source],
    into: &mut SpotifyHistory,
    progress: impl FnMut(usize, usize),
) -> Result<Summary, ImportError> {
    import_within(sources, into, Limits::default(), progress)
}
/// `import`, refusing more than `limits` allow.
pub fn import_within(
    sources: &[Source],
    into: &mut SpotifyHistory,
    limits: Limits,
    mut progress: impl FnMut(usize, usize),
) -> Result<Summary, ImportError> {
    if sources.is_empty() {
        return Err(ImportError::NoHistory);
    }
    let meta = into.meta().ok_or(ImportError::Damaged)?;
    let mut plays = vec![];
    let mut skipped = 0u64;
    // Exports overlap, and a file can be reached twice; a play is the same play when all three match.
    let mut seen = HashSet::new();
    for (i, source) in sources.iter().enumerate() {
        for row in read_rows(source, limits.file_bytes)? {
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
            if plays.len() > limits.plays {
                return Err(ImportError::TooMany);
            }
        }
        progress(i + 1, sources.len());
    }
    let mut summary = Summary {
        files: sources.len(),
        through: meta.through,
        ..Summary::default()
    };
    // In the order they were played, so a song Spotify renamed keeps its newest name and album whatever order the
    // export's files list it in.
    plays.sort_by_key(|play| play.ms);
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
