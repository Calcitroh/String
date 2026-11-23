use super::*;
use crate::{Args, PlaylistUsage, add_work::Adds};
use std::time::Instant;
use std::{ffi::OsString, fs, path::Path, process::Command};
use string_core::{library::Library, state::State};
use string_platform::instance;

fn args(words: &[&str]) -> Result<Args, crate::Error> {
    Args::parse_from(words.iter().map(OsString::from))
}

fn session(state: State) -> Session {
    Session::new(state, Default::default(), Library::default())
}

fn reserve(s: &mut Session, name: &str, paths: &[PathBuf]) {
    super::reserve(s, name, &crate::add_work::classify(paths.to_vec()));
}

fn file(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, b"").unwrap();
    path
}

fn finish_wav(path: &Path) {
    // Two silent 16-bit mono samples at 8 kHz.
    let wav = b"RIFF\x28\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0\x40\x1f\0\0\x80\x3e\0\0\x02\0\x10\0data\x04\0\0\0\0\0\0\0";
    fs::write(path, wav).unwrap();
}

fn id(path: &Path) -> String {
    track_id(path.to_str().unwrap())
}

fn visible(s: &Session) -> &[String] {
    &s.models.playlists[0].ids
}

#[test]
fn review_folder_reservation_survives_an_incomplete_scan_and_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let album = dir.path().join("disc");
    let volume = album.join("volume");
    fs::create_dir_all(&volume).unwrap();
    let other = dir.path().join("disc-two");
    fs::create_dir(&other).unwrap();
    let unvisited = other.join("offline");
    fs::create_dir(&unvisited).unwrap();
    let other_song = file(&other, "song.wav");
    let tail = file(dir.path(), "last.wav");
    finish_wav(&tail);
    finish_wav(&other_song);
    let mut s = session(State::default());
    reserve(
        &mut s,
        "List",
        &[
            album.clone(),
            other.clone(),
            tail.clone(),
            unvisited.clone(),
        ],
    );
    let marker = s.field("playlists")[0]["items"][0].clone();
    let covered_marker = s.field("playlists")[0]["items"][3].clone();
    // A separately selected subdirectory becomes unavailable. Its parent is still readable.
    let roots = [album.clone(), volume.clone(), other];
    fs::remove_dir(&volume).unwrap();
    fs::remove_dir(&unvisited).unwrap();
    let report = s.library.scan(&roots, std::slice::from_ref(&tail));
    assert_eq!(report.issues[0].path, volume);
    s.rebuild();
    resolve_folders(&mut s, &report.complete_folders);
    assert_eq!(s.field("playlists")[0]["items"][0], marker);
    assert_eq!(s.field("playlists")[0]["items"][3], covered_marker);
    assert_eq!(visible(&s), [id(&other_song), id(&tail)]);
    let mut s = session(s.state);
    fs::create_dir(&volume).unwrap();
    fs::create_dir(&unvisited).unwrap();
    let song = file(&volume, "restored.wav");
    finish_wav(&song);
    let recovered = file(&unvisited, "returned.wav");
    finish_wav(&recovered);
    let report = s.library.scan(&roots, std::slice::from_ref(&tail));
    assert!(report.issues.is_empty());
    s.rebuild();
    resolve_folders(&mut s, &report.complete_folders);
    assert_eq!(
        visible(&s),
        [id(&song), id(&other_song), id(&tail), id(&recovered)]
    );
}

#[test]
fn review_helpers_share_drop_admission_and_reserve_existing_songs() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("music");
    let disc = root.join("disc");
    fs::create_dir_all(&disc).unwrap();
    let existing = file(&root, "old.wav");
    finish_wav(&existing);
    let note = file(dir.path(), "notes.txt");
    let fresh = file(dir.path(), "new.opus");
    let mut s = session(State::default());
    s.set_field("roots", json!([root]));
    s.library.scan(&[root.clone()], &[]);
    s.rebuild();
    let paths = [note, existing.clone(), disc.clone(), fresh.clone()];
    let mut adds = Adds::new().unwrap();
    assert!(adds.input.submit(Some("Mix".into()), paths.to_vec(), 256));
    let mut scan = crate::scan::Scanner::default();
    adds.next_at_exit(Instant::now() + Duration::from_secs(5))
        .unwrap()
        .apply(&mut s, &mut scan);
    assert_eq!(s.field("roots"), &json!([root]));
    assert_eq!(s.field("files"), &json!([fresh]));
    let items = s.field("playlists")[0]["items"].as_array().unwrap();
    assert_eq!(
        items
            .iter()
            .map(|v| v["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [id(&existing), format!("{}\\", id(&disc)), id(&fresh)]
    );
    assert!(scan.requested);
}

#[test]
fn review_missing_spelling_does_not_hide_an_existing_song() {
    let dir = tempfile::tempdir().unwrap();
    // These Unicode spellings fold to the same String ID, even on case-insensitive NTFS.
    let missing = dir.path().join("\u{130}.wav");
    let existing = file(dir.path(), "i\u{307}.wav");
    assert!(!missing.exists());
    assert_eq!(id(&missing), id(&existing));
    let found = crate::add_work::classify(vec![missing, existing.clone(), existing.clone()]);
    assert_eq!(
        found,
        [Location {
            path: existing,
            directory: false
        }]
    );
}

#[test]
fn review_startup_and_incoming_requests_share_capacity() {
    use std::os::windows::ffi::OsStrExt;
    use string_platform::instance::Instance;
    let dir = tempfile::tempdir().unwrap();
    let path = file(dir.path(), "startup.wav");
    let args = Args::options([
        OsString::from("--add"),
        "--playlist".into(),
        "Mix".into(),
        path.clone().into(),
    ])
    .unwrap();
    let names = Names::custom(&format!("StartupCapacity.{}", std::process::id()));
    let (resume, gate) = std::sync::mpsc::channel();
    let mut adds = Adds::with_classifier(move |paths| {
        let _ = gate.recv();
        crate::add_work::classify(paths)
    })
    .unwrap();
    let Start::Run(_first) = claim(
        &names,
        args.playlist.as_deref(),
        args.add.as_ref().unwrap(),
        Duration::ZERO,
        || Ok(adds.input.clone()),
    )
    .unwrap() else {
        panic!("expected isolated instance");
    };
    let path_bytes = (path.as_os_str().encode_wide().count() + 1) * 2;
    // The remaining bytes must account for the playlist name as well as the path.
    assert!(!adds.input.submit(
        None,
        vec![path.clone()],
        Instance::MAX_ADD_BYTES - path_bytes
    ));
    assert!(!names.send_paths(
        &vec![PathBuf::from(r"C:\a.wav"); Instance::MAX_ADD_PATHS],
        Duration::ZERO
    ));
    resume.send(()).unwrap();
    assert_eq!(adds.finish_startup().unwrap(), [path.clone()]);
    assert!(!adds.input.submit(
        None,
        vec![path.clone()],
        Instance::MAX_ADD_BYTES - path_bytes
    ));
    assert!(names.send_paths(&[path], Duration::ZERO));
}

#[test]
fn review_bulk_playlist_append_finishes_in_one_ui_operation() {
    // Exclude time when the test runner was descheduled by other builds or tests.
    fn thread_cpu() -> Duration {
        use windows::Win32::{
            Foundation::FILETIME,
            System::Threading::{GetCurrentThread, GetThreadTimes},
        };
        let [mut created, mut exited, mut kernel, mut user] = [FILETIME::default(); 4];
        unsafe {
            GetThreadTimes(
                GetCurrentThread(),
                &mut created,
                &mut exited,
                &mut kernel,
                &mut user,
            )
        }
        .unwrap();
        let ticks = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
        Duration::from_nanos((ticks(kernel) + ticks(user)) * 100)
    }
    let mut s = session(State::default());
    let old: Vec<_> = (0..50_000).map(|i| format!("old-{i}")).collect();
    s.add_playlist("Mix".into(), &old);
    let key = s.models.playlists[0].id.clone();
    let mut add: Vec<_> = (0..2_000).map(|i| format!("new-{i}")).collect();
    add.extend(["old-49999".into(), "new-1999".into()]);
    let start = thread_cpu();
    s.add_to_playlist(&key, add.into());
    let elapsed = thread_cpu() - start;
    eprintln!("Bulk append on the session thread: {elapsed:?}");
    assert_eq!(
        s.field("playlists")[0]["items"].as_array().unwrap().len(),
        52_000
    );
    assert!(
        elapsed < Duration::from_secs(1),
        "bulk playlist append took {elapsed:?}"
    );
}

#[test]
fn parses_named_add_and_skips_missing_paths() {
    let dir = tempfile::tempdir().unwrap();
    let song = file(dir.path(), "01.flac");
    let a = Args::parse_from([
        "--add".into(),
        "--playlist".into(),
        "  Études  ".into(),
        song.clone().into(),
        dir.path().join("missing.flac").into(),
        song.clone().into(),
        "--data".into(),
        dir.path().into(),
    ])
    .unwrap();
    assert_eq!(a.playlist.as_deref(), Some("Études"));
    assert_eq!(a.add, Some(vec![song]));
    assert_eq!(a.root, dir.path());
    let missing = args(&["--add", "--playlist", "Mix", "missing.flac"]).unwrap();
    assert_eq!(missing.add, Some(vec![]));
}

#[test]
fn forwarding_preserves_unchecked_paths_until_the_receivers_worker_classifies_them() {
    let dir = tempfile::tempdir().unwrap();
    let pending = dir.path().join("soon.wav");
    let cmd = Args::options([OsString::from("--add"), pending.clone().into()]).unwrap();
    let paths = cmd.add.unwrap();
    assert_eq!(paths, [pending.clone()]);
    finish_wav(&pending);
    assert_eq!(crate::add_work::classify(paths)[0].path, pending);
}

#[test]
fn refuses_the_whole_call_before_opening_a_profile() {
    let dir = tempfile::tempdir().unwrap();
    let song = file(dir.path(), "existing.flac");
    let path = song.to_str().unwrap();
    for words in [
        vec!["--add", "--playlist"],
        vec!["--add", "--playlist", "  ", path],
        vec!["--add", "--playlist", "Mix"],
        vec!["--add", "--playlist", "Mix", ""],
        vec!["--playlist", "Mix", path],
        vec!["--add", path, "--playlist", ""],
        vec!["--add", "--playlist", "A", path, "--playlist", "B", path],
        vec!["--quit", "--add", "--playlist", "Mix"],
    ] {
        assert!(
            args(&words).is_err_and(|e| e.is::<PlaylistUsage>()),
            "{words:?}"
        );
    }
}

#[test]
fn keeps_places_across_reopen_and_appends_once_to_legacy_entries() {
    let dir = tempfile::tempdir().unwrap();
    let a = file(dir.path(), "One.wav");
    let b = file(dir.path(), "Two.wav");
    let c = file(dir.path(), "Three.wav");
    let mut s = session(State::default());
    reserve(&mut s, " Études ", &[b.clone(), a.clone(), b.clone()]);
    let list = &s.field("playlists")[0];
    assert_eq!(list["name"], "Études");
    assert_eq!(list["items"][0]["id"], id(&b));
    assert!(list["items"][0]["at"].is_i64());
    assert!(visible(&s).is_empty());

    // A 1.x playlist can also have bare IDs and an untrimmed name.
    let mut state = s.state;
    let list = &mut state.fields["playlists"][0];
    list["name"] = " Études ".into();
    list["items"][1] = id(&a).into();
    let mut s = session(serde_json::from_value(serde_json::to_value(state).unwrap()).unwrap());
    reserve(&mut s, "ÉTUDES", &[a.clone(), c.clone(), b.clone()]);
    assert_eq!(s.models.playlists.len(), 1);
    assert_eq!(s.field("playlists")[0]["name"], " Études ");
    assert_eq!(s.field("playlists")[0]["items"][1], id(&a));

    // Scan A and C first. Once B has bytes it appears at its original place.
    finish_wav(&a);
    finish_wav(&c);
    let paths = [a.clone(), b.clone(), c.clone()];
    assert!(s.library.scan(&[], &paths).issues.is_empty());
    s.rebuild();
    assert_eq!(visible(&s), [id(&a), id(&c)]);
    finish_wav(&b);
    assert!(s.library.scan(&[], &paths).issues.is_empty());
    s.rebuild();
    assert_eq!(visible(&s), [id(&b), id(&a), id(&c)]);
    let before = s.field("playlists").clone();
    reserve(&mut s, "études", &paths);
    assert_eq!(s.field("playlists"), &before);
}

#[test]
fn scanned_folders_expand_at_their_argument_positions() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("album");
    fs::create_dir(&root).unwrap();
    let a = file(&root, "a.wav");
    let b = file(&root, "b.wav");
    let first = file(dir.path(), "first.wav");
    let last = file(dir.path(), "last.wav");
    let sibling = dir.path().join("album-extra");
    fs::create_dir(&sibling).unwrap();
    let other = file(&sibling, "another.wav");
    for p in [&a, &b, &first, &last, &other] {
        finish_wav(p);
    }
    let mut s = session(State::default());
    reserve(
        &mut s,
        "Mix",
        &[first.clone(), root.clone(), last.clone(), a.clone()],
    );
    let mut s = session(s.state);
    let report = s
        .library
        .scan(&[root.clone(), sibling], &[first.clone(), last.clone()]);
    s.rebuild();
    resolve_folders(&mut s, &report.complete_folders);
    assert_eq!(visible(&s), [id(&first), id(&a), id(&b), id(&last)]);
    reserve(&mut s, "Mix", &[root]);
    resolve_folders(&mut s, &report.complete_folders);
    assert_eq!(visible(&s), [id(&first), id(&a), id(&b), id(&last)]);
    assert_eq!(
        s.field("playlists")[0]["items"].as_array().unwrap().len(),
        4
    );
}

// A second process runs the normal parser and forwards to the parent's isolated instance.
#[test]
#[ignore = "started by named_add_crosses_processes"]
fn playlist_sender() {
    let Ok(dir) = std::env::var("STRING_PLAYLIST_TEST_DIR") else {
        return;
    };
    let prefix = std::env::var("STRING_PLAYLIST_TEST_PREFIX").unwrap();
    assert!(prefix.starts_with("PlaylistAddTest."));
    let names = Names::custom(&prefix);
    let args = Args::options([
        OsString::from("--add"),
        "--playlist".into(),
        " Études ".into(),
        Path::new(&dir).join("two.flac").into(),
        Path::new(&dir).join("one.flac").into(),
    ])
    .unwrap();
    let paths = args.add.as_ref().unwrap();
    // Both instances are real. The sender owns this second mutex, and the parent owns the window.
    let local = Names {
        add: None,
        ..Names::custom(&format!("{prefix}.Sender"))
    };
    let Claim::First(_local_instance) = instance::claim_quietly(&local).unwrap() else {
        panic!("expected an isolated sender mutex")
    };
    let mismatched = Names {
        mutex: local.mutex,
        ..names.clone()
    };
    assert!(!mismatched.send_paths(paths, Duration::ZERO));
    assert!(!instance::send_add(&mismatched, paths, Duration::ZERO));
    assert!(!mismatched.send_playlist_add("Wrong owner", paths, Duration::ZERO));
    assert!(matches!(
        claim(
            &names,
            args.playlist.as_deref(),
            paths,
            Duration::from_secs(5),
            || panic!("a forwarding process should not create an add worker"),
        )
        .unwrap(),
        Start::Done(0)
    ));
}

#[test]
fn named_add_crosses_processes() {
    let dir = tempfile::tempdir().unwrap();
    let one = file(dir.path(), "one.flac");
    let two = file(dir.path(), "two.flac");
    let prefix = format!("PlaylistAddTest.{}", std::process::id());
    let names = Names::custom(&prefix);
    let args = Args::options([
        OsString::from("--add"),
        "--playlist".into(),
        "Startup".into(),
        one.clone().into(),
    ])
    .unwrap();
    let mut adds = Adds::new().unwrap();
    let Start::Run(first) = claim(
        &names,
        Some("Startup"),
        args.add.as_ref().unwrap(),
        Duration::ZERO,
        || Ok(adds.input.clone()),
    )
    .unwrap() else {
        panic!("expected an isolated first instance")
    };
    assert!(first.add_error().is_none());
    let child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "playlist_add::tests::playlist_sender",
            "--ignored",
        ])
        .env("STRING_PLAYLIST_TEST_DIR", dir.path())
        .env("STRING_PLAYLIST_TEST_PREFIX", prefix)
        .output()
        .unwrap();
    assert!(
        child.status.success(),
        "{}",
        String::from_utf8_lossy(&child.stdout)
    );

    // The early request and the one accepted immediately before close must reach the saved profile.
    assert_eq!(adds.finish_startup().unwrap(), [one.clone()]);
    let three = file(dir.path(), "three.flac");
    assert!(names.send_paths(std::slice::from_ref(&three), Duration::ZERO));
    adds.set_closed(true);
    assert!(!names.send_paths(std::slice::from_ref(&two), Duration::ZERO));
    assert!(!names.send_playlist_add("Late", std::slice::from_ref(&two), Duration::ZERO));
    let folder = crate::storage::folder(&dir.path().join("profile"), true);
    let (mut store, mut s) =
        crate::storage::Storage::open(&folder, &egui::Context::default()).unwrap();
    let mut scan = crate::scan::Scanner::default();
    let deadline = Instant::now() + Duration::from_secs(5);
    while let Some(batch) = adds.next_at_exit(deadline) {
        batch.apply(&mut s, &mut scan);
    }
    assert!(scan.requested);
    assert!(!store.flush(&mut s).failed());
    let saved = folder.open().unwrap().state.value;
    assert_eq!(saved.fields["files"], json!([one, two, three]));
    let lists = saved.fields["playlists"].as_array().unwrap();
    assert_eq!(lists.len(), 2);
    assert_eq!(lists[0]["name"], "Startup");
    assert_eq!(lists[1]["name"], "Études");
    assert_eq!(lists[1]["items"][0]["id"], id(&two));
    assert_eq!(lists[1]["items"][1]["id"], id(&one));
}
