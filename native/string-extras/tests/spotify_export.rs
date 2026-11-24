use chrono::{DateTime, Local, NaiveDate, Timelike, Utc};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::Instant,
};
use string_core::data::{DataFolder, SpotifyHistory};
use string_extras::spotify::*;
use tempfile::tempdir;

fn row(ts: &str, n: u32, ms: i64) -> Value {
    json!({
        "ts": ts,
        "ms_played": ms,
        "master_metadata_track_name": format!("Song {n}"),
        "master_metadata_album_artist_name": "Artist A",
        "master_metadata_album_album_name": "Album A",
        "spotify_track_uri": format!("spotify:track:id{n}"),
        "ip_addr": "203.0.113.7",
        "conn_country": "ZZ",
        "platform": "test-device",
    })
}
fn json_bytes(rows: &[Value]) -> Vec<u8> {
    serde_json::to_vec(rows).unwrap()
}
fn two_plays() -> Vec<Value> {
    vec![
        row("2021-05-01T10:00:00Z", 1, 200_000),
        row("2021-05-02T11:00:00Z", 2, 100_000),
    ]
}
fn zip_of(path: &Path, entries: &[(&str, Vec<u8>)]) {
    let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
    for (name, bytes) in entries {
        zip.start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap();
}
fn run(paths: &[PathBuf], into: &mut SpotifyHistory) -> Result<Summary, ImportError> {
    let sources = find_history(paths).unwrap();
    import(&sources, into, |_, _| {})
}
const AUDIO: &str = "Streaming_History_Audio_2021_0.json";

#[test]
fn reads_the_zip_the_folder_and_loose_files_alike() {
    let tmp = tempdir().unwrap();
    let bytes = json_bytes(&two_plays());
    let zip = tmp.path().join("export.zip");
    zip_of(
        &zip,
        &[(
            &format!("Spotify Extended Streaming History/{AUDIO}"),
            bytes.clone(),
        )],
    );
    let folder = tmp.path().join("folder");
    fs::create_dir_all(folder.join("Spotify Extended Streaming History")).unwrap();
    fs::write(
        folder
            .join("Spotify Extended Streaming History")
            .join(AUDIO),
        &bytes,
    )
    .unwrap();
    let loose = tmp.path().join(AUDIO);
    fs::write(&loose, &bytes).unwrap();

    let mut totals = vec![];
    for path in [zip, folder, loose] {
        let mut h = SpotifyHistory::default();
        let s = run(&[path], &mut h).unwrap();
        assert_eq!((s.added, s.skipped, s.files), (2, 0, 1));
        assert!((s.seconds - 300.).abs() < 1e-9);
        totals.push((s.added, s.seconds as u64, h.meta().unwrap().plays));
    }
    assert!(totals.windows(2).all(|w| w[0] == w[1]));
}

#[test]
fn finds_history_one_folder_up_and_in_nested_zip_paths() {
    let tmp = tempdir().unwrap();
    let bytes = json_bytes(&two_plays());
    let deep = tmp
        .path()
        .join("Downloads/my_spotify_data/Spotify Extended Streaming History");
    fs::create_dir_all(&deep).unwrap();
    fs::write(deep.join(AUDIO), &bytes).unwrap();
    fs::write(deep.join("Streaming_History_Video_2021.json"), b"[]").unwrap();
    fs::write(deep.join("ReadMeFirst_ExtendedStreamingHistory.pdf"), b"x").unwrap();
    let found = find_history(&[tmp.path().join("Downloads")]).unwrap();
    assert_eq!(found.len(), 1);
    assert!(is_history(&tmp.path().join("Downloads/my_spotify_data")));

    let zip = tmp.path().join("a.zip");
    zip_of(
        &zip,
        &[
            (&format!("my_spotify_data/x/{AUDIO}"), bytes.clone()),
            ("Streaming_History_Video_1.json", b"[]".to_vec()),
        ],
    );
    let found = find_history(&[zip.clone()]).unwrap();
    assert_eq!(found.len(), 1);
    assert!(matches!(&found[0], Source::ZipEntry { name, .. } if name.ends_with(AUDIO)));
    assert!(is_history(&zip));
    assert!(!is_history(&deep.join("Streaming_History_Video_2021.json")));
    assert!(find_history(&[tmp.path().join("nothing.zip")]).is_err());
    let empty = tmp.path().join("empty");
    fs::create_dir(&empty).unwrap();
    assert!(find_history(&[empty]).unwrap().is_empty());
}

#[test]
fn holds_finds_history_as_find_history_does_and_songs_as_the_scan_does() {
    let tmp = tempdir().unwrap();
    let bytes = json_bytes(&two_plays());
    // A music folder: albums of songs and a playlist file, no history anywhere.
    let music = tmp.path().join("Music");
    for artist in 0..6 {
        for album in 0..3 {
            let dir = music.join(format!("Artist {artist}/Album {album}"));
            fs::create_dir_all(&dir).unwrap();
            for n in 0..8 {
                fs::write(dir.join(format!("{n:02} song.mp3")), b"").unwrap();
            }
            fs::write(dir.join("tracks.json"), b"[]").unwrap();
        }
    }
    // History four folders below the one dropped is found (as `find_history` finds it); five folders down isn't,
    // though a song beside it is, as the library scan reaches it.
    let near = tmp.path().join("near/a/b/c/d");
    let far = tmp.path().join("far/a/b/c/d/e");
    for dir in [&near, &far] {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join(AUDIO), &bytes).unwrap();
    }
    fs::write(far.join("deep song.flac"), b"").unwrap();
    let song = music.join("Artist 0/Album 0/00 song.mp3");
    let loud = tmp.path().join("LOUD.FLAC");
    fs::write(&loud, b"").unwrap();
    let playlist = music.join("Artist 0/Album 0/tracks.json");
    // History beside songs, after the albums in the listing.
    let mixed = music.join("Artist 5/zz export");
    fs::create_dir_all(&mixed).unwrap();
    fs::write(mixed.join(AUDIO), &bytes).unwrap();
    let videos = tmp.path().join("videos.zip");
    zip_of(
        &videos,
        &[("Streaming_History_Video_1.json", b"[]".to_vec())],
    );
    let broken = tmp.path().join("broken.zip");
    fs::write(&broken, b"not a zip").unwrap();
    // Dropped, then moved away before it was looked at.
    let missing = tmp.path().join("missing.zip");

    // (history, music)
    let cases = [
        (music.join("Artist 0"), (false, true)),
        (tmp.path().join("near"), (true, false)),
        (tmp.path().join("far"), (false, true)),
        // Songs with an export among them: both.
        (music.clone(), (true, true)),
        (videos, (false, false)),
        (broken.clone(), (false, false)),
        (near.join(AUDIO), (true, false)),
        (song, (false, true)),
        (loud, (false, true)),
        (playlist, (false, false)),
        (missing.clone(), (false, false)),
    ];
    for (path, (history, music)) in cases {
        let found = holds(&path);
        assert_eq!((found.history, found.music), (history, music), "{path:?}");
        assert_eq!(is_history(&path), history, "{path:?}");
        // Only the broken zip and the missing one can't be read; `find_history` refuses both.
        assert_eq!(
            found.unreadable,
            path == broken || path == missing,
            "{path:?}"
        );
        if path != broken && path.exists() {
            assert_eq!(
                !find_history(&[path.clone()]).unwrap().is_empty(),
                history,
                "{path:?}"
            );
        }
    }
    assert!(find_history(&[broken]).is_err());
    assert!(find_history(&[missing]).is_err());
}

#[test]
fn skips_video_podcast_and_audiobook_rows() {
    let tmp = tempdir().unwrap();
    let mut rows = two_plays();
    rows.push(json!({"ts": "2021-05-03T10:00:00Z", "ms_played": 5000, "master_metadata_track_name": null,
        "spotify_track_uri": null, "episode_name": "Ep 1", "spotify_episode_uri": "spotify:episode:e1"}));
    let mut book = row("2021-05-03T11:00:00Z", 3, 5000);
    book["spotify_track_uri"] = json!("spotify:audiobook:b1");
    rows.push(book);
    rows.push(row("2021-05-03T12:00:00Z", 4, 0));
    rows.push(row("not a time", 5, 1000));
    let mut unnamed = row("2021-05-03T13:00:00Z", 6, 1000);
    unnamed["master_metadata_track_name"] = json!("");
    rows.push(unnamed);
    let file = tmp.path().join(AUDIO);
    fs::write(&file, json_bytes(&rows)).unwrap();
    let mut h = SpotifyHistory::default();
    let s = run(&[file], &mut h).unwrap();
    assert_eq!((s.added, s.skipped), (2, 5));
}

#[test]
fn never_stores_personal_fields() {
    let tmp = tempdir().unwrap();
    let file = tmp.path().join(AUDIO);
    fs::write(&file, json_bytes(&two_plays())).unwrap();
    let folder = DataFolder::new(tmp.path().join("data"), tmp.path().join("backup"));
    let mut doc = folder.read::<SpotifyHistory>().unwrap();
    run(&[file], &mut doc.value).unwrap();
    doc.save().unwrap();
    let saved = fs::read_to_string(tmp.path().join("data/spotify.json")).unwrap();
    assert!(saved.contains("Song 1"));
    for private in ["203.0.113.7", "ZZ", "test-device"] {
        assert!(!saved.contains(private), "{private}");
    }
}

#[test]
fn converts_utc_to_local_time() {
    let tmp = tempdir().unwrap();
    // Instants on both sides of a spring and an autumn clock change, plus one late in a UTC day.
    let stamps = [
        "2021-03-28T00:30:00Z",
        "2021-03-28T02:30:00Z",
        "2021-10-31T00:30:00Z",
        "2021-10-31T01:30:00Z",
        "2021-07-01T23:30:00Z",
    ];
    let rows: Vec<Value> = stamps
        .iter()
        .enumerate()
        .map(|(i, ts)| row(ts, i as u32, 60_000))
        .collect();
    let file = tmp.path().join(AUDIO);
    fs::write(&file, json_bytes(&rows)).unwrap();
    let mut h = SpotifyHistory::default();
    run(&[file], &mut h).unwrap();
    let mut want: BTreeMap<(NaiveDate, usize), f64> = BTreeMap::new();
    for ts in stamps {
        let local = DateTime::parse_from_rfc3339(ts)
            .unwrap()
            .with_timezone(&Local)
            .naive_local();
        *want
            .entry((local.date(), local.hour() as usize))
            .or_default() += 60.;
    }
    let days = h.0.days();
    let got_days: BTreeSet<NaiveDate> = days.keys().copied().collect();
    assert_eq!(got_days, want.keys().map(|k| k.0).collect());
    for (date, day) in days {
        let mut total = 0.;
        for (hour, secs) in day.h.iter().enumerate() {
            let expected = want.get(&(*date, hour)).copied().unwrap_or(0.);
            assert!((secs - expected).abs() < 1e-6, "{date} {hour}h");
            total += expected;
        }
        assert!((day.s - total).abs() < 1e-6, "{date}");
    }
}

#[test]
fn counts_local_file_plays() {
    let tmp = tempdir().unwrap();
    let mut local = row("2021-05-01T10:00:00Z", 1, 90_000);
    local["spotify_track_uri"] = json!("spotify:local:Artist+A:Album:Song+1:90");
    let file = tmp.path().join(AUDIO);
    fs::write(&file, json_bytes(&[local])).unwrap();
    let mut h = SpotifyHistory::default();
    let s = run(&[file], &mut h).unwrap();
    assert_eq!((s.added, s.skipped), (1, 0));
}

#[test]
fn a_damaged_file_refuses_the_whole_import() {
    let tmp = tempdir().unwrap();
    let good = tmp.path().join("Streaming_History_Audio_2020_0.json");
    fs::write(&good, json_bytes(&two_plays())).unwrap();
    let bad = tmp.path().join("Streaming_History_Audio_2021_1.json");
    let mut bytes = json_bytes(&two_plays());
    bytes.truncate(bytes.len() / 2);
    fs::write(&bad, bytes).unwrap();
    let mut h = SpotifyHistory::default();
    let before = h.meta();
    let err = run(&[tmp.path().to_path_buf()], &mut h).unwrap_err();
    match &err {
        ImportError::Unreadable { file, .. } => {
            assert_eq!(file, "Streaming_History_Audio_2021_1.json")
        }
        other => panic!("{other:?}"),
    }
    assert!(
        err.to_string()
            .contains("Streaming_History_Audio_2021_1.json")
    );
    assert_eq!(h.meta(), before);
    assert_eq!(h.0.track_count(), 0);

    fs::write(&bad, b"{\"not\": \"an array\"}").unwrap();
    assert!(matches!(
        run(&[bad], &mut h),
        Err(ImportError::Unreadable { .. })
    ));
    assert_eq!(h.0.track_count(), 0);
}

#[test]
fn no_history_is_reported() {
    let mut h = SpotifyHistory::default();
    assert!(matches!(
        import(&[], &mut h, |_, _| {}),
        Err(ImportError::NoHistory)
    ));
}

#[test]
fn the_same_export_twice_adds_nothing() {
    let tmp = tempdir().unwrap();
    let file = tmp.path().join(AUDIO);
    fs::write(&file, json_bytes(&two_plays())).unwrap();
    let mut h = SpotifyHistory::default();
    let first = run(&[file.clone()], &mut h).unwrap();
    let t = DateTime::parse_from_rfc3339("2021-05-02T11:00:00Z")
        .unwrap()
        .timestamp_millis();
    assert_eq!(first.through, t);
    assert_eq!(h.meta().unwrap().through, t);
    assert_eq!((h.meta().unwrap().plays, h.meta().unwrap().files), (2, 1));
    assert!(h.meta().unwrap().imported > 0);
    assert!(matches!(run(&[file], &mut h), Err(ImportError::NothingNew)));
    assert_eq!(h.meta().unwrap().plays, 2);
}

#[test]
fn a_newer_export_adds_only_newer_plays() {
    let tmp = tempdir().unwrap();
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    fs::create_dir_all(&a).unwrap();
    fs::create_dir_all(&b).unwrap();
    fs::write(a.join(AUDIO), json_bytes(&two_plays())).unwrap();
    let mut rows = two_plays();
    rows.push(row("2021-06-01T10:00:00Z", 3, 60_000));
    rows.push(row("2021-06-02T10:00:00Z", 4, 60_000));
    fs::write(b.join(AUDIO), json_bytes(&rows)).unwrap();
    let mut h = SpotifyHistory::default();
    run(&[a], &mut h).unwrap();
    let s = run(&[b], &mut h).unwrap();
    assert_eq!((s.added, s.skipped), (2, 0));
    assert_eq!(h.meta().unwrap().plays, 4);
}

#[test]
fn a_newer_export_keeps_links_set_by_hand() {
    let tmp = tempdir().unwrap();
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    fs::create_dir_all(&a).unwrap();
    fs::create_dir_all(&b).unwrap();
    fs::write(a.join(AUDIO), json_bytes(&two_plays())).unwrap();
    let mut rows = two_plays();
    rows.push(row("2021-06-01T10:00:00Z", 3, 60_000));
    fs::write(b.join(AUDIO), json_bytes(&rows)).unwrap();
    let mut h = SpotifyHistory::default();
    run(&[a], &mut h).unwrap();
    let links =
        json!([{"title": "Song 1", "artist": "Artist A", "as": {"title": "Song", "artist": "B"}}]);
    h.0.set_top_field("sameAs", links.clone());
    run(&[b], &mut h).unwrap();
    assert_eq!(h.0.top_field("sameAs"), Some(&links));
}

#[test]
fn the_newest_play_names_a_song_whatever_order_the_files_list_it_in() {
    let tmp = tempdir().unwrap();
    let named = |ts: &str, title: &str, album: &str| {
        json!({
            "ts": ts,
            "ms_played": 60_000,
            "master_metadata_track_name": title,
            "master_metadata_album_artist_name": "Artist A",
            "master_metadata_album_album_name": album,
            "spotify_track_uri": "spotify:track:renamed",
        })
    };
    let rows = vec![
        named("2021-05-02T10:00:00Z", "New Name", "New Release"),
        named("2021-05-01T10:00:00Z", "Old Name", "First Release"),
    ];
    fs::write(tmp.path().join(AUDIO), json_bytes(&rows)).unwrap();
    let mut h = SpotifyHistory::default();
    run(&[tmp.path().join(AUDIO)], &mut h).unwrap();
    let t = h.0.tracks().next().unwrap();
    assert_eq!(t.1, "New Name");
    assert_eq!(h.0.album("spotify:track:renamed"), Some("New Release"));
}

#[test]
fn an_older_export_adds_nothing() {
    let tmp = tempdir().unwrap();
    let new = tmp.path().join("new");
    let old = tmp.path().join("old");
    fs::create_dir_all(&new).unwrap();
    fs::create_dir_all(&old).unwrap();
    fs::write(new.join(AUDIO), json_bytes(&two_plays())).unwrap();
    fs::write(
        old.join(AUDIO),
        json_bytes(&[row("2020-01-01T10:00:00Z", 9, 60_000)]),
    )
    .unwrap();
    let mut h = SpotifyHistory::default();
    run(&[new], &mut h).unwrap();
    let meta = h.meta();
    assert!(matches!(run(&[old], &mut h), Err(ImportError::NothingNew)));
    assert_eq!(h.meta(), meta);
}

#[test]
fn imports_170k_plays_quickly() {
    let tmp = tempdir().unwrap();
    let start = DateTime::parse_from_rfc3339("2015-01-01T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    for file in 0..12u32 {
        let rows: Vec<Value> = (0..14_167u32)
            .map(|i| {
                let n = file * 14_167 + i;
                let ts = start + chrono::Duration::minutes(n as i64 * 4);
                row(
                    &ts.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
                    n % 3000,
                    180_000,
                )
            })
            .collect();
        fs::write(
            tmp.path()
                .join(format!("Streaming_History_Audio_{file:02}.json")),
            json_bytes(&rows),
        )
        .unwrap();
    }
    let t = Instant::now();
    let mut h = SpotifyHistory::default();
    let s = run(&[tmp.path().to_path_buf()], &mut h).unwrap();
    let took = t.elapsed();
    assert_eq!(s.added, 12 * 14_167);
    assert!(took.as_secs_f64() < 15., "{took:?}");
    eprintln!("170k import took {took:?}");
}

#[test]
fn the_same_file_reached_several_ways_counts_once() {
    let tmp = tempdir().unwrap();
    let bytes = json_bytes(&two_plays());
    let folder = tmp.path().join("export");
    fs::create_dir(&folder).unwrap();
    fs::write(folder.join(AUDIO), &bytes).unwrap();
    let zip = tmp.path().join("export.zip");
    zip_of(&zip, &[(AUDIO, bytes)]);

    let paths = [
        folder.clone(),
        folder.join(AUDIO),
        folder.clone(),
        zip.clone(),
        zip,
    ];
    let found = find_history(&paths).unwrap();
    assert_eq!(found.len(), 2); // the file once, the zip entry once
    let mut h = SpotifyHistory::default();
    let s = import(&found, &mut h, |_, _| {}).unwrap();
    assert_eq!((s.added, s.skipped), (2, 0));
    assert_eq!(h.meta().unwrap().plays, 2);
}

#[test]
fn a_corrupt_zip_next_to_a_good_folder_refuses_the_import() {
    let tmp = tempdir().unwrap();
    let folder = tmp.path().join("export");
    fs::create_dir(&folder).unwrap();
    fs::write(folder.join(AUDIO), json_bytes(&two_plays())).unwrap();
    let zip = tmp.path().join("broken.zip");
    fs::write(&zip, b"this is not a zip").unwrap();
    let err = find_history(&[folder, zip]).unwrap_err();
    assert!(err.to_string().contains("broken.zip"), "{err}");
}

#[test]
fn a_history_whose_import_record_is_damaged_refuses_a_reimport() {
    let tmp = tempdir().unwrap();
    let file = tmp.path().join(AUDIO);
    fs::write(&file, json_bytes(&two_plays())).unwrap();
    let folder = DataFolder::new(tmp.path().join("data"), tmp.path().join("backup"));
    let mut doc = folder.read::<SpotifyHistory>().unwrap();
    run(&[file.clone()], &mut doc.value).unwrap();
    doc.save().unwrap();
    let saved = tmp.path().join("data/spotify.json");
    let good: Value = serde_json::from_slice(&fs::read(&saved).unwrap()).unwrap();
    // The record gone, its cursor gone, or a cursor of the wrong type.
    for record in [
        None,
        Some(json!({"plays": 2, "imported": 1, "files": 1})),
        Some(json!({"through": "soon", "plays": 2, "imported": 1, "files": 1})),
    ] {
        let mut damaged = good.clone();
        match record {
            None => {
                damaged.as_object_mut().unwrap().remove("spotify");
            }
            Some(record) => damaged["spotify"] = record,
        }
        fs::write(&saved, serde_json::to_vec(&damaged).unwrap()).unwrap();
        let mut doc = folder.read::<SpotifyHistory>().unwrap();
        let before = serde_json::to_string(&doc.value).unwrap();
        let err = run(&[file.clone()], &mut doc.value).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Your saved Spotify history's import record is damaged, so nothing was imported."
        );
        assert_eq!(serde_json::to_string(&doc.value).unwrap(), before);
    }
}

#[test]
fn songs_count_as_deep_as_the_library_scan_reaches() {
    let tmp = tempdir().unwrap();
    let nested = |root: &Path, levels: usize| {
        (0..levels).fold(root.to_owned(), |p, i| p.join(format!("d{i}")))
    };
    // History at the top of the folder, its only song fourteen folders down: the scan's deepest.
    let mixed = tmp.path().join("mixed");
    let deep = nested(&mixed, 14);
    fs::create_dir_all(&deep).unwrap();
    fs::write(mixed.join(AUDIO), json_bytes(&two_plays())).unwrap();
    fs::write(deep.join("track.flac"), b"").unwrap();
    // A song one folder deeper than the scan goes.
    let beyond = tmp.path().join("beyond");
    fs::create_dir_all(nested(&beyond, 15)).unwrap();
    fs::write(nested(&beyond, 15).join("track.flac"), b"").unwrap();
    // Songs only in folders the scan skips.
    let skipped = tmp.path().join("skipped");
    for name in [".hidden", "$RECYCLE.BIN", "node_modules", "__MACOSX"] {
        fs::create_dir_all(skipped.join(name)).unwrap();
        fs::write(skipped.join(name).join("track.mp3"), b"").unwrap();
    }
    for (path, want) in [
        (mixed, (true, true)),
        (beyond, (false, false)),
        (skipped, (false, false)),
    ] {
        let found = holds(&path);
        assert_eq!((found.history, found.music), want, "{path:?}");
    }
}

fn replaced(bytes: &[u8], from: &[u8], to: &[u8]) -> (Vec<u8>, usize) {
    let mut out = Vec::with_capacity(bytes.len());
    let mut count = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(from) {
            out.extend_from_slice(to);
            i += from.len();
            count += 1;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    (out, count)
}

#[test]
fn a_zip_holding_two_files_of_one_name_is_refused() {
    let tmp = tempdir().unwrap();
    let zip = tmp.path().join("export.zip");
    let other = "Streaming_History_Audio_2021_9.json";
    zip_of(
        &zip,
        &[
            (AUDIO, json_bytes(&[row("2020-01-01T10:00:00Z", 9, 60_000)])),
            (other, json_bytes(&two_plays())),
        ],
    );
    // The second entry renamed to the first's name, in its own header and in the zip's list alike.
    let (bytes, count) = replaced(&fs::read(&zip).unwrap(), other.as_bytes(), AUDIO.as_bytes());
    assert_eq!(count, 2);
    fs::write(&zip, bytes).unwrap();
    let err = find_history(&[zip.clone()]).unwrap_err();
    assert!(err.to_string().contains("export.zip"), "{err}");
    // A drop of it goes to the import, which refuses it, as for any zip that can't be read.
    assert!(holds(&zip).unreadable);
}

#[test]
fn a_link_that_cant_be_followed_is_unreadable_within_the_history_search_only() {
    let tmp = tempdir().unwrap();
    // Links to a folder that's gone, as when the drive it was on is unplugged.
    let root = tmp.path().join("root");
    let four = ["a", "b", "c", "d"]
        .iter()
        .fold(root.clone(), |p, n| p.join(n));
    let near = four.join("near");
    let far = four.join("e").join("far");
    fs::create_dir_all(four.join("e")).unwrap();
    let target = tmp.path().join("gone");
    fs::create_dir(&target).unwrap();
    for at in [&near, &far] {
        let made = std::process::Command::new("cmd")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(at)
            .arg(&target)
            .output()
            .unwrap();
        assert!(made.status.success());
    }
    fs::remove_dir(&target).unwrap();
    // `near` is in a folder four down, which the search lists; given alone, a link is refused too.
    assert!(holds(&root).unreadable);
    assert!(find_history(&[root.clone()]).is_err());
    assert!(holds(&near).unreadable);
    fs::remove_dir(&near).unwrap();
    // `far` is in a folder five down: beyond the search.
    assert!(!holds(&root).unreadable);
    assert!(find_history(&[root]).unwrap().is_empty());
}

#[test]
fn an_import_beyond_its_limits_is_refused_before_anything_changes() {
    let tmp = tempdir().unwrap();
    let bytes = json_bytes(&two_plays());
    let loose = tmp.path().join(AUDIO);
    fs::write(&loose, &bytes).unwrap();
    let zip = tmp.path().join("export.zip");
    zip_of(&zip, &[(AUDIO, bytes.clone())]);
    let size = bytes.len() as u64;
    for path in [loose, zip] {
        let sources = find_history(&[path.clone()]).unwrap();
        let mut h = SpotifyHistory::default();
        let small = Limits {
            file_bytes: size - 1,
            ..Limits::default()
        };
        let err = import_within(&sources, &mut h, small, |_, _| {}).unwrap_err();
        assert!(
            matches!(&err, ImportError::Unreadable { file, .. } if file == AUDIO),
            "{path:?}: {err:?}"
        );
        assert_eq!((h.0.track_count(), h.0.days().len()), (0, 0));
        let exact = Limits {
            file_bytes: size,
            ..Limits::default()
        };
        import_within(&sources, &mut h, exact, |_, _| {}).unwrap();
    }
    // Four plays in two files, against a limit of three.
    let more = tmp.path().join("more");
    fs::create_dir(&more).unwrap();
    fs::write(more.join("Streaming_History_Audio_2021_0.json"), &bytes).unwrap();
    fs::write(
        more.join("Streaming_History_Audio_2021_1.json"),
        json_bytes(&[
            row("2021-06-01T10:00:00Z", 3, 60_000),
            row("2021-06-02T10:00:00Z", 4, 60_000),
        ]),
    )
    .unwrap();
    let sources = find_history(&[more]).unwrap();
    let mut h = SpotifyHistory::default();
    let three = Limits {
        plays: 3,
        ..Limits::default()
    };
    let err = import_within(&sources, &mut h, three, |_, _| {}).unwrap_err();
    assert_eq!(
        err.to_string(),
        "That holds more than 5,000,000 plays, more than String can import, so nothing was imported."
    );
    assert_eq!((h.0.track_count(), h.0.days().len()), (0, 0));
    let four = Limits {
        plays: 4,
        ..Limits::default()
    };
    assert_eq!(
        import_within(&sources, &mut h, four, |_, _| {})
            .unwrap()
            .added,
        4
    );
}
