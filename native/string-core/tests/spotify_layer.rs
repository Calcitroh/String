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
    assert_eq!(back.value.meta(), meta);
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
