//! Discover music and folder art, then refresh changed rows in parallel.

#[cfg(windows)]
use super::native_path;
use super::{Issue, Library, ScanReport, Track, track_id};
use crate::tags;
use rayon::prelude::*;
use serde_json::json;
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

fn audio(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(_, e)| {
        matches!(
            e,
            "mp3"
                | "flac"
                | "m4a"
                | "m4b"
                | "aac"
                | "ogg"
                | "oga"
                | "opus"
                | "wav"
                | "weba"
                | "webm"
        )
    })
}
/// Whether a scan takes a file of this name as a song: by its extension, in any case.
pub fn is_song_name(name: &str) -> bool {
    audio(&name.to_lowercase())
}
fn cover(name: &str) -> bool {
    let Some((base, ext)) = name.rsplit_once('.') else {
        return false;
    };
    if !matches!(ext, "jpg" | "jpeg" | "png" | "webp") {
        return false;
    }
    if matches!(base, "cover" | "folder" | "front" | "album" | "art") {
        return true;
    }
    let Some(rest) = base.strip_prefix("albumart") else {
        return false;
    };
    let rest = rest
        .strip_suffix("large")
        .or_else(|| rest.strip_suffix("small"))
        .unwrap_or(rest);
    rest.is_empty()
        || rest
            .strip_prefix("_{")
            .and_then(|s| s.strip_suffix('}'))
            .is_some_and(|s| !s.contains('}'))
}
fn millis(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH).map_or_else(
        |e| -(e.duration().as_millis() as i64),
        |d| d.as_millis() as i64,
    )
}
fn path_string(path: &Path) -> io::Result<Cow<'_, str>> {
    if let Some(s) = path.to_str() {
        return Ok(Cow::Borrowed(s));
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let units: Vec<_> = path.as_os_str().encode_wide().collect();
        Ok(Cow::Owned(crate::data::text::from_units(&units)))
    }
    #[cfg(not(windows))]
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "path is not Unicode",
    ))
}
fn key_text(path: &Path) -> Cow<'_, str> {
    path_string(path).unwrap_or_else(|_| path.to_string_lossy())
}
// Folder and loose-file selections can come from either the profile or the OS.
fn scan_input(path: &Path) -> Cow<'_, Path> {
    #[cfg(windows)]
    if let Some(saved) = path.to_str().filter(|s| s.contains('\0')) {
        return Cow::Owned(native_path(saved));
    }
    Cow::Borrowed(path)
}
#[cfg(windows)]
fn child(path: &Path, name: &str) -> PathBuf {
    native_path(&format!(
        "{}\\{}",
        key_text(path).trim_end_matches(['\\', '/']),
        name
    ))
}
#[cfg(not(windows))]
fn child(path: &Path, name: &str) -> PathBuf {
    path.join(name)
}
fn dir(s: &str) -> &str {
    s.rsplit_once(['\\', '/']).map_or("", |(d, _)| d)
}
fn below(id: &str, root: &str) -> bool {
    id.strip_prefix(root.trim_end_matches(['\\', '/']))
        .is_some_and(|s| s.starts_with(['\\', '/']))
}

const MAX_DEPTH: u8 = 14;

fn traversable(name: &str) -> bool {
    !name.starts_with(['.', '$'])
        && !matches!(
            name,
            "node_modules" | "system volume information" | "__macosx"
        )
}
/// Whether a scan goes into a subfolder of this name, found in a folder `depth` folders below the one scanned.
pub fn scan_enters(name: &str, depth: u8) -> bool {
    depth < MAX_DEPTH && traversable(&name.to_lowercase())
}

struct Found {
    path: String,
    size: u64,
    mtime: i64,
}
#[derive(Default)]
struct Listing {
    dirs: Vec<PathBuf>,
    files: Vec<Found>,
    cover: String,
    issues: Vec<Issue>,
    unread: Vec<String>,
}
impl Listing {
    fn file(&mut self, path: &Path, m: &fs::Metadata) -> io::Result<()> {
        if m.is_file() && m.len() > 0 {
            self.files.push(Found {
                path: path_string(path)?.into_owned(),
                size: m.len(),
                mtime: millis(m.modified()?),
            });
        }
        Ok(())
    }
    fn entry(&mut self, entry: fs::DirEntry, path: &Path, name: &str, depth: u8) -> io::Result<()> {
        let kind = entry.file_type()?;
        let visit = depth < MAX_DEPTH && traversable(name);
        let m = if kind.is_symlink() {
            match fs::metadata(path) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    // A missing directory target leaves this part of the scan unfinished.
                    #[cfg(windows)]
                    let could_be_dir = std::os::windows::fs::FileTypeExt::is_symlink_dir(&kind);
                    #[cfg(not(windows))]
                    let could_be_dir = true; // The link itself does not record its target's type.
                    if could_be_dir && visit {
                        self.inaccessible(path, error);
                    }
                    return Ok(());
                }
                result => result?,
            }
        } else {
            entry.metadata()?
        };
        if m.is_dir() {
            if visit {
                self.dirs.push(path.to_owned());
            }
        } else if cover(name) {
            if self.cover.is_empty()
                || ["cover.", "folder.", "front."]
                    .iter()
                    .any(|s| name.starts_with(s))
            {
                self.cover = path_string(path)?.into_owned();
            }
        } else if audio(name) {
            self.file(path, &m)?;
        }
        Ok(())
    }
    fn inaccessible(&mut self, path: &Path, error: io::Error) {
        self.unread.push(track_id(&key_text(path)));
        self.issues.push(Issue {
            path: path.into(),
            error,
        });
    }
}
fn list(root: &Path, depth: u8) -> Listing {
    let mut out = Listing::default();
    let root = scan_input(root);
    let root = root.as_ref();
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) => {
            out.inaccessible(root, error);
            return out;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(error) => {
                // The missed name is unknown, so keep cached descendants of this folder.
                out.inaccessible(root, error);
                continue;
            }
        };
        let name = entry.file_name();
        let Ok(name) = path_string(Path::new(&name)) else {
            continue;
        };
        let path = child(root, &name);
        if let Err(error) = out.entry(entry, &path, &name.to_lowercase(), depth)
            && error.kind() != io::ErrorKind::NotFound
        {
            out.inaccessible(&path, error);
        }
    }
    out
}

impl Library {
    /// This folded folder ID and the parent root IDs from which a scan can enumerate it.
    /// Keep the spelling used by the scanner when it constructs a child's song ID.
    pub fn scan_ancestors(folder: &str) -> impl Iterator<Item = &str> {
        std::iter::successors(Some(folder.trim_end_matches(['\\', '/'])), |&p| {
            let (parent, name) = p.rsplit_once(std::path::MAIN_SEPARATOR)?;
            (!name.is_empty() && !(cfg!(windows) && name.contains('/')) && traversable(name))
                .then_some(parent)
        })
        .take(usize::from(MAX_DEPTH) + 1)
    }

    pub fn scan(&mut self, roots: &[PathBuf], loose: &[PathBuf]) -> ScanReport {
        let mut report = ScanReport::default();
        let mut found = HashMap::new();
        let mut covers = HashMap::new();
        let mut unread = vec![];
        let mut keep = HashSet::new();
        let mut seen = HashSet::new();
        let mut dirs = roots.to_vec();
        for depth in 0..=MAX_DEPTH {
            dirs.retain(|p| seen.insert(track_id(&key_text(p))));
            if dirs.is_empty() {
                break;
            }
            let batch: Vec<_> = dirs.par_iter().map(|root| list(root, depth)).collect();
            let mut next = vec![];
            for (root, l) in dirs.iter().zip(batch) {
                covers.insert(
                    track_id(key_text(root).trim_end_matches(['\\', '/'])),
                    l.cover,
                );
                next.extend(l.dirs);
                report.issues.extend(l.issues);
                unread.extend(l.unread);
                for f in l.files {
                    found.insert(track_id(&f.path), f);
                }
            }
            dirs = next;
        }
        for path in loose {
            let path = scan_input(path);
            let path = path.as_ref();
            let mut l = Listing::default();
            match fs::metadata(path).and_then(|m| l.file(path, &m)) {
                Ok(()) => {
                    for f in l.files {
                        found.insert(track_id(&f.path), f);
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(error) => {
                    keep.insert(track_id(&key_text(path)));
                    report.issues.push(Issue {
                        path: path.to_owned(),
                        error,
                    });
                }
            }
        }
        let now = millis(SystemTime::now());
        let first = !self.scanned && self.tracks.is_empty();
        self.scanned = true;
        let mut todo = vec![];
        let mut retained = Vec::with_capacity(self.tracks.len().max(found.len()));
        for mut t in std::mem::take(&mut self.tracks) {
            let id = t.id();
            if let Some(f) = found.remove(&id) {
                let cover = covers.get(dir(&id)).cloned().unwrap_or_default();
                if t.read_error == 0 && t.size == f.size && t.mtime == f.mtime {
                    report.reused += 1;
                    if t.cover != cover {
                        t.cover = cover;
                        report.covers_changed += 1;
                    }
                } else {
                    todo.push((f, cover, t.added, Some(retained.len())));
                }
            } else if !keep.contains(&id)
                && !unread.iter().any(|r| id == *r || below(&id, r))
                && !t.selected_wide_path_exists(roots, loose)
            {
                report.removed += 1;
                continue;
            }
            retained.push(t);
        }
        for (id, f) in found {
            let added = Some(if first { f.mtime } else { now }).filter(|&v| v != 0);
            todo.push((
                f,
                covers.get(dir(&id)).cloned().unwrap_or_default(),
                added,
                None,
            ));
        }
        todo.sort_unstable_by(|a, b| a.0.path.cmp(&b.0.path));
        report.read = todo.len();
        let rows: Vec<_> = todo
            .into_par_iter()
            .map(|(f, cover, added, slot)| {
                let mut t = Track {
                    path: f.path,
                    size: f.size,
                    mtime: f.mtime,
                    added,
                    cover,
                    ..Default::default()
                };
                let mut issue = None;
                let path = scan_input(Path::new(&t.path));
                match tags::read(&path) {
                    Ok(meta) => {
                        t.tags = meta.tags;
                        if meta.cover_mime.is_some() {
                            t.art = json!({"p":1});
                        }
                    }
                    Err(error) => {
                        t.read_error = 1;
                        issue = Some(Issue {
                            path: path.into_owned(),
                            error,
                        });
                    }
                }
                (slot, t, issue)
            })
            .collect();
        for (slot, mut t, issue) in rows {
            if let Some(i) = slot {
                t.extra = std::mem::take(&mut retained[i].extra);
                retained[i] = t;
            } else {
                retained.push(t);
            }
            report.issues.extend(issue);
        }
        self.tracks = retained;
        report.complete_folders = covers.into_keys().collect();
        for path in &unread {
            for parent in Path::new(path).ancestors() {
                report
                    .complete_folders
                    .remove(parent.to_string_lossy().trim_end_matches(['\\', '/']));
            }
        }
        if report.changed() {
            self.saved = Some(now);
        }
        report
    }
}

#[cfg(all(test, windows))]
#[test]
fn distinct_wide_folders_keep_their_tracks_and_covers() {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt};
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let wide = root.join(OsString::from_wide(&[0xd800]));
    let replacement = root.join("\u{fffd}");
    for folder in [&wide, &replacement] {
        fs::create_dir(folder).unwrap();
        fs::write(folder.join("song.webm"), [0x1a, 0x45, 0xdf, 0xa3]).unwrap();
        fs::write(folder.join("cover.png"), []).unwrap();
    }
    let mut library = Library::default();
    let cold = library.scan(std::slice::from_ref(&root), &[]);
    assert_eq!(cold.read, 2);
    assert!(cold.issues.is_empty());
    for track in &library.tracks {
        let song = native_path(&track.path);
        assert_eq!(native_path(&track.cover), song.with_file_name("cover.png"));
    }
    let saved_wide = root.join("\0\u{e000}");
    let warm = library.scan(std::slice::from_ref(&saved_wide), &[]);
    assert!(warm.issues.is_empty());
    assert_eq!((warm.read, warm.reused, warm.removed), (0, 1, 1));
    let id = library.tracks[0].id();
    let song = wide.join("song.webm");
    assert_eq!(native_path(&library.tracks[0].path), song);
    fs::write(
        song,
        include_bytes!("../../tests/fixtures/tag-scan/tone.flac"),
    )
    .unwrap();
    let changed = library.scan(&[], &[saved_wide.join("song.webm")]);
    assert_eq!(changed.read, 1);
    assert!(changed.issues.is_empty());
    assert_eq!(library.tracks[0].id(), id);
    assert_eq!(library.tracks[0].tags.year, 2009);
}
