//! Filesystem regressions and a reproducible cold and cached 12,000-file scan.

use serde_json::json;
use std::{fs, path::PathBuf, time::Instant};
use string_core::{
    library::{Library, Track, track_id},
    state::State,
};
use tempfile::tempdir;

fn wave() -> Vec<u8> {
    let mut b = b"RIFF".to_vec();
    b.extend(52u32.to_le_bytes());
    b.extend(b"WAVEfmt ");
    b.extend(16u32.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(8000u32.to_le_bytes());
    b.extend(16000u32.to_le_bytes());
    b.extend(2u16.to_le_bytes());
    b.extend(16u16.to_le_bytes());
    b.extend(b"data");
    b.extend(16u32.to_le_bytes());
    b.extend([0; 16]);
    b
}

#[test]
fn cached_retry_removal_unavailable_root_hidden_and_loose() {
    let t = tempdir().unwrap();
    let root = t.path().join("Music");
    fs::create_dir(&root).unwrap();
    let path = root.join("MİX ΣΟΣ.WAV");
    fs::write(&path, wave()).unwrap();
    let loose = t.path().join("Single.wav");
    fs::write(&loose, wave()).unwrap();
    let mut l = Library::default();
    let loose = vec![loose];
    let locked = t.path().join("Unavailable");
    let roots = [root.clone(), locked.clone()];
    let r = l.scan(&roots, &loose);
    assert_eq!((r.read, r.issues.len()), (2, 1));
    let id = track_id(path.to_str().unwrap());
    let js = std::process::Command::new("node")
        .args(["-p", "process.argv[1].toLowerCase()"])
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(js.stdout).unwrap().trim_end(), id);
    let mut s = State::default();
    s.fields.insert("hidden".into(), json!({id.clone():1}));
    assert_eq!(l.visible(&s).count(), 1);
    let cached = l.tracks.iter_mut().find(|t| t.id() == id).unwrap();
    let stamp = (cached.size, cached.mtime);
    cached.read_error = 1;
    cached.tags.title = "temporary".into();
    l.tracks.push(Track {
        path: locked.join("known.mp3").to_str().unwrap().into(),
        ..Default::default()
    });
    l.tracks.push(Track {
        path: root.join("gone.mp3").to_str().unwrap().into(),
        ..Default::default()
    });
    fs::write(root.join("cover.jpg"), b"picture").unwrap();
    let r = l.scan(&roots, &loose);
    assert_eq!((r.read, r.reused, r.removed, l.tracks.len()), (1, 1, 1, 3));
    let cached = l.tracks.iter().find(|t| t.id() == id).unwrap();
    assert_eq!((cached.size, cached.mtime), stamp);
    assert_eq!(cached.read_error, 0);
    assert!(cached.tags.title.is_empty());
    assert_eq!(cached.tags.format, "wav");
    assert!(cached.cover.ends_with("cover.jpg"));
    fs::remove_file(root.join("cover.jpg")).unwrap();
    let r = l.scan(&roots, &loose);
    assert_eq!((r.read, r.covers_changed), (0, 1));
    fs::remove_file(&loose[0]).unwrap();
    assert_eq!(l.scan(&roots, &loose).removed, 1);
}
#[test]
fn rescan_follows_current_selection_and_skips_hidden_directories() {
    let t = tempdir().unwrap();
    let first = t.path().join("One");
    let second = t.path().join("Two");
    for root in [
        &first,
        &second,
        &first.join(".skip"),
        &first.join("node_modules"),
    ] {
        fs::create_dir(root).unwrap();
        fs::write(root.join("song.wav"), wave()).unwrap();
    }
    let mut l = Library::default();
    assert_eq!(l.scan(std::slice::from_ref(&first), &[]).read, 1);
    assert_eq!(l.scan(&[first, second], &[]).read, 1);
    assert_eq!(l.tracks.len(), 2);
}
#[test]
fn failures_from_reader_are_retried_even_when_file_is_unchanged() {
    let t = tempdir().unwrap();
    let path = t.path().join("unreadable.mp3");
    fs::write(&path, b"broken").unwrap();
    let roots = [t.path().to_owned()];
    let mut l = Library::default();
    for _ in 0..2 {
        let r = l.scan(&roots, &[]);
        assert_eq!((r.read, r.issues.len(), l.tracks[0].read_error), (1, 1, 1));
    }
    fs::write(&path, wave()).unwrap();
    let r = l.scan(&roots, &[]);
    assert_eq!((r.read, r.issues.len(), l.tracks[0].read_error), (1, 0, 0));
}
#[test]
#[ignore = "creates 12,000 temporary WAV files; run explicitly for a timing sample"]
fn twelve_thousand() {
    let t = tempdir().unwrap();
    let bytes = wave();
    for album in 0..120 {
        let d = t.path().join(format!("Album {album}"));
        fs::create_dir(&d).unwrap();
        for song in 0..100 {
            fs::write(d.join(format!("Track {song:03}.wav")), &bytes).unwrap();
        }
    }
    let roots: Vec<PathBuf> = vec![t.path().to_owned()];
    let mut l = Library::default();
    let start = Instant::now();
    let cold = l.scan(&roots, &[]);
    let cold_time = start.elapsed();
    let start = Instant::now();
    let warm = l.scan(&roots, &[]);
    let warm_time = start.elapsed();
    assert_eq!(
        (cold.read, cold.issues.len(), warm.reused, warm.read),
        (12000, 0, 12000, 0)
    );
    println!(
        "12,000 WAV files, {} rayon workers: cold {cold_time:?}, cached {warm_time:?}",
        rayon::current_num_threads()
    );
}
#[test]
fn a_cache_with_numbers_string_1x_accepts_still_opens() {
    // track and disc numbers past u32, a fractional year, a string, a null length: String 1.x saves what tags say
    let lib: string_core::library::Library = serde_json::from_value(json!({
        "v": 1,
        "tracks": [
            { "path": "C:\\m\\a.wav", "track": 4294967296u64, "disc": 1e20, "year": 2001.5, "duration": null },
            { "path": "C:\\m\\b.mp3", "track": "7", "disc": -3, "year": "x", "duration": 12.5 }
        ]
    }))
    .unwrap();
    let (a, b) = (&lib.tracks[0].tags, &lib.tracks[1].tags);
    assert_eq!((a.track, a.disc, a.year, a.duration), (u32::MAX, u32::MAX, 2001, 0.));
    assert_eq!((b.track, b.disc, b.year, b.duration), (7, 0, 0, 12.5));
}
