use chrono::{DateTime, Local, NaiveDate, Utc};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
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
        totals.push((s.added, s.seconds as u64, h.meta().plays));
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
    let want: BTreeSet<NaiveDate> = stamps
        .iter()
        .map(|ts| {
            DateTime::parse_from_rfc3339(ts)
                .unwrap()
                .with_timezone(&Local)
                .date_naive()
        })
        .collect();
    let got: BTreeSet<NaiveDate> = h.0.days().keys().copied().collect();
    assert_eq!(got, want);
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
    assert_eq!(h.meta().through, t);
    assert_eq!((h.meta().plays, h.meta().files), (2, 1));
    assert!(h.meta().imported > 0);
    assert!(matches!(run(&[file], &mut h), Err(ImportError::NothingNew)));
    assert_eq!(h.meta().plays, 2);
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
    assert_eq!(h.meta().plays, 4);
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
