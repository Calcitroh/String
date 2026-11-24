//! The Spotify listening layer: long plays, merging into the phone-style layer, its saved file, setting a file aside.

use chrono::{Local, NaiveDate};
use serde_json::json;
use std::fs;
use string_core::{
    data::{DataFolder, SpotifyHistory, SpotifyMeta},
    stats::{Listening, Track},
};
use tempfile::tempdir;

fn day(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}
fn at(date: NaiveDate) -> chrono::NaiveDateTime {
    date.and_hms_opt(12, 0, 0).unwrap()
}
fn track(id: &str, artist: &str) -> Track {
    Track(id.into(), "Song".into(), artist.into())
}

#[test]
fn imported_plays_keep_their_album_and_allow_long_plays() {
    let mut layer = Listening::default();
    let t = track("spotify:track:x", "Band");
    assert!(layer.add_imported(&t, 3_600., at(day(2020, 3, 1)), "Album"));
    assert_eq!(layer.album("spotify:track:x"), Some("Album"));
    assert!(!layer.add_imported(&t, 20_000., at(day(2020, 3, 2)), "Album"));
    assert_eq!(layer.tracks().count(), 1);
}

#[test]
fn a_renamed_song_keeps_its_names_and_album_from_the_same_play() {
    let mut layer = Listening::default();
    let old = Track("spotify:track:x".into(), "Old Name".into(), "Band".into());
    let new = Track("spotify:track:x".into(), "New Name".into(), "Band".into());
    assert!(layer.add_imported(&old, 10., at(day(2020, 3, 1)), "First Release"));
    assert!(layer.add_imported(&new, 20., at(day(2020, 3, 2)), "New Release"));
    let t = layer.tracks().next().unwrap();
    assert_eq!(t.1, "New Name");
    assert_eq!(layer.album("spotify:track:x"), Some("New Release"));
}

fn pair() -> (Listening, Listening) {
    let mut own = Listening::default();
    own.add(&track("a.mp3", "Band"), 30., at(day(2026, 11, 20)));
    own.add(&track("a.mp3", "Band"), 30., at(day(2026, 11, 20)));
    let mut layer = Listening::default();
    layer.add_imported(&track("a.mp3", "Band"), 120., at(day(2026, 11, 20)), "");
    layer.add_imported(&track("spotify:track:x", "Other"), 30., at(day(2020, 3, 1)), "Album");
    (own, layer)
}

#[test]
fn merged_sums_both_and_marks_spotify_seconds() {
    let (own, layer) = pair();
    let merged = own.merged(&layer);
    let range = merged.range(day(2020, 1, 1), day(2026, 12, 31));
    assert_eq!(range.total, 210.);
    assert_eq!(range.spotify, 150.);
    assert_eq!(range.tracks[0].0.0, "a.mp3");
    assert_eq!(range.tracks[0].1, 180.);
    assert_eq!(merged.first_day(), Some(day(2020, 3, 1)));
}

#[test]
fn merged_keeps_the_saved_shape() {
    let (own, layer) = pair();
    let before = serde_json::to_string(&own).unwrap();
    let layer_before = serde_json::to_string(&layer).unwrap();
    let merged = own.merged(&layer);
    assert_eq!(serde_json::to_string(&own).unwrap(), before);
    assert_eq!(serde_json::to_string(&layer).unwrap(), layer_before);
    assert!(!serde_json::to_string(&merged).unwrap().contains("\"sp\""));
}

#[test]
fn spotify_history_round_trips_its_meta() {
    let dir = tempdir().unwrap();
    let folder = DataFolder::new(dir.path(), dir.path().join("backup"));
    let mut doc = folder.read::<SpotifyHistory>().unwrap();
    let meta = SpotifyMeta {
        through: 1_700_000_000_000,
        plays: 12,
        imported: 1_700_000_100_000,
        files: 3,
    };
    doc.value.set_meta(meta.clone());
    doc.value.0.set_top_field("future", json!({"keep": true}));
    doc.save().unwrap();
    let back = folder.read::<SpotifyHistory>().unwrap();
    assert_eq!(back.value.meta(), Some(meta));
    assert_eq!(back.value.0.top_field("future"), Some(&json!({"keep": true})));
}

#[test]
fn set_aside_moves_the_file_into_the_backup_folder() {
    let dir = tempdir().unwrap();
    let backup = dir.path().join("backup");
    let folder = DataFolder::new(dir.path(), &backup);
    assert_eq!(folder.set_aside::<SpotifyHistory>("1").unwrap(), None);
    fs::write(dir.path().join("spotify.json"), b"{\"v\":1}").unwrap();
    let moved = folder.set_aside::<SpotifyHistory>("1").unwrap().unwrap();
    assert!(!dir.path().join("spotify.json").exists());
    assert_eq!(moved, backup.join("spotify-removed-1.json"));
    assert_eq!(fs::read(&moved).unwrap(), b"{\"v\":1}");
    assert_eq!(folder.set_aside::<SpotifyHistory>("1").unwrap(), None);
    fs::write(dir.path().join("spotify.json"), b"{\"v\":1}").unwrap();
    let again = folder.set_aside::<SpotifyHistory>("1").unwrap_err();
    assert_eq!(again.kind(), std::io::ErrorKind::AlreadyExists);
    assert!(dir.path().join("spotify.json").exists());
}

#[test]
fn the_opening_copy_includes_the_spotify_file() {
    let dir = tempdir().unwrap();
    let backup = dir.path().join("backup");
    let folder = DataFolder::new(dir.path(), &backup);
    fs::write(dir.path().join("spotify.json"), b"{\"v\":1}").unwrap();
    folder.keep_opening_copy().unwrap();
    let kept = backup.join("opened").join(Local::now().date_naive().to_string());
    assert_eq!(fs::read(kept.join("spotify.json")).unwrap(), b"{\"v\":1}");
}

#[test]
fn set_aside_refusing_an_existing_target_leaves_both_files_alone() {
    let dir = tempdir().unwrap();
    let backup = dir.path().join("backup");
    fs::create_dir_all(&backup).unwrap();
    fs::write(backup.join("spotify-removed-2.json"), b"old").unwrap();
    fs::write(dir.path().join("spotify.json"), b"new").unwrap();
    let folder = DataFolder::new(dir.path(), &backup);
    let err = folder.set_aside::<SpotifyHistory>("2").unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(fs::read(backup.join("spotify-removed-2.json")).unwrap(), b"old");
    assert_eq!(fs::read(dir.path().join("spotify.json")).unwrap(), b"new");
}

#[test]
fn remap_sums_colliding_tracks_and_rebuilds_artists() {
    let mut layer = Listening::default();
    let when = day(2020, 3, 1).and_hms_opt(9, 0, 0).unwrap();
    layer.add_imported(&track("spotify:track:x", "Artist A"), 100., when, "Album");
    layer.add_imported(&track("spotify:track:y", "artist a"), 50., when, "Other");
    layer.add_imported(&track("spotify:track:z", "Artist B"), 20., when, "");
    let before = layer.clone();
    let out = layer.remap(|t| match t.0.as_str() {
        "spotify:track:z" => t.clone(),
        _ => Track("mine.mp3".into(), "Mine".into(), "Artist A".into()),
    });
    assert_eq!(out.tracks().count(), 2);
    assert_eq!(out.artist_count(), 2);
    assert_eq!(out.album("mine.mp3"), Some("Album"));
    let d = &out.days()[&day(2020, 3, 1)];
    assert_eq!(d.s, 170.);
    assert_eq!(d.h[9], 170.);
    let r = out.range(day(2020, 3, 1), day(2020, 3, 1));
    assert_eq!(r.tracks[0].1, 150.);
    assert_eq!(r.artists[0], ("Artist A".to_string(), 150.));
    assert_eq!(layer.tracks().count(), before.tracks().count());
    assert_eq!(layer.days()[&day(2020, 3, 1)].t.len(), 3);
}

/// Opens `path` the way another program holding it does: nobody else can read it until the handle closes.
#[cfg(windows)]
fn locked(path: &std::path::Path) -> fs::File {
    use std::os::windows::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(path)
        .unwrap()
}

#[cfg(windows)]
#[test]
fn a_spotify_file_that_cant_be_copied_leaves_the_other_opening_copies() {
    let dir = tempdir().unwrap();
    let backup = dir.path().join("backup");
    let folder = DataFolder::new(dir.path(), &backup);
    fs::write(dir.path().join("state.json"), b"{\"v\":1}").unwrap();
    fs::write(dir.path().join("listening.json"), b"{\"v\":1}").unwrap();
    fs::write(dir.path().join("spotify.json"), b"{\"v\":1}").unwrap();
    let lock = locked(&dir.path().join("spotify.json"));
    let err = folder.keep_opening_copy().unwrap_err().to_string();
    assert!(err.contains("spotify.json"), "{err}");
    for other in ["state.json", "listening.json", "library.json"] {
        assert!(!err.contains(other), "{err}");
    }
    let kept = backup
        .join("opened")
        .join(Local::now().date_naive().to_string());
    assert_eq!(fs::read(kept.join("state.json")).unwrap(), b"{\"v\":1}");
    assert_eq!(fs::read(kept.join("listening.json")).unwrap(), b"{\"v\":1}");
    assert!(!kept.join("spotify.json").exists());
    drop(lock);

    // The first save of the day makes the opening copy too: a locked spotify.json doesn't cost it its daily backup.
    let dir = tempdir().unwrap();
    let backup = dir.path().join("backup");
    let folder = DataFolder::new(dir.path(), &backup);
    fs::create_dir_all(&backup).unwrap();
    let date = Local::now().date_naive();
    fs::write(backup.join(format!("state-{date}.json")), b"{\"v\":1}").unwrap();
    fs::write(dir.path().join("spotify.json"), b"{\"v\":1}").unwrap();
    let _lock = locked(&dir.path().join("spotify.json"));
    let mut state = folder.read::<string_core::state::State>().unwrap();
    let saved = state.save().unwrap();
    assert!(saved.backup_error.is_none(), "{:?}", saved.backup_error);
    assert!(
        backup
            .join("opened")
            .join(date.to_string())
            .join(format!("state-{date}.json"))
            .exists()
    );
}
