use super::*;
use crate::{Args, PlaylistUsage};
use std::{ffi::OsString, fs, path::Path, process::Command};
use string_core::{library::Library, state::State};

fn args(words: &[&str]) -> Result<Args, crate::Error> {
    Args::parse_from(words.iter().map(OsString::from))
}

fn session(state: State) -> Session {
    Session::new(state, Default::default(), Library::default())
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
    s.library
        .scan(&[root.clone(), sibling], &[first.clone(), last.clone()]);
    s.rebuild();
    resolve_folders(&mut s);
    assert_eq!(visible(&s), [id(&first), id(&a), id(&b), id(&last)]);
    reserve(&mut s, "Mix", &[root]);
    resolve_folders(&mut s);
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
    let args = Args::parse_from([
        OsString::from("--add"),
        "--playlist".into(),
        " Études ".into(),
        Path::new(&dir).join("two.flac").into(),
        Path::new(&dir).join("one.flac").into(),
    ])
    .unwrap();
    assert!(matches!(
        claim(
            &Names::custom(&prefix),
            args.playlist.as_deref().unwrap(),
            args.add.as_ref().unwrap(),
            Duration::from_secs(5)
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
    let mut args = Args::parse_from([
        OsString::from("--add"),
        "--playlist".into(),
        "Startup".into(),
        one.clone().into(),
    ])
    .unwrap();
    let Start::Run(mut first) = claim(
        &names,
        "Startup",
        args.add.as_ref().unwrap(),
        Duration::ZERO,
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

    // Delivery can precede both the window and the profile. Cold and warm requests emerge in order.
    let (tx, wake) = mpsc::channel();
    first.listen(move |s| tx.send(s).unwrap());
    assert_eq!(
        wake.recv_timeout(Duration::from_secs(2)),
        Ok(instance::Signal::Add(vec![]))
    );
    args.root = dir.path().to_owned();
    let received = first.take_playlist_adds();
    let log = string_platform::errors_log::ErrorLog::new(&args.root);
    let mut platform = Platform::new(first, &args, std::sync::Arc::new(log));
    let mut requests = Requests::new(args.playlist.take(), &mut platform, received);
    assert!(platform.take_added().is_empty());
    assert_eq!(
        requests.drain().collect::<Vec<_>>(),
        [
            ("Startup".into(), vec![one.clone()]),
            ("Études".into(), vec![two, one]),
        ]
    );
    assert_eq!(requests.drain().count(), 0);
}
