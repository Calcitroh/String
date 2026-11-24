//! The imported Spotify history, off the UI thread: read and matched to the library at startup, imported from an
//! export dropped on Stats or chosen there, and set aside again. Also where dropped files go: Stats or the library.

use crate::{add_work, playback::Session, ui::Toast};
use chrono::{TimeDelta, Utc};
use crossbeam_channel::{Receiver, Sender, TryRecvError, bounded, unbounded};
use egui::Context;
use serde_json::{Value, json};
use std::{
    fs::File,
    hash::{DefaultHasher, Hash, Hasher},
    io::{self, Write},
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use string_core::{
    data::{DataFolder, Document, SpotifyHistory},
    stats::Listening,
};
use string_extras::spotify::{
    self as export, Holds, ImportError, LibrarySong, Matcher, Resolved, Summary,
};

/// Said when an import or a removal is asked for while one runs: the panel shows the one running.
pub const BUSY: &str = "String is still busy with your Spotify history.";
/// Said when a Spotify history is dropped on another page.
pub const ELSEWHERE: &str = "That's a Spotify history: drop it on Stats";
const REMOVED: &str = "Your Spotify history is out of Stats. A copy is in String Backup.";
const FILE: &str = "spotify.json";

/// `spotify.json` as read and matched to the library: what `Session::set_spotify` takes. The default is no layer;
/// `matched`, `read` and `failed` make the others.
#[derive(Default)]
pub struct SpotifyLayer {
    doc: Option<Document<SpotifyHistory>>,
    /// Why `spotify.json` couldn't be read.
    error: Option<String>,
    /// The history matched to the songs `songs` fingerprints; `None` when it holds no plays.
    view: Option<(Listening, Resolved)>,
    songs: u64,
}
/// A layer's parts, as the session keeps them.
pub(crate) type LayerParts = (
    Option<Document<SpotifyHistory>>,
    Option<String>,
    Option<(Listening, Resolved)>,
    u64,
);
impl SpotifyLayer {
    /// `doc` matched to `songs`, the library's from `Session::spotify_songs`.
    pub fn matched(doc: Document<SpotifyHistory>, songs: Vec<LibrarySong>) -> Self {
        let fingerprint = fingerprint(&songs);
        Self {
            view: resolve(&doc.value.0, songs),
            doc: Some(doc),
            error: None,
            songs: fingerprint,
        }
    }
    /// Reads `spotify.json` and matches it to `songs`. A missing file is no layer.
    pub fn read(folder: &DataFolder, songs: Vec<LibrarySong>) -> Self {
        match folder.read::<SpotifyHistory>() {
            Ok(doc) if doc.exists() => Self::matched(doc, songs),
            Ok(_) => Self::default(),
            Err(e) => Self::failed(format!(
                "Couldn't read your Spotify history ({FILE}: {}).",
                reason(&e)
            )),
        }
    }
    /// No history, only why there isn't one.
    pub(crate) fn failed(error: String) -> Self {
        Self {
            error: Some(error),
            ..Self::default()
        }
    }
    pub(crate) fn into_parts(self) -> LayerParts {
        (self.doc, self.error, self.view, self.songs)
    }
    /// The part of the history's seconds that is songs in the library.
    fn matched_share(&self) -> f64 {
        self.view.as_ref().map_or(0., |(_, r)| {
            r.matched_seconds / r.seconds.max(f64::MIN_POSITIVE)
        })
    }
}

/// Repaints the window when a worker ends, a panic included, so no page waits on a worker that's gone.
struct Wake(Context);
impl Drop for Wake {
    fn drop(&mut self) {
        self.0.request_repaint();
    }
}

/// Reads and matches `spotify.json` on a worker; the window repaints when it's in.
pub fn load(
    folder: DataFolder,
    songs: Vec<LibrarySong>,
    ctx: &Context,
) -> io::Result<Receiver<SpotifyLayer>> {
    let (tx, rx) = bounded(1);
    let wake = Wake(ctx.clone());
    thread::Builder::new()
        .name("spotify history".into())
        .spawn(move || {
            let _wake = wake;
            let _ = tx.send(SpotifyLayer::read(&folder, songs));
        })?;
    Ok(rx)
}

pub(crate) fn resolve(
    history: &Listening,
    songs: Vec<LibrarySong>,
) -> Option<(Listening, Resolved)> {
    history
        .first_day()
        .is_some()
        .then(|| Matcher::new(songs).resolve(history))
}

/// Changes when any song's id, title, artist or album changes, or their order does.
pub(crate) fn fingerprint<'a>(songs: impl IntoIterator<Item = &'a LibrarySong>) -> u64 {
    fingerprint_names(
        songs
            .into_iter()
            .map(|s| [&*s.track.0, &*s.track.1, &*s.track.2, &*s.album]),
    )
}
pub(crate) fn fingerprint_names<'a>(songs: impl Iterator<Item = [&'a str; 4]>) -> u64 {
    let mut hasher = DefaultHasher::new();
    for names in songs {
        names.hash(&mut hasher);
    }
    hasher.finish()
}

/* ---------- importing and removing ---------- */

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobKind {
    Import,
    Remove,
}
/// An import or a removal on a thread of its own. The session keeps one at a time.
pub struct Job {
    kind: JobKind,
    events: Receiver<Event>,
    progress: Option<(usize, usize)>,
}
enum Event {
    Progress(usize, usize),
    Imported(Result<Imported, String>),
    /// Whether there was a file to set aside.
    Removed(Result<bool, String>),
}
struct Imported {
    layer: SpotifyLayer,
    summary: Summary,
    /// No `spotify.json` before this import.
    first: bool,
    /// Saved, but its copy in the day's backups wasn't.
    backup_failed: bool,
}
/// How an import or a removal ended.
pub struct Outcome {
    /// What the toast says.
    pub message: String,
    pub failed: bool,
    /// What an import added.
    pub summary: Option<Summary>,
}
impl Job {
    pub fn kind(&self) -> JobKind {
        self.kind
    }
    /// Files read so far and how many there are, once the import has found them.
    pub fn progress(&self) -> Option<(usize, usize)> {
        self.progress
    }
    /// Takes in the worker's news. Once it has ended: how, and the layer to show from now on (`None` keeps the one
    /// shown).
    pub(crate) fn poll(&mut self) -> Option<(Outcome, Option<SpotifyLayer>)> {
        let (message, failed, summary, layer) = loop {
            match self.events.try_recv() {
                Ok(Event::Progress(done, total)) => self.progress = Some((done, total)),
                Err(TryRecvError::Empty) => return None,
                Ok(Event::Imported(Ok(i))) => {
                    let s = &i.summary;
                    let mut message =
                        import_message(i.first, s.added, s.seconds, i.layer.matched_share());
                    if i.backup_failed {
                        message.push_str(&format!(
                            " · The daily backup of {FILE} couldn't be written"
                        ));
                    }
                    break (message, false, Some(i.summary), Some(i.layer));
                }
                Ok(Event::Imported(Err(why)) | Event::Removed(Err(why))) => {
                    break (why, true, None, None);
                }
                Ok(Event::Removed(Ok(moved))) => {
                    let message = if moved {
                        REMOVED
                    } else {
                        "There was no Spotify history to take out of Stats."
                    };
                    break (message.into(), false, None, Some(SpotifyLayer::default()));
                }
                Err(TryRecvError::Disconnected) => {
                    let message =
                        "String stopped working on your Spotify history before it was done.";
                    break (message.into(), true, None, None);
                }
            }
        };
        let outcome = Outcome {
            message,
            failed,
            summary,
        };
        Some((outcome, layer))
    }
}
fn spawn_job(
    kind: JobKind,
    ctx: &Context,
    work: impl FnOnce(&dyn Fn(usize, usize)) -> Event + Send + 'static,
) -> io::Result<Job> {
    let (tx, events) = unbounded();
    let wake = Wake(ctx.clone());
    let name = match kind {
        JobKind::Import => "spotify import",
        JobKind::Remove => "spotify removal",
    };
    thread::Builder::new().name(name.into()).spawn(move || {
        let wake = wake;
        let progress = |done, total| {
            let _ = tx.send(Event::Progress(done, total));
            wake.0.request_repaint();
        };
        let _ = tx.send(work(&progress));
    })?;
    Ok(Job {
        kind,
        events,
        progress: None,
    })
}
/// Imports the history in `paths` into `spotify.json`, read afresh, and matches it to `songs`.
pub(crate) fn start_import(
    folder: DataFolder,
    paths: Vec<PathBuf>,
    songs: Vec<LibrarySong>,
    ctx: &Context,
) -> io::Result<Job> {
    spawn_job(JobKind::Import, ctx, move |progress| {
        Event::Imported(import(&folder, &paths, songs, progress))
    })
}
fn import(
    folder: &DataFolder,
    paths: &[PathBuf],
    songs: Vec<LibrarySong>,
    progress: &dyn Fn(usize, usize),
) -> Result<Imported, String> {
    let sources = export::find_history(paths).map_err(|e| {
        let text = e.to_string();
        let (file, why) = text.split_once(": ").unwrap_or((&text, "unreadable"));
        format!("Couldn't read {file} ({why}), so nothing was imported.")
    })?;
    if sources.is_empty() {
        return Err(ImportError::NoHistory.to_string());
    }
    let mut doc = folder.read::<SpotifyHistory>().map_err(|e| {
        format!(
            "Couldn't read {FILE} ({}), so nothing was imported.",
            reason(&e)
        )
    })?;
    let first = !doc.exists();
    progress(0, sources.len());
    let summary = export::import(&sources, &mut doc.value, progress).map_err(|e| e.to_string())?;
    let saved = doc.save().map_err(|e| {
        format!(
            "Couldn't save {FILE} ({}), so nothing was imported.",
            reason(&e)
        )
    })?;
    Ok(Imported {
        layer: SpotifyLayer::matched(doc, songs),
        summary,
        first,
        backup_failed: saved.backup_error.is_some(),
    })
}
/// Moves `spotify.json` into the backup folder as `spotify-removed-<stamp>.json`.
pub(crate) fn start_remove(folder: DataFolder, stamp: String, ctx: &Context) -> io::Result<Job> {
    spawn_job(JobKind::Remove, ctx, move |_| {
        Event::Removed(
            folder
                .set_aside::<SpotifyHistory>(&stamp)
                .map(|moved| moved.is_some())
                .map_err(|e| {
                    format!(
                        "Couldn't take your Spotify history out of Stats ({}).",
                        reason(&e)
                    )
                }),
        )
    })
}
/// An error's words without the data folder's path, which holds the person's user name.
fn reason(e: &io::Error) -> String {
    let text = e.to_string();
    let after = text.rfind(FILE).map_or(&text[..], |at| {
        text[at + FILE.len()..].trim_start_matches([':', ' '])
    });
    after
        .split(" at path ")
        .next()
        .unwrap_or(after)
        .trim()
        .to_owned()
}

/// 166474 as "166,474".
pub(crate) fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}
/// Whole hours, grouped: "4,212", or "< 1" for less than half an hour.
pub(crate) fn hours(seconds: f64) -> String {
    let h = (seconds / 3600.).round() as u64;
    if h == 0 && seconds > 0. {
        "< 1".into()
    } else {
        grouped(h)
    }
}
pub(crate) fn percent(share: f64) -> u64 {
    (share.clamp(0., 1.) * 100.).round() as u64
}
pub(crate) fn import_message(first: bool, added: u64, seconds: f64, share: f64) -> String {
    let s = if added == 1 { "" } else { "s" };
    if first {
        format!(
            "Added {} play{s} ({} h) from Spotify · {}% of that time is songs in your library",
            grouped(added),
            hours(seconds),
            percent(share)
        )
    } else {
        format!("Added {} new play{s}", grouped(added))
    }
}

/* ---------- drops ---------- */

/// Where dropped files go. Songs, and anything that is neither songs nor a Spotify history, go to the library as
/// before; a history goes to the import on Stats (with anything there that can't be read), and elsewhere a history
/// alone gets a word about Stats. Telling them apart opens zips and walks folders, so it runs on a thread of its own;
/// the library's share goes on to the library's add worker from there, in the order dropped.
pub struct Drops {
    classify: Arc<Mutex<Box<Check>>>,
    worker: Option<(Sender<Sorting>, Receiver<Sorted>)>,
    /// Drops sent and not yet taken in.
    pending: usize,
    /// Drops the thread hasn't handed on yet: a close waits for them.
    sorting: Arc<AtomicUsize>,
    wake: Option<Context>,
}
type Check = dyn Fn(&Path) -> Holds + Send;
struct Sorting {
    paths: Vec<PathBuf>,
    on_stats: bool,
    library: add_work::Input,
}
struct Sorted {
    /// What to import: the paths that hold a history or can't be read, when dropped on Stats.
    history: Vec<PathBuf>,
    /// A history alone, all of it readable, was dropped on another page.
    elsewhere: bool,
    /// The library couldn't take its share.
    refused: bool,
}
impl Default for Drops {
    fn default() -> Self {
        // A path is a folder, or a song, by what it is on disk: a folder can be named like a song.
        Self::with_classifier(export::holds)
    }
}
impl Drops {
    pub(crate) fn with_classifier(classify: impl Fn(&Path) -> Holds + Send + 'static) -> Self {
        Self {
            classify: Arc::new(Mutex::new(Box::new(classify))),
            worker: None,
            pending: 0,
            sorting: Default::default(),
            wake: None,
        }
    }
    /// The window to wake when a drop is sorted.
    pub(crate) fn attach(&mut self, ctx: &Context) {
        if self.wake.is_none() {
            self.wake = Some(ctx.clone());
        }
    }
    /// Sends dropped paths off to be sorted; false when they couldn't go anywhere.
    pub(crate) fn send(
        &mut self,
        paths: Vec<PathBuf>,
        on_stats: bool,
        library: &add_work::Input,
    ) -> bool {
        if library.closed() {
            return false;
        }
        let mut job = Sorting {
            paths,
            on_stats,
            library: library.clone(),
        };
        if self.worker.is_none() {
            self.worker = self.spawn().ok();
        }
        if let Some((tx, _)) = &self.worker {
            self.sorting.fetch_add(1, Ordering::SeqCst);
            match tx.send(job) {
                Ok(()) => {
                    self.pending += 1;
                    return true;
                }
                Err(e) => job = e.0,
            }
        }
        // Without the sorting thread, the drop goes to the library as it did before Stats took Spotify histories.
        self.worker = None;
        self.pending = 0;
        self.sorting.store(0, Ordering::SeqCst);
        job.library.queue_drop(job.paths)
    }
    fn spawn(&self) -> io::Result<(Sender<Sorting>, Receiver<Sorted>)> {
        let (tx, inbox) = unbounded::<Sorting>();
        let (done, rx) = unbounded();
        let classify = self.classify.clone();
        let sorting = self.sorting.clone();
        let wake = self.wake.clone();
        thread::Builder::new()
            .name("drop sorting".into())
            .spawn(move || {
                for job in inbox {
                    let check = classify.lock().unwrap_or_else(PoisonError::into_inner);
                    let mut sorted = Sorted {
                        history: vec![],
                        elsewhere: false,
                        refused: false,
                    };
                    let mut library = vec![];
                    for path in job.paths {
                        // A check that panics leaves the path to the library, as before.
                        let holds =
                            catch_unwind(AssertUnwindSafe(|| check(&path))).unwrap_or_default();
                        // What can't be read goes to the import too, which refuses it by name: imported without
                        // it, the history would move past the plays it may hold.
                        let import = holds.history || holds.unreadable;
                        if job.on_stats {
                            if import {
                                sorted.history.push(path.clone());
                            }
                            if holds.music || !import {
                                library.push(path);
                            }
                        } else if holds.history && !holds.music && !holds.unreadable {
                            sorted.elsewhere = true;
                        } else {
                            library.push(path);
                        }
                    }
                    drop(check);
                    sorted.refused = !library.is_empty() && !job.library.queue_sorted_drop(library);
                    sorting.fetch_sub(1, Ordering::SeqCst);
                    if done.send(sorted).is_err() {
                        break;
                    }
                    if let Some(ctx) = &wake {
                        ctx.request_repaint();
                    }
                }
            })?;
        Ok((tx, rx))
    }
    pub(crate) fn pending(&self) -> bool {
        self.pending > 0
    }
    /// A drop is still being sorted: its songs aren't with the library yet.
    pub(crate) fn sorting(&self) -> bool {
        self.sorting.load(Ordering::SeqCst) > 0
    }
    /// Waits until no drop is being sorted, or until `deadline`.
    pub(crate) fn wait_sorted(&self, deadline: Instant) {
        while self.sorting() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(1));
        }
    }
    /// Takes in sorted drops: a history dropped on Stats starts the import, one dropped elsewhere says where it goes.
    pub(crate) fn poll(&mut self, s: &mut Session, toast: &mut Toast) {
        while let Some((_, rx)) = &self.worker {
            let sorted = match rx.try_recv() {
                Ok(sorted) => sorted,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.worker = None;
                    self.pending = 0;
                    self.sorting.store(0, Ordering::SeqCst);
                    break;
                }
            };
            self.pending -= 1;
            if sorted.refused {
                toast.set("String could not take more songs.");
            }
            if sorted.elsewhere {
                toast.set(ELSEWHERE);
            }
            if !sorted.history.is_empty()
                && let Err(why) = s.import_spotify(sorted.history)
            {
                toast.set(why);
            }
        }
    }
}

/* ---------- a synthetic export, for the self-test and the tests ---------- */

/// `plays` made-up plays over the last few days, in two `Streaming_History_Audio_*.json` files as Spotify writes
/// them, personal fields included (with made-up values, which the import never keeps). The first plays are of
/// `like`'s (title, artist), so they match those songs.
pub(crate) fn sample_files(plays: usize, like: &[(String, String)]) -> Vec<(String, Vec<Value>)> {
    let now = Utc::now();
    let rows: Vec<Value> = (0..plays)
        .map(|n| {
            let song = n % 17;
            let (uri, (title, artist)) = match like.get(n) {
                Some(names) => (format!("spotify:track:library{n}"), names.clone()),
                None => (
                    format!("spotify:track:sample{song}"),
                    (
                        format!("Sample Song {}", song + 1),
                        format!("Sample Artist {}", song % 5 + 1),
                    ),
                ),
            };
            let at = now - TimeDelta::minutes(90 * (plays - n) as i64 + 7);
            json!({
                "ts": at.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
                "username": "sample-listener",
                "platform": "sample device",
                "ms_played": 60_000 + (n as i64 * 37_000) % 180_000,
                "conn_country": "ZZ",
                "ip_addr": "192.0.2.1",
                "master_metadata_track_name": title,
                "master_metadata_album_artist_name": artist,
                "master_metadata_album_album_name": "Sample Album",
                "spotify_track_uri": uri,
                "reason_start": "trackdone",
                "shuffle": false,
                "offline": false,
                "incognito_mode": false,
            })
        })
        .collect();
    let half = rows.len().div_ceil(2);
    let year = now.format("%Y");
    vec![
        (
            format!("Streaming_History_Audio_{year}_0.json"),
            rows[..half].to_vec(),
        ),
        (
            format!("Streaming_History_Audio_{year}_1.json"),
            rows[half..].to_vec(),
        ),
    ]
}
/// A zip laid out as Spotify's download: the history files in a folder.
pub(crate) fn write_export(path: &Path, files: &[(String, Vec<Value>)]) -> io::Result<()> {
    let mut zip = zip::ZipWriter::new(File::create(path)?);
    for (name, rows) in files {
        zip.start_file(
            format!("Spotify Extended Streaming History/{name}"),
            zip::write::SimpleFileOptions::default(),
        )
        .map_err(io::Error::other)?;
        zip.write_all(&serde_json::to_vec(rows)?)?;
    }
    zip.finish().map_err(io::Error::other)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        playback::Session,
        storage::{self, Storage},
    };
    use chrono::NaiveDateTime;
    use std::{
        fs,
        time::{Duration, Instant},
    };
    use string_core::stats::Track;

    fn open(dir: &Path) -> Session {
        let (_store, s) = Storage::open(&storage::folder(dir, true), &Context::default()).unwrap();
        s
    }
    fn export(dir: &Path) -> PathBuf {
        let path = dir.join("my_spotify_data.zip");
        write_export(&path, &sample_files(2, &[])).unwrap();
        path
    }
    /// Runs the session's frame update until the read at open is in and a running import or removal has ended;
    /// returns what it said.
    fn settle(s: &mut Session) -> String {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut said = None;
        while said.is_none() || s.spotify_loading() {
            assert!(Instant::now() < deadline, "the Spotify work never ended");
            std::thread::sleep(Duration::from_millis(2));
            said = said.or(s.poll_spotify());
        }
        said.unwrap()
    }

    #[test]
    fn remove_sets_spotify_json_aside_and_stats_forget_it() {
        let dir = tempfile::tempdir().unwrap();
        // The person's own listening, which neither the import nor Remove may touch.
        let mut own = storage::folder(dir.path(), true)
            .read::<Listening>()
            .unwrap();
        let song = Track("own".into(), "Own Song".into(), "Own Band".into());
        own.value
            .add(&song, 30., chrono::Local::now().naive_local());
        own.save().unwrap();
        let listening = dir.path().join("listening.json");
        let before = fs::read(&listening).unwrap();
        let mut s = open(dir.path());
        s.import_spotify(vec![export(dir.path())]).unwrap();
        settle(&mut s);
        let file = dir.path().join("spotify.json");
        let saved = fs::read(&file).unwrap();
        assert!(s.spotify_view().is_some());
        let revision = s.spotify_revision;

        s.remove_spotify().unwrap();
        assert_eq!(
            settle(&mut s),
            "Your Spotify history is out of Stats. A copy is in String Backup."
        );
        assert!(!file.exists());
        let aside: Vec<_> = fs::read_dir(dir.path().join("backup"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.to_string_lossy().contains("spotify-removed-"))
            .collect();
        assert_eq!(aside.len(), 1);
        let name = aside[0].file_name().unwrap().to_string_lossy().into_owned();
        let stamp = name
            .strip_prefix("spotify-removed-")
            .and_then(|n| n.strip_suffix(".json"))
            .unwrap();
        assert!(
            NaiveDateTime::parse_from_str(stamp, "%Y-%m-%d %H%M%S").is_ok(),
            "{name}"
        );
        assert_eq!(fs::read(&aside[0]).unwrap(), saved);
        assert!(s.spotify.is_none() && s.spotify_error.is_none());
        assert!(s.spotify_view().is_none() && s.spotify_summary().is_none());
        assert_ne!(s.spotify_revision, revision);
        // An import afterwards starts again from nothing.
        s.import_spotify(vec![export(dir.path())]).unwrap();
        assert!(settle(&mut s).starts_with("Added 2 plays ("));
        assert_eq!(fs::read(&listening).unwrap(), before);
    }

    #[test]
    fn an_unreadable_export_is_named_and_nothing_is_imported() {
        let dir = tempfile::tempdir().unwrap();
        let zip = dir.path().join("my_spotify_data.zip");
        fs::write(&zip, b"not a zip").unwrap();
        let mut s = open(dir.path());
        s.import_spotify(vec![zip]).unwrap();
        let said = settle(&mut s);
        assert!(
            said.starts_with("Couldn't read my_spotify_data.zip ("),
            "{said}"
        );
        assert!(said.ends_with("), so nothing was imported."), "{said}");
        assert!(!dir.path().join("spotify.json").exists());
    }

    #[test]
    fn an_unreadable_spotify_json_refuses_import_and_stays_untouched() {
        for broken in [&b"{\"v\":1,\"days\":{"[..], br#"{"v":2,"days":{}}"#] {
            let dir = tempfile::tempdir().unwrap();
            let file = dir.path().join("spotify.json");
            fs::write(&file, broken).unwrap();
            let mut s = open(dir.path());
            s.import_spotify(vec![export(dir.path())]).unwrap();
            let said = settle(&mut s);
            assert!(said.starts_with("Couldn't read spotify.json ("), "{said}");
            assert!(said.ends_with("), so nothing was imported."), "{said}");
            let outcome = s.spotify_outcome.as_ref().unwrap();
            assert!(outcome.failed && outcome.summary.is_none());
            assert_eq!(fs::read(&file).unwrap(), broken);
            assert!(s.spotify.is_none() && s.spotify_view().is_none());
            // Neither message names the data folder: its path holds the person's user name.
            let error = s.spotify_error.clone().unwrap();
            let folder = dir.path().to_string_lossy().into_owned();
            assert!(
                !said.contains(&folder) && !error.contains(&folder),
                "{error}"
            );
        }
    }

    #[test]
    fn one_import_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = open(dir.path());
        let zip = export(dir.path());
        s.import_spotify(vec![zip.clone()]).unwrap();
        assert!(s.spotify_busy());
        assert_eq!(s.import_spotify(vec![zip.clone()]), Err(BUSY.into()));
        assert_eq!(s.remove_spotify(), Err(BUSY.into()));
        assert_eq!(
            settle(&mut s),
            "Added 2 plays (< 1 h) from Spotify · 0% of that time is songs in your library"
        );
        assert!(!s.spotify_busy());
        assert_eq!(s.spotify_summary().unwrap().plays, 2);
        s.import_spotify(vec![zip]).unwrap();
        assert_eq!(
            settle(&mut s),
            "This export holds no plays newer than the ones already in Stats."
        );
        assert_eq!(s.spotify_summary().unwrap().plays, 2);
    }

    #[test]
    fn the_import_toast_says_what_came_in() {
        assert_eq!(
            import_message(true, 166_474, 4212. * 3600., 0.61),
            "Added 166,474 plays (4,212 h) from Spotify · 61% of that time is songs in your library"
        );
        assert_eq!(
            import_message(false, 1204, 9000., 0.5),
            "Added 1,204 new plays"
        );
        assert_eq!(import_message(false, 1, 90., 0.), "Added 1 new play");
        assert_eq!(
            import_message(true, 1, 90., 1.),
            "Added 1 play (< 1 h) from Spotify · 100% of that time is songs in your library"
        );
    }

    #[test]
    fn a_daily_backup_that_cant_be_written_is_said_after_the_import() {
        let dir = tempfile::tempdir().unwrap();
        // A file where the backup folder goes: the history is saved, its daily backup copy can't be.
        fs::write(dir.path().join("backup"), b"in the way").unwrap();
        let mut s = open(dir.path());
        s.import_spotify(vec![export(dir.path())]).unwrap();
        let said = settle(&mut s);
        assert!(said.starts_with("Added 2 plays ("), "{said}");
        assert!(
            said.ends_with(" · The daily backup of spotify.json couldn't be written"),
            "{said}"
        );
        assert!(!said.contains(&*dir.path().to_string_lossy()), "{said}");
        assert!(!s.spotify_outcome.as_ref().unwrap().failed);
        assert!(dir.path().join("spotify.json").exists());
        assert_eq!(s.spotify_summary().unwrap().plays, 2);
    }

    #[test]
    fn a_damaged_import_record_refuses_the_import_and_stays_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = open(dir.path());
        s.import_spotify(vec![export(dir.path())]).unwrap();
        settle(&mut s);
        let file = dir.path().join("spotify.json");
        let mut doc: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        doc.as_object_mut().unwrap().remove("spotify");
        let damaged = serde_json::to_vec(&doc).unwrap();
        fs::write(&file, &damaged).unwrap();
        let mut s = open(dir.path());
        s.import_spotify(vec![export(dir.path())]).unwrap();
        assert_eq!(
            settle(&mut s),
            "Your saved Spotify history's import record is damaged, so nothing was imported."
        );
        assert!(s.spotify_outcome.as_ref().unwrap().failed);
        assert_eq!(fs::read(&file).unwrap(), damaged);
    }

    #[test]
    fn a_worker_that_panics_still_wakes_the_window() {
        let ctx = Context::default();
        let woken = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = woken.clone();
        ctx.set_request_repaint_callback(move |_| flag.store(true, Ordering::SeqCst));
        let mut job = spawn_job(JobKind::Import, &ctx, |_| panic!("a worker that fails")).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let (outcome, layer) = loop {
            if let Some(ended) = job.poll() {
                break ended;
            }
            assert!(Instant::now() < deadline, "the job never ended");
            std::thread::sleep(Duration::from_millis(2));
        };
        assert!(woken.load(Ordering::SeqCst));
        assert!(outcome.failed && layer.is_none());
    }

    #[test]
    fn a_worker_that_stops_never_wipes_a_history_already_shown() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = open(dir.path());
        s.import_spotify(vec![export(dir.path())]).unwrap();
        settle(&mut s);
        let revision = s.spotify_revision;
        let (tx, rx) = bounded::<SpotifyLayer>(1);
        drop(tx);
        s.watch_spotify_read(rx);
        assert_eq!(s.poll_spotify(), None);
        assert!(!s.spotify_loading());
        assert!(s.spotify_view().is_some() && s.spotify_error.is_none());
        assert_eq!(s.spotify_revision, revision);
    }
}
