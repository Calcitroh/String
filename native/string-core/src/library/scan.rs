//! Discover music and folder art, then refresh changed rows in parallel.

use super::{Issue, Library, ScanReport, Track, track_id};
use crate::tags;
use rayon::prelude::*;
use serde_json::json;
use std::{
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
fn path_string(p: &Path) -> io::Result<String> {
    p.to_str()
        .map(str::to_owned)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "path is not Unicode"))
}
#[cfg(windows)]
fn child(path: &Path, name: &str) -> PathBuf {
    PathBuf::from(format!(
        "{}\\{}",
        path.to_string_lossy().trim_end_matches(['\\', '/']),
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
                path: path_string(path)?,
                size: m.len(),
                mtime: millis(m.modified()?),
            });
        }
        Ok(())
    }
}
fn list(root: &Path, depth: u8) -> Listing {
    let mut out = Listing::default();
    let result = (|| -> io::Result<()> {
        for entry in fs::read_dir(root)? {
            let e = entry?;
            let name = e
                .file_name()
                .into_string()
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid filename"))?;
            let lower = name.to_lowercase();
            let path = child(root, &name);
            // A missed entry might be a directory. Keep cached descendants on a partial listing.
            let m = if e.file_type()?.is_symlink() {
                fs::metadata(&path)?
            } else {
                e.metadata()?
            };
            if m.is_dir() {
                if depth < 14
                    && !lower.starts_with(['.', '$'])
                    && !matches!(
                        lower.as_str(),
                        "node_modules" | "system volume information" | "__macosx"
                    )
                {
                    out.dirs.push(path);
                }
            } else if cover(&lower) {
                if out.cover.is_empty()
                    || ["cover.", "folder.", "front."]
                        .iter()
                        .any(|s| lower.starts_with(s))
                {
                    out.cover = path_string(&path)?;
                }
            } else if audio(&lower) {
                out.file(&path, &m)?;
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        out.unread.push(track_id(&root.to_string_lossy()));
        out.issues.push(Issue {
            path: root.into(),
            error,
        });
    }
    out
}

impl Library {
    pub fn scan(&mut self, roots: &[PathBuf], loose: &[PathBuf]) -> ScanReport {
        let mut report = ScanReport::default();
        let mut found = HashMap::new();
        let mut covers = HashMap::new();
        let mut unread = vec![];
        let mut keep = HashSet::new();
        let mut seen = HashSet::new();
        let mut dirs = roots.to_vec();
        for depth in 0..=14 {
            dirs.retain(|p| seen.insert(track_id(&p.to_string_lossy())));
            if dirs.is_empty() {
                break;
            }
            let batch: Vec<_> = dirs.par_iter().map(|root| list(root, depth)).collect();
            let mut next = vec![];
            for (root, l) in dirs.iter().zip(batch) {
                covers.insert(
                    track_id(root.to_string_lossy().trim_end_matches(['\\', '/'])),
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
            let mut l = Listing::default();
            match fs::metadata(path).and_then(|m| l.file(path, &m)) {
                Ok(()) => {
                    for f in l.files {
                        found.insert(track_id(&f.path), f);
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(error) => {
                    keep.insert(track_id(&path.to_string_lossy()));
                    report.issues.push(Issue {
                        path: path.clone(),
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
            } else if !keep.contains(&id) && !unread.iter().any(|r| id == *r || below(&id, r))
                && !t.selected_wide_path_exists(roots, loose) {
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
                match tags::read(Path::new(&t.path)) {
                    Ok(meta) => {
                        t.tags = meta.tags;
                        if meta.cover_mime.is_some() {
                            t.art = json!({"p":1});
                        }
                    }
                    Err(error) => {
                        t.read_error = 1;
                        issue = Some(Issue {
                            path: PathBuf::from(&t.path),
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
        if report.changed() {
            self.saved = Some(now);
        }
        report
    }
}
