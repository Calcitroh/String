use super::*;
use crate::{
    add_work::{Adds, Location},
    integration::spotify::{self, Drops},
    scan::Scanner,
    storage::{self, Storage},
};
use std::{
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};

#[derive(Debug)]
struct File(PathBuf);
impl egui::DroppedFile for File {
    fn path(&self) -> &Path {
        &self.0
    }
    fn bytes(&self) -> Result<Vec<u8>, String> {
        panic!("The drop handler should only need the path")
    }
}

struct Dropping {
    ctx: Context,
    adds: Adds,
    sorter: Drops,
    toast: ui::Toast,
    /// Every toast shown, oldest first.
    said: Vec<String>,
}
impl Dropping {
    fn new(adds: Adds, sorter: Drops) -> Self {
        let ctx = Context::default();
        crate::theme::install(&ctx);
        Self {
            ctx,
            adds,
            sorter,
            toast: Default::default(),
            said: vec![],
        }
    }
    fn frame(&mut self, input: egui::RawInput, on_stats: bool) -> egui::FullOutput {
        let Self {
            adds,
            sorter,
            toast,
            ..
        } = self;
        self.ctx.run_ui(input, |ui| {
            drops(ui.ctx(), Tint::Mono, &adds.input, sorter, on_stats, toast)
        })
    }
    fn drop(&mut self, paths: &[PathBuf], on_stats: bool) {
        let input = egui::RawInput {
            dropped_files: paths
                .iter()
                .cloned()
                .map(|p| Arc::new(File(p)) as egui::DroppedFileHandle)
                .collect(),
            ..Default::default()
        };
        self.frame(input, on_stats);
    }
    fn heard(&mut self) {
        if let Some((message, _)) = self.toast.current.take() {
            self.said.push(message);
        }
    }
    /// Waits until the drops are sorted, and the music among them handed to the library.
    fn sorted(&mut self, s: &mut Session) {
        let until = Instant::now() + Duration::from_secs(5);
        while self.sorter.pending() {
            assert!(Instant::now() < until, "Drop was never sorted");
            thread::sleep(Duration::from_millis(1));
            self.sorter.poll(s, &mut self.toast);
        }
    }
    /// Runs what the frames run until the drop is sorted, added and imported.
    fn finish(&mut self, s: &mut Session, scan: &mut Scanner) {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            self.sorter.poll(s, &mut self.toast);
            self.heard();
            self.adds.poll(s, scan, &mut self.toast);
            self.heard();
            if let Some(message) = s.poll_spotify() {
                self.said.push(message);
            }
            if !self.sorter.pending() && !self.adds.pending() && !s.spotify_busy() {
                break;
            }
            assert!(Instant::now() < until, "Drop did not finish");
            thread::sleep(Duration::from_millis(1));
        }
    }
}

/// A Spotify export as it's downloaded: a zip with the history inside a folder.
fn export(dir: &Path) -> PathBuf {
    let path = dir.join("my_spotify_data.zip");
    spotify::write_export(&path, &spotify::sample_files(2, &[])).unwrap();
    path
}
/// The same export unzipped into `dir`: `dir\my_spotify_data`.
fn unzipped(dir: &Path) -> PathBuf {
    let top = dir.join("my_spotify_data");
    let inner = top.join("Spotify Extended Streaming History");
    std::fs::create_dir_all(&inner).unwrap();
    for (name, plays) in spotify::sample_files(2, &[]) {
        std::fs::write(inner.join(name), serde_json::to_vec(&plays).unwrap()).unwrap();
    }
    top
}

#[test]
fn hover_and_drop_keep_processing_frames_while_the_classifier_is_blocked() {
    let temp = tempfile::tempdir().unwrap();
    // Only the injected classifier knows these paths. An on-frame stat would reject them.
    let paths = ["disc.txt", "one.FLAC", "two.wav", "notes.txt"].map(|n| temp.path().join(n));
    let (entered, started) = mpsc::channel();
    let (resume, wait) = mpsc::channel();
    let adds = Adds::with_classifier(move |paths| {
        entered
            .send((thread::current().id(), paths.clone()))
            .unwrap();
        wait.recv_timeout(Duration::from_secs(5)).unwrap();
        paths
            .into_iter()
            .map(|path| Location {
                directory: path.ends_with("disc.txt"),
                path,
            })
            .collect()
    })
    .unwrap();
    let mut d = Dropping::new(adds, Drops::default());
    let hover = egui::RawInput {
        hovered_files: vec![egui::HoveredFile {
            path: Some(paths[0].clone()),
            ..Default::default()
        }],
        ..Default::default()
    };
    let out = d.frame(hover, false);
    assert!(out.shapes.iter().any(|s| matches!(&s.shape, Shape::Text(t)
        if t.galley.job.text.eq_ignore_ascii_case("Drop folders or songs to add them"))));
    assert!(
        !d.adds.pending() && !d.sorter.pending(),
        "Hover alone should never enqueue a disk check"
    );
    d.drop(&paths, false);
    let (worker, received) = started
        .recv_timeout(Duration::from_secs(2))
        .expect("The UI should forward every path, including unavailable ones");
    assert_ne!(thread::current().id(), worker);
    assert_eq!(received, paths);

    let mut session = Session::new(Default::default(), Default::default(), Default::default());
    let mut scan = Scanner::default();
    d.sorter.poll(&mut session, &mut d.toast);
    d.adds.poll(&mut session, &mut scan, &mut d.toast);
    d.drop(&[], false);
    assert!(d.adds.pending());
    assert!(d.toast.current.is_none());
    assert!(!scan.requested);
    resume.send(()).unwrap();
    d.finish(&mut session, &mut scan);
    assert_eq!(*session.field("roots"), json!([paths[0]]));
    assert_eq!(*session.field("files"), json!([paths[1], paths[2]]));
    assert_eq!(d.said, ["Folder added: reading its songs · 2 songs added"]);
    assert!(scan.requested);
}

#[test]
fn rejected_and_duplicate_drops_keep_their_messages() {
    let temp = tempfile::tempdir().unwrap();
    let note = temp.path().join("notes.txt");
    let missing = temp.path().join("missing.wav");
    let song = temp.path().join("one.mp3");
    std::fs::write(&note, []).unwrap();
    std::fs::write(&song, []).unwrap();
    let mut d = Dropping::new(Adds::new().unwrap(), Drops::default());
    let mut session = Session::new(Default::default(), Default::default(), Default::default());
    let mut scan = Scanner::default();
    for (paths, message) in [
        (
            vec![note.clone(), missing.clone()],
            "Those aren't songs String can play",
        ),
        (vec![missing.clone()], "Those aren't songs String can play"),
        (vec![song.clone(), note.clone(), missing], "1 song added"),
        (
            vec![song.clone(), note],
            "Those are already in your library",
        ),
    ] {
        d.drop(&paths, false);
        d.finish(&mut session, &mut scan);
        assert_eq!(d.said.pop().unwrap(), message);
    }
    assert_eq!(*session.field("files"), json!([song]));
    d.adds.set_closed(true);
    d.said.clear();
    d.drop(&[song], false);
    d.finish(&mut session, &mut scan);
    assert_eq!(d.said, ["String could not take more songs."]);
}

#[test]
fn review_partial_drop_reports_added_and_skipped_together() {
    let temp = tempfile::tempdir().unwrap();
    let song = temp.path().join("a.wav");
    let second = temp.path().join("b.wav");
    std::fs::write(&song, []).unwrap();
    std::fs::write(&second, []).unwrap();
    let adds = Adds::new().unwrap();
    use std::os::windows::ffi::OsStrExt;
    let room = (song.as_os_str().encode_wide().count() + 1) * 2;
    let taken = string_platform::instance::Instance::MAX_ADD_BYTES - room;
    assert!(
        adds.input
            .submit(None, vec![temp.path().join("missing")], taken)
    );
    let mut d = Dropping::new(adds, Drops::default());
    d.drop(&[song.clone(), second], false);
    let mut session = Session::new(Default::default(), Default::default(), Default::default());
    // The earlier add holds its room until the session takes it in.
    d.sorted(&mut session);
    d.finish(&mut session, &mut Scanner::default());
    assert_eq!(*session.field("files"), json!([song]));
    assert_eq!(
        d.said,
        ["1 song added · 1 skipped"],
        "a partially admitted drop has one completion message"
    );
}

#[test]
fn a_history_drop_on_stats_imports_and_music_still_goes_to_the_library() {
    let temp = tempfile::tempdir().unwrap();
    let folder = storage::folder(&temp.path().join("profile"), true);
    let mut d = Dropping::new(Adds::new().unwrap(), Drops::default());
    let (_store, mut session) = Storage::open(&folder, &d.ctx).unwrap();
    let zip = export(temp.path());
    let song = temp.path().join("one.wav");
    let album = temp.path().join("Album");
    std::fs::write(&song, []).unwrap();
    std::fs::create_dir(&album).unwrap();
    std::fs::write(album.join("two.flac"), []).unwrap();
    let mut scan = Scanner::default();
    d.drop(&[album.clone(), zip.clone(), song.clone()], true);
    d.finish(&mut session, &mut scan);
    assert_eq!(*session.field("roots"), json!([album]));
    assert_eq!(*session.field("files"), json!([song]));
    assert!(temp.path().join("profile/spotify.json").exists());
    assert_eq!(session.spotify_summary().map(|s| s.plays), Some(2));
    assert!(
        d.said.iter().any(|m| m.starts_with("Added 2 plays (")
            && m.ends_with(") from Spotify · 0% are songs in your library")),
        "{:?}",
        d.said
    );
    assert!(
        d.said
            .contains(&"Folder added: reading its songs · 1 song added".to_string()),
        "{:?}",
        d.said
    );

    // A drop while an import runs isn't queued: the panel shows the one running.
    session.import_spotify(vec![zip.clone()]).unwrap();
    d.drop(&[zip.clone()], true);
    let until = Instant::now() + Duration::from_secs(5);
    while d.sorter.pending() {
        assert!(Instant::now() < until);
        d.sorter.poll(&mut session, &mut d.toast);
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(d.toast.current.take().unwrap().0, spotify::BUSY);
    d.said.clear();
    d.finish(&mut session, &mut scan);
    assert_eq!(
        d.said,
        ["This export holds no plays newer than the ones already in Stats."]
    );
    assert_eq!(session.spotify_summary().map(|s| s.plays), Some(2));
}

#[test]
fn a_history_drop_elsewhere_adds_nothing_and_says_where_to_drop() {
    let temp = tempfile::tempdir().unwrap();
    let folder = storage::folder(&temp.path().join("profile"), true);
    let mut d = Dropping::new(Adds::new().unwrap(), Drops::default());
    let (_store, mut session) = Storage::open(&folder, &d.ctx).unwrap();
    let zip = export(temp.path());
    // The same history unzipped: a folder, which the library would otherwise take as a music folder.
    let folder = unzipped(temp.path());
    let mut scan = Scanner::default();
    d.drop(&[zip.clone(), folder], false);
    d.finish(&mut session, &mut scan);
    assert_eq!(d.said, ["That's a Spotify history: drop it on Stats"]);
    for list in ["roots", "files"] {
        assert!(
            session.field(list).as_array().is_none_or(Vec::is_empty),
            "{list}"
        );
    }
    assert!(!scan.requested);
    assert!(!temp.path().join("profile/spotify.json").exists());
    assert!(session.spotify.is_none() && session.spotify_outcome.is_none());

    // Songs dropped with it still go to the library.
    let song = temp.path().join("one.mp3");
    std::fs::write(&song, []).unwrap();
    d.said.clear();
    d.drop(&[zip, song.clone()], false);
    d.finish(&mut session, &mut scan);
    assert_eq!(*session.field("files"), json!([song]));
    assert!(
        d.said
            .contains(&"That's a Spotify history: drop it on Stats".to_string()),
        "{:?}",
        d.said
    );
    assert!(!temp.path().join("profile/spotify.json").exists());
}

#[test]
fn classifying_drops_never_blocks_the_ui_thread() {
    let temp = tempfile::tempdir().unwrap();
    let song = temp.path().join("one.wav");
    std::fs::write(&song, []).unwrap();
    // Only the injected check knows the zip is a history: it never touches the disk.
    let zip = temp.path().join("not even there.zip");
    let (entered, started) = mpsc::channel();
    let (resume, wait) = mpsc::channel::<()>();
    let sorter = Drops::with_classifier(move |path: &Path| {
        entered
            .send((thread::current().id(), path.to_owned()))
            .unwrap();
        wait.recv_timeout(Duration::from_secs(5)).unwrap();
        let zip = path.extension().is_some_and(|e| e == "zip");
        string_extras::spotify::Holds {
            history: zip,
            music: !zip,
        }
    });
    let mut d = Dropping::new(Adds::new().unwrap(), sorter);
    let mut session = Session::new(Default::default(), Default::default(), Default::default());
    let start = Instant::now();
    d.drop(&[zip.clone(), song.clone()], false);
    let (worker, first) = started.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_ne!(thread::current().id(), worker);
    assert_eq!(first, zip);
    // Frames go on, hovering and dropping nothing, while the check is stuck.
    for _ in 0..5 {
        let hover = egui::RawInput {
            hovered_files: vec![egui::HoveredFile::default()],
            ..Default::default()
        };
        let out = d.frame(hover, true);
        assert!(out.shapes.iter().any(|s| matches!(&s.shape, Shape::Text(t)
            if t.galley.job.text.eq_ignore_ascii_case("Drop a Spotify export, or folders and songs to add them"))));
        d.sorter.poll(&mut session, &mut d.toast);
    }
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(d.sorter.pending());
    assert!(
        !d.adds.pending(),
        "the music waits for the drop to be sorted"
    );
    assert!(d.toast.current.is_none());
    resume.send(()).unwrap();
    assert_eq!(
        started.recv_timeout(Duration::from_secs(2)).unwrap().1,
        song
    );
    resume.send(()).unwrap();
    d.finish(&mut session, &mut Scanner::default());
    assert_eq!(*session.field("files"), json!([song]));
    assert!(
        d.said
            .contains(&"That's a Spotify history: drop it on Stats".to_string()),
        "{:?}",
        d.said
    );
}

#[test]
fn a_folder_of_songs_holding_an_export_still_goes_to_the_library() {
    let temp = tempfile::tempdir().unwrap();
    // Downloads: an album, and the export unzipped beside it.
    let downloads = temp.path().join("Downloads");
    std::fs::create_dir_all(downloads.join("Album")).unwrap();
    std::fs::write(downloads.join("Album/one.flac"), []).unwrap();
    unzipped(&downloads);
    let mut d = Dropping::new(Adds::new().unwrap(), Drops::default());
    let mut scan = Scanner::default();

    // Elsewhere: its songs come in, and nothing is said about Stats.
    let folder = storage::folder(&temp.path().join("one"), true);
    let (_store, mut session) = Storage::open(&folder, &d.ctx).unwrap();
    d.drop(&[downloads.clone()], false);
    d.finish(&mut session, &mut scan);
    assert_eq!(*session.field("roots"), json!([downloads]));
    assert!(
        !d.said.contains(&spotify::ELSEWHERE.to_string()),
        "{:?}",
        d.said
    );
    assert!(!temp.path().join("one/spotify.json").exists());

    // On Stats: imported and added.
    d.said.clear();
    let folder = storage::folder(&temp.path().join("two"), true);
    let (_store, mut session) = Storage::open(&folder, &d.ctx).unwrap();
    d.drop(&[downloads.clone()], true);
    d.finish(&mut session, &mut scan);
    assert_eq!(*session.field("roots"), json!([downloads]));
    assert_eq!(session.spotify_summary().map(|s| s.plays), Some(2));
    assert!(
        d.said.iter().any(|m| m.starts_with("Added 2 plays (")),
        "{:?}",
        d.said
    );
}
