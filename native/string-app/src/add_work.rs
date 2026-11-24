use crate::{playback::Session, playlist_add, scan::Scanner};
use std::{
    collections::HashSet,
    fs, io,
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};
use string_platform::instance::Instance;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Location {
    pub path: PathBuf,
    pub directory: bool,
}

pub(crate) fn classify(paths: Vec<PathBuf>) -> Vec<Location> {
    let mut seen = HashSet::new();
    paths
        .into_iter()
        .filter(|p| p.is_absolute())
        .filter_map(|path| {
            let key = path.to_string_lossy().to_lowercase();
            if seen.contains(&key) {
                return None;
            }
            let m = fs::metadata(&path).ok()?;
            (m.is_dir() || m.is_file()).then(|| {
                seen.insert(key);
                Location {
                    path,
                    directory: m.is_dir(),
                }
            })
        })
        .collect()
}

#[derive(Default)]
struct Pending {
    closed: bool,
    paths: usize,
    bytes: usize,
    wake: Option<egui::Context>,
}

// The charge follows a request through the worker and its result queue until the session applies it.
struct Charge {
    state: Arc<Mutex<Pending>>,
    paths: usize,
    bytes: usize,
}
impl Drop for Charge {
    fn drop(&mut self) {
        let mut p = self.state.lock().unwrap();
        p.paths -= self.paths;
        p.bytes -= self.bytes;
    }
}

enum Source {
    Helper(Option<String>),
    Picker,
    Drop { skipped: usize },
}
type Job = (Source, Vec<PathBuf>, Charge);

fn path_charge(path: &Path) -> usize {
    (path.as_os_str().encode_wide().count() + 1) * 2
}

#[derive(Clone)]
pub(crate) struct Input {
    state: Arc<Mutex<Pending>>,
    jobs: mpsc::Sender<Job>,
}

impl Input {
    pub fn startup(&self, name: Option<&str>, paths: &[PathBuf]) -> io::Result<()> {
        let name_bytes = name.map_or(0, |n| (n.encode_utf16().count() + 1) * 2);
        let bytes = name_bytes + paths.iter().map(|p| path_charge(p)).sum::<usize>();
        if self.submit(name.map(str::to_owned), paths.to_vec(), bytes) {
            Ok(())
        } else {
            Err(io::Error::other("String could not take the startup add"))
        }
    }
    pub(crate) fn submit(&self, name: Option<String>, paths: Vec<PathBuf>, bytes: usize) -> bool {
        self.enqueue(Source::Helper(name), paths, bytes)
    }
    pub(crate) fn window_path(&self, path: PathBuf) -> bool {
        let Ok(path) = std::path::absolute(path) else {
            return false;
        };
        let bytes = path_charge(&path);
        self.enqueue(Source::Picker, vec![path], bytes)
    }
    pub(crate) fn queue_drop(&self, paths: Vec<PathBuf>) -> bool {
        self.drop_paths(paths, false)
    }
    /// The songs and folders of a drop taken before String started closing, sorted since: let in while closing
    /// too, as they would have been had they come in at once.
    pub(crate) fn queue_sorted_drop(&self, paths: Vec<PathBuf>) -> bool {
        self.drop_paths(paths, true)
    }
    fn drop_paths(&self, paths: Vec<PathBuf>, while_closing: bool) -> bool {
        let bytes = paths.iter().map(|p| path_charge(p)).sum();
        self.enqueue_with(Source::Drop { skipped: 0 }, paths, bytes, while_closing)
    }
    /// String has started closing: it takes no new adds.
    pub(crate) fn closed(&self) -> bool {
        self.state.lock().unwrap().closed
    }
    fn enqueue(&self, source: Source, paths: Vec<PathBuf>, bytes: usize) -> bool {
        self.enqueue_with(source, paths, bytes, false)
    }
    fn enqueue_with(
        &self,
        mut source: Source,
        mut paths: Vec<PathBuf>,
        mut bytes: usize,
        while_closing: bool,
    ) -> bool {
        let mut p = self.state.lock().unwrap();
        if (p.closed && !while_closing) || paths.is_empty() {
            return false;
        }
        let free_paths = Instance::MAX_ADD_PATHS - p.paths;
        let free_bytes = Instance::MAX_ADD_BYTES - p.bytes;
        let fits = paths.len() <= free_paths && bytes <= free_bytes;
        if !fits {
            let Source::Drop { skipped } = &mut source else {
                return false;
            };
            if free_paths == 0 {
                return false;
            }
            // Drops used to admit each path separately. Keep what fits, in the same order.
            let total = paths.len();
            let mut count = 0;
            bytes = 0;
            paths.retain(|path| {
                let size = path_charge(path);
                if count == free_paths || size > free_bytes - bytes {
                    return false;
                }
                count += 1;
                bytes += size;
                true
            });
            if paths.is_empty() {
                return false;
            }
            *skipped = total - paths.len();
        }
        p.paths += paths.len();
        p.bytes += bytes;
        let charge = Charge {
            state: self.state.clone(),
            paths: paths.len(),
            bytes,
        };
        // Serialize submissions to preserve admission order. Release the lock before dropping a failed send.
        let sent = self.jobs.send((source, paths, charge));
        drop(p);
        sent.is_ok()
    }
}

pub(crate) struct Batch {
    source: Source,
    locations: Vec<Location>,
    _charge: Option<Charge>,
}
impl Batch {
    fn new(source: Source, mut locations: Vec<Location>, charge: Option<Charge>) -> Self {
        if !matches!(source, Source::Picker) {
            locations.retain(|l| l.directory || crate::screens::settings::is_song_path(&l.path));
        }
        Self {
            source,
            locations,
            _charge: charge,
        }
    }
    /// Adds the checked paths; for drops, returns what to tell the person.
    pub fn apply(self, s: &mut Session, scan: &mut Scanner) -> Option<String> {
        if let Source::Helper(Some(name)) = &self.source {
            // A song already in the library still gets its requested playlist position.
            playlist_add::reserve(s, name, &self.locations);
        }
        let mut folders = Vec::new();
        let mut songs = Vec::new();
        for loc in self.locations {
            if loc.directory {
                folders.push(loc.path);
            } else {
                songs.push(loc.path);
            }
        }
        let a = crate::screens::settings::add_roots_to(s, scan, &folders);
        let b = crate::screens::settings::add_songs_to(s, scan, &songs);
        if matches!(self.source, Source::Helper(_)) {
            scan.requested |= !folders.is_empty() || !songs.is_empty();
            return None;
        }
        let message = match (a, b) {
            (Some(a), Some(b)) => Some(format!("{a} · {b}")),
            (a, b) => a.or(b),
        };
        if let Source::Drop { skipped } = self.source {
            let message = message.unwrap_or_else(|| "Those aren't songs String can play".into());
            return Some(if skipped == 0 {
                message
            } else {
                format!("{message} · {skipped} skipped")
            });
        }
        message
    }
}

/// Dropped paths checked and added at once, with the same rules as a drop through the worker (the self-test's way).
pub(crate) fn add_dropped_now(
    s: &mut Session,
    scan: &mut Scanner,
    paths: Vec<PathBuf>,
) -> Option<String> {
    Batch::new(Source::Drop { skipped: 0 }, classify(paths), None).apply(s, scan)
}

pub(crate) struct Adds {
    pub input: Input,
    ready: mpsc::Receiver<Batch>,
    startup: Option<Batch>,
}

impl Adds {
    pub fn new() -> io::Result<Self> {
        Self::with_classifier(classify)
    }
    pub(crate) fn with_classifier(
        inspect: impl Fn(Vec<PathBuf>) -> Vec<Location> + Send + 'static,
    ) -> io::Result<Self> {
        let (jobs, inbox) = mpsc::channel::<Job>();
        let (finished, ready) = mpsc::channel();
        thread::Builder::new()
            .name("string-add-paths".into())
            .spawn(move || {
                for (source, paths, charge) in inbox {
                    let locations = inspect(paths);
                    let state = charge.state.clone();
                    if finished
                        .send(Batch::new(source, locations, Some(charge)))
                        .is_err()
                    {
                        break;
                    }
                    if let Some(ctx) = state.lock().unwrap().wake.clone() {
                        ctx.request_repaint();
                    }
                }
            })?;
        Ok(Self {
            input: Input {
                state: Default::default(),
                jobs,
            },
            ready,
            startup: None,
        })
    }
    pub fn attach(&self, ctx: &egui::Context) {
        self.input.state.lock().unwrap().wake = Some(ctx.clone());
    }
    /// Startup was queued before opening the receiver, so its result always arrives first.
    pub fn finish_startup(&mut self) -> io::Result<Vec<PathBuf>> {
        let batch = self
            .ready
            .recv()
            .map_err(|_| io::Error::other("Path worker stopped"))?;
        let paths = batch.locations.iter().map(|l| l.path.clone()).collect();
        if !batch.locations.is_empty() {
            self.startup = Some(batch);
        }
        Ok(paths)
    }
    pub fn set_closed(&self, closed: bool) {
        self.input.state.lock().unwrap().closed = closed;
    }
    pub fn close_if_idle(&self) -> bool {
        let mut p = self.input.state.lock().unwrap();
        if p.paths != 0 {
            return false;
        }
        p.closed = true;
        true
    }
    pub fn pending(&self) -> bool {
        self.input.state.lock().unwrap().paths != 0
    }
    pub fn poll(&mut self, s: &mut Session, scan: &mut Scanner, toast: &mut crate::ui::Toast) {
        // Picker results arrive as separate paths. Each drag-and-drop already arrives in one batch.
        let mut picked = Vec::new();
        let mut charges = Vec::new();
        let mut missing = false;
        for batch in self.startup.take().into_iter().chain(self.ready.try_iter()) {
            if matches!(batch.source, Source::Picker) {
                missing |= batch.locations.is_empty();
                picked.extend(batch.locations);
                charges.push(batch._charge);
            } else if let Some(message) = batch.apply(s, scan) {
                toast.set(message);
            }
        }
        if picked.is_empty() {
            if missing {
                toast.set("This path does not exist.");
            }
            return;
        }
        let message = Batch {
            source: Source::Picker,
            locations: picked,
            _charge: None,
        }
        .apply(s, scan);
        // Their share of the pending limit is released only once they're in the library.
        drop(charges);
        if let Some(message) = message {
            toast.set(message);
        }
    }
    /// Applies the adds taken before closing until none is `unfinished` (asked after each look at the queue) or
    /// `deadline`; `wait` waits, until the time it's given, for work not queued yet. Then, without waiting, whatever
    /// is ready by then: a batch can become ready between the last look at the queue and the look at the clock.
    pub fn finish_at_exit(
        &mut self,
        deadline: Instant,
        unfinished: impl Fn(&Self) -> bool,
        mut wait: impl FnMut(Instant),
        mut apply: impl FnMut(Batch),
    ) {
        loop {
            while let Some(batch) = self.next_at_exit(deadline) {
                apply(batch);
            }
            if !unfinished(self) || Instant::now() >= deadline {
                break;
            }
            wait((Instant::now() + Duration::from_millis(5)).min(deadline));
        }
        for batch in self.ready.try_iter() {
            apply(batch);
        }
    }
    pub fn next_at_exit(&mut self, deadline: Instant) -> Option<Batch> {
        self.startup.take().or_else(|| {
            if !self.pending() {
                return None;
            }
            self.ready
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .ok()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use string_core::library::track_id;

    fn named_add(s: &mut Session, paths: Vec<PathBuf>) {
        Batch::new(Source::Helper(Some("List".into())), classify(paths), None)
            .apply(s, &mut Scanner::default());
    }

    fn scan_selection(s: &mut Session) -> string_core::library::ScanReport {
        let [roots, files] = ["roots", "files"].map(|key| {
            s.field(key)
                .as_array()
                .unwrap()
                .iter()
                .map(|v| PathBuf::from(v.as_str().unwrap()))
                .collect::<Vec<_>>()
        });
        let report = s.library.scan(&roots, &files);
        s.rebuild();
        playlist_add::resolve_folders(s, &report.complete_folders);
        report
    }

    fn song(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(
            &path,
            include_bytes!("../../string-core/tests/fixtures/tag-scan/tone.flac"),
        )
        .unwrap();
        path
    }

    #[test]
    fn final_add_readmits_a_cached_song_after_its_root_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let file = song(dir.path(), "keep.flac");
        song(dir.path(), "leave.flac");
        let mut s = Session::new(Default::default(), Default::default(), Default::default());
        s.set_field("roots", json!([dir.path()]));
        assert_eq!(scan_selection(&mut s).read, 2);
        s.set_field("roots", json!([]));
        named_add(&mut s, vec![file.clone(), file.clone()]);
        assert_eq!(scan_selection(&mut s).removed, 1);
        assert_eq!(s.field("files"), &json!([file]));
        assert_eq!(
            &*s.models.playlists[0].ids,
            &[track_id(file.to_str().unwrap())]
        );
    }

    #[test]
    fn final_explicit_roots_reach_skipped_names_and_depths_in_either_order() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let deep = (0..15).fold(root.to_owned(), |p, _| p.join("d"));
        let hidden = root.join(".archive").join("disc");
        let ordinary = root.join("album");
        let folders = [&deep, &hidden, &ordinary];
        let mut ids = Vec::new();
        for folder in folders {
            fs::create_dir_all(folder).unwrap();
            ids.push(track_id(song(folder, "one.flac").to_str().unwrap()));
        }
        for parent_first in [true, false] {
            let mut s = Session::new(Default::default(), Default::default(), Default::default());
            if parent_first {
                s.set_field("roots", json!([root]));
            }
            named_add(&mut s, folders.into_iter().cloned().collect());
            if !parent_first {
                Batch::new(Source::Helper(None), classify(vec![root.to_owned()]), None)
                    .apply(&mut s, &mut Scanner::default());
            }
            assert_eq!(scan_selection(&mut s).read, 3);
            assert_eq!(&*s.models.playlists[0].ids, &ids);
            assert_eq!(s.field("roots").as_array().unwrap().len(), 3);
        }
    }

    #[test]
    fn final_batch_uses_the_roots_that_will_actually_be_scanned() {
        let root = PathBuf::from(r"C:\Music");
        let middle = (0..10).fold(root.clone(), |p, _| p.join("disc"));
        let deep = (0..10).fold(middle.clone(), |p, _| p.join("disc"));
        let mut folders = vec![deep.clone(), middle, root.clone()];
        for name in [
            "$offline",
            "node_modules",
            "System Volume Information",
            "__MACOSX",
        ] {
            folders.push(root.join(name).join("songs"));
        }
        let mut expected = folders.clone();
        expected.remove(1);
        let mut s = Session::new(Default::default(), Default::default(), Default::default());
        let folders = folders
            .into_iter()
            .map(|path| Location {
                path,
                directory: true,
            })
            .collect();
        Batch::new(Source::Helper(None), folders, None).apply(&mut s, &mut Scanner::default());
        assert_eq!(s.field("roots"), &json!(expected));
    }

    #[test]
    fn final_unavailable_link_keeps_the_folder_place_until_its_target_returns() {
        let tmp = tempfile::tempdir().unwrap();
        let album = tmp.path().join("album");
        let target = tmp.path().join("target");
        let link = album.join("disc");
        fs::create_dir(&album).unwrap();
        fs::create_dir(&target).unwrap();
        assert!(
            std::process::Command::new("cmd")
                .args(["/d", "/c", "mklink", "/J"])
                .arg(&link)
                .arg(&target)
                .output()
                .unwrap()
                .status
                .success()
        );
        let tail = song(tmp.path(), "last.flac");
        let mut s = Session::new(Default::default(), Default::default(), Default::default());
        named_add(&mut s, vec![album, tail.clone()]);
        let marker = s.field("playlists")[0]["items"][0].clone();
        fs::remove_dir(&target).unwrap();
        scan_selection(&mut s);
        assert_eq!(s.field("playlists")[0]["items"][0], marker);
        let mut s = Session::new(s.state, Default::default(), s.library);
        fs::create_dir(&target).unwrap();
        song(&target, "first.flac");
        let recovered = link.join("first.flac");
        assert!(scan_selection(&mut s).issues.is_empty());
        assert_eq!(
            &*s.models.playlists[0].ids,
            &[
                track_id(recovered.to_str().unwrap()),
                track_id(tail.to_str().unwrap())
            ]
        );
    }

    #[test]
    fn final_bulk_helper_folders_fit_one_ui_operation() {
        use windows::Win32::{
            Foundation::FILETIME,
            System::Threading::{GetCurrentThread, GetThreadTimes},
        };
        fn thread_cpu() -> Duration {
            let [mut created, mut exit, mut kernel, mut user] = [FILETIME::default(); 4];
            unsafe {
                GetThreadTimes(
                    GetCurrentThread(),
                    &mut created,
                    &mut exit,
                    &mut kernel,
                    &mut user,
                )
            }
            .unwrap();
            let ticks =
                |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
            Duration::from_nanos((ticks(kernel) + ticks(user)) * 100)
        }
        let mut s = Session::new(Default::default(), Default::default(), Default::default());
        let folders = (0..4_000)
            .map(|i| Location {
                path: PathBuf::from(format!(r"C:\Collection\album-{i}")),
                directory: true,
            })
            .collect();
        let batch = Batch::new(Source::Helper(None), folders, None);
        let start = thread_cpu();
        let wall = Instant::now();
        batch.apply(&mut s, &mut Scanner::default());
        let elapsed = thread_cpu() - start;
        eprintln!(
            "4,000 sibling folders on the session thread: {elapsed:?} CPU, {:?} elapsed",
            wall.elapsed()
        );
        assert!(elapsed < Duration::from_millis(250), "{elapsed:?}");
        assert_eq!(s.field("roots").as_array().unwrap().len(), 4_000);
    }

    #[test]
    fn a_partly_full_queue_keeps_the_dropped_paths_that_fit() {
        let adds = Adds::with_classifier(|paths| {
            paths
                .into_iter()
                .map(|path| Location {
                    path,
                    directory: false,
                })
                .collect()
        })
        .unwrap();
        let small = PathBuf::from(r"C:\a.mp3");
        let large = PathBuf::from(r"C:\larger.mp3");
        assert!(adds.input.submit(
            None,
            vec![large.clone()],
            Instance::MAX_ADD_BYTES - path_charge(&small)
        ));
        assert!(adds.input.queue_drop(vec![large, small.clone()]));
        let wait = || adds.ready.recv_timeout(Duration::from_secs(2)).unwrap();
        let _helper = wait();
        let drop = wait();
        assert!(matches!(drop.source, Source::Drop { skipped: 1 }));
        assert_eq!(
            drop.locations,
            [Location {
                path: small,
                directory: false
            }]
        );
    }

    #[test]
    fn an_exit_takes_in_a_batch_that_became_ready_just_before_the_deadline() {
        let ctx = egui::Context::default();
        let (woken, ready) = mpsc::channel();
        ctx.set_request_repaint_callback(move |_| {
            let _ = woken.send(());
        });
        let mut adds = Adds::with_classifier(|paths| {
            paths
                .into_iter()
                .map(|path| Location {
                    path,
                    directory: false,
                })
                .collect()
        })
        .unwrap();
        adds.attach(&ctx);
        adds.set_closed(true);
        let input = adds.input.clone();
        let song = PathBuf::from(r"C:\Music\late.wav");
        let queued = std::cell::Cell::new(false);
        let mut applied = vec![];
        // The time is up when the exit looks at the clock.
        let deadline = Instant::now();
        adds.finish_at_exit(
            deadline,
            |adds| {
                // Between the exit's look at an empty queue and its look at the clock, a sorted drop's song is
                // checked and ready to apply.
                if !queued.replace(true) {
                    assert!(input.queue_sorted_drop(vec![song.clone()]));
                    ready.recv_timeout(Duration::from_secs(2)).unwrap();
                }
                adds.pending()
            },
            |_| {},
            |batch| applied.extend(batch.locations.iter().map(|l| l.path.clone())),
        );
        assert_eq!(applied, [song]);
        assert!(!adds.pending());
    }

    #[test]
    fn capacity_covers_queued_work_the_classifier_and_unapplied_results() {
        let (started, working) = mpsc::channel();
        let (resume, gate) = mpsc::channel();
        let adds = Adds::with_classifier(move |_| {
            started.send(thread::current().id()).unwrap();
            gate.recv().unwrap();
            Vec::new()
        })
        .unwrap();
        let path = || vec![PathBuf::from(r"C:\Music\example.wav")];
        assert!(adds.input.submit(None, path(), Instance::MAX_ADD_BYTES - 2));
        assert_ne!(
            working.recv_timeout(Duration::from_secs(2)).unwrap(),
            thread::current().id()
        );
        assert!(adds.input.submit(Some("List".into()), path(), 2));
        assert!(!adds.input.submit(None, path(), 2));
        resume.send(()).unwrap();
        let batch = adds.ready.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(!adds.input.submit(None, path(), 2));
        drop(batch);
        resume.send(()).unwrap();
        drop(adds.ready.recv_timeout(Duration::from_secs(2)).unwrap());
        assert!(
            adds.input
                .submit(None, vec![PathBuf::new(); Instance::MAX_ADD_PATHS], 2)
        );
        assert!(!adds.input.submit(None, path(), 2));
        adds.set_closed(true);
        resume.send(()).unwrap();
        drop(adds.ready.recv_timeout(Duration::from_secs(2)).unwrap());
        assert!(!adds.input.submit(None, path(), 2));
        adds.set_closed(false);
        assert!(adds.input.submit(None, path(), 2));
        resume.send(()).unwrap();
        drop(adds.ready.recv_timeout(Duration::from_secs(2)).unwrap());
    }
}
