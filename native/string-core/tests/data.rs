//! Guarded loads, atomic writers, retention and the command-line entry points.

use chrono::{Days, Local};
use serde_json::json;
use std::{
    fs,
    path::Path,
    process::Command,
    sync::{Arc, Barrier},
};
use string_core::{data::DataFolder, state::State, stats::Listening};
use tempfile::tempdir;

fn ctl(cmd: &str, dir: &Path, args: &[&Path]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_stringctl"))
        .arg(cmd)
        .arg("--data")
        .arg(dir)
        .args(args)
        .output()
        .unwrap()
}
#[test]
fn corrupt_file_aborts_roundtrip_before_writes_and_inspect_reports_all() {
    let t = tempdir().unwrap();
    let dir = t.path();
    let state = b"{ \"v\": 1, \"queue\": { \"items\": [] } }";
    fs::write(dir.join("state.json"), state).unwrap();
    fs::write(dir.join("listening.json"), "{").unwrap();
    fs::write(dir.join("library.json"), r#"{"v":2,"tracks":[]}"#).unwrap();
    let result = ctl("roundtrip", dir, &[]);
    assert!(!result.status.success());
    assert_eq!(fs::read(dir.join("state.json")).unwrap(), state);
    assert!(!dir.join("backup").exists());
    let r = ctl("inspect", dir, &[]);
    let err = String::from_utf8(r.stderr).unwrap();
    assert!(!r.status.success());
    assert!(err.contains("listening.json"));
    assert!(
        String::from_utf8(r.stdout)
            .unwrap()
            .contains("0 queue items")
    );
}
#[test]
fn absent_models_stay_absent_on_roundtrip() {
    let t = tempdir().unwrap();
    let r = ctl("roundtrip", t.path(), &[]);
    assert!(r.status.success());
    assert_eq!(
        String::from_utf8(r.stdout)
            .unwrap()
            .matches("absent")
            .count(),
        3
    );
    assert_eq!(fs::read_dir(t.path()).unwrap().count(), 0);
}

#[test]
fn roundtrip_keeps_the_original_before_replacing_a_defaulted_field() {
    let dir = tempdir().unwrap();
    let before = br#"{"v":1,"queue":{"repeat":false},"custom":"keep"}"#;
    fs::write(dir.path().join("state.json"), before).unwrap();
    assert!(ctl("roundtrip", dir.path(), &[]).status.success());
    let kept = dir
        .path()
        .join("backup/opened")
        .join(Local::now().date_naive().to_string());
    assert_eq!(fs::read(kept.join("state.json")).unwrap(), before);
}

#[test]
fn failed_backup_is_reported_and_live_save_can_be_retried() {
    let t = tempdir().unwrap();
    let backup = t.path().join("copies");
    fs::write(&backup, b"unavailable").unwrap();
    let mut d = DataFolder::new(t.path(), &backup).read::<State>().unwrap();
    let saved = d.save().unwrap();
    assert!(saved.bytes_changed && saved.backup_error.is_some());
    assert!(t.path().join("state.json").is_file());
    fs::remove_file(&backup).unwrap();
    let saved = d.save().unwrap();
    assert!(!saved.bytes_changed && saved.backup_error.is_none());
}
#[test]
fn stale_or_damaged_file_cannot_be_overwritten() {
    let t = tempdir().unwrap();
    let f = DataFolder::new(t.path(), t.path().join("backups"));
    let mut fresh = f.read::<State>().unwrap();
    fresh.save().unwrap();
    let mut stale = f.read::<State>().unwrap();
    fresh.value.fields.insert("liked".into(), json!({"a":1}));
    fresh.save().unwrap();
    assert!(stale.save().is_err());
    fs::write(t.path().join("state.json"), b"{").unwrap();
    assert!(fresh.save().is_err());
    assert!(f.read::<State>().is_err());
    assert_eq!(fs::read(t.path().join("state.json")).unwrap(), b"{");
}
#[cfg(windows)]
#[test]
fn exclusively_locked_file_is_a_read_error() {
    use std::os::windows::fs::OpenOptionsExt;
    let t = tempdir().unwrap();
    let f = DataFolder::new(t.path(), t.path().join("backup"));
    let mut doc = f.read::<State>().unwrap();
    doc.save().unwrap();
    let file = fs::File::options()
        .read(true)
        .share_mode(0)
        .open(t.path().join("state.json"))
        .unwrap();
    assert!(f.open().is_err());
    assert!(doc.save().is_err());
    drop(file);
    assert!(f.open().is_ok());
}
#[test]
fn independent_writers_detect_competing_versions() {
    let t = tempdir().unwrap();
    let dir = t.path();
    let f = DataFolder::new(dir, dir.join("backup"));
    let gate = Arc::new(Barrier::new(8));
    let wins = std::thread::scope(|s| {
        let mut threads = vec![];
        for i in 0..8 {
            let mut doc = f.read::<State>().unwrap();
            doc.value.fields.insert("sequence".into(), json!(i));
            let gate = gate.clone();
            threads.push(s.spawn(move || {
                gate.wait();
                doc.save().is_ok()
            }));
        }
        threads
            .into_iter()
            .map(|t| usize::from(t.join().unwrap()))
            .sum::<usize>()
    });
    assert_eq!(wins, 1);
    assert!(f.read::<State>().unwrap().value.fields["sequence"].is_number());
    assert_eq!(
        fs::read_dir(dir).unwrap().count(),
        3,
        "only the live file, lock and backup folder"
    );
}
#[test]
fn daily_retention_is_per_name_and_preserves_other_files() {
    let t = tempdir().unwrap();
    let backup = t.path().join("copies");
    fs::create_dir(&backup).unwrap();
    let f = DataFolder::new(t.path(), &backup);
    let today = Local::now().date_naive();
    for days in 1..=64 {
        let date = today - Days::new(days);
        fs::write(backup.join(format!("state-{date}.json")), b"old").unwrap();
    }
    let unrelated = backup.join("state-keep.json");
    fs::write(&unrelated, b"keep").unwrap();
    let mut d = f.read::<State>().unwrap();
    assert!(d.save().unwrap().backup_error.is_none());
    d.value.fields.insert("flag".into(), json!(true));
    d.save().unwrap();
    f.read::<Listening>().unwrap().save().unwrap();
    assert_eq!(fs::read_dir(&backup).unwrap().count(), 62);
    assert_eq!(fs::read(&unrelated).unwrap(), b"keep");
    assert_eq!(
        fs::read(backup.join(format!("state-{today}.json"))).unwrap(),
        fs::read(t.path().join("state.json")).unwrap()
    );
}
#[test]
fn invalid_queue_entries_and_listening_indices_are_local_to_the_entry() {
    let t = tempdir().unwrap();
    let f = DataFolder::new(t.path(), t.path().join("copy"));
    for queue in [
        json!({"paths":[[2,"x",0]],"items":[0]}),
        json!({"paths":[],"items":[-1]}),
        json!({"items":[],"listStart":8}),
    ] {
        fs::write(
            t.path().join("state.json"),
            json!({"v":1,"queue":queue}).to_string(),
        )
        .unwrap();
        assert!(f.read::<State>().unwrap().value.queue.items().is_empty());
    }
    let day = json!({"s":1,"h":vec![0;24],"a":{},"t":{"3":1}});
    fs::write(
        t.path().join("listening.json"),
        json!({"v":1,"days":{"2026-02-01":day},"tracks":[],"artists":[]}).to_string(),
    )
    .unwrap();
    let listening = f.read::<Listening>().unwrap();
    let day = listening.value.days().values().next().unwrap();
    assert_eq!(day.s, 1.);
    assert!(day.t.is_empty());
}

#[test]
fn wrong_fields_keep_their_siblings_and_dictionary_slots() {
    let t = tempdir().unwrap();
    let dir = t.path();
    fs::write(
        dir.join("state.json"),
        br#"{
        "v":1, "roots":false, "route":"liked", "set":{"fade":false,"custom":"yes"},
        "queue":{"items":["one",null,"two"],"at":2,"repeat":false,"pos":null,"extension":[1]}
    }"#,
    )
    .unwrap();
    fs::write(
        dir.join("listening.json"),
        br#"{
        "v":1, "artists":[null,"second"], "tracks":[["a",false,"artist"],["b","B"]],
        "days":{"2026-01-01":{"s":20,"h":[null,3],"a":{"0":null,"1":8},"t":{"1":5},"note":"kept"}},
        "phoneImport":{"after":{"bad":"oops","good":12},"plays":4}
    }"#,
    )
    .unwrap();
    let p = DataFolder::new(dir, dir.join("copies")).open().unwrap();
    assert_eq!(p.state.value.fields["roots"], json!([]));
    assert_eq!(p.state.value.fields["set"]["fade"], 0);
    assert_eq!(p.state.value.fields["set"]["custom"], "yes");
    assert_eq!(p.state.value.fields["route"], "liked");
    let queue = &p.state.value.queue;
    assert_eq!(queue.items().len(), 2);
    assert_eq!(queue.current(), Some("two"));
    assert_eq!(queue.pos(), 0.);
    let state: serde_json::Value = serde_json::from_slice(&p.state.json().unwrap()).unwrap();
    assert_eq!(state["queue"]["extension"], json!([1]));
    let stats = serde_json::to_value(p.listening.value).unwrap();
    let day = &stats["days"]["2026-01-01"];
    assert_eq!(day["h"][1], 3.);
    assert_eq!(day["a"]["1"], 8.);
    assert_eq!(day["t"]["1"], 5.);
    assert_eq!(day["note"], "kept");
    assert_eq!(stats["artists"][1], "second");
    assert_eq!(stats["tracks"][1], json!(["b", "B", ""]));
    assert_eq!(stats["phoneImport"]["after"]["good"], 12);
}

#[test]
fn an_unrecognized_day_key_survives_a_listening_edit() {
    let tmp = tempdir().unwrap();
    let path = tmp.path().join("listening.json");
    fs::write(
        &path,
        br#"{"v":1,"days":{"NaN-NaN-NaN":{"s":3,"extension":true}}}"#,
    )
    .unwrap();
    let mut doc = DataFolder::new(tmp.path(), tmp.path().join("copies"))
        .read::<Listening>()
        .unwrap();
    doc.value.add(
        &string_core::stats::Track("a".into(), "A".into(), "Artist".into()),
        2.,
        "2026-11-22T10:00:00".parse().unwrap(),
    );
    doc.save().unwrap();
    let json: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(json["days"]["NaN-NaN-NaN"], json!({"s":3,"extension":true}));
    assert_eq!(json["days"]["2026-11-22"]["s"], 2);
}

#[test]
fn a_bad_playlist_items_value_and_optional_setting_keep_the_rest_of_the_playlist() {
    let tmp = tempdir().unwrap();
    fs::write(tmp.path().join("state.json"),br#"{"v":1,"playlists":[{"id":"p","name":"Mix","items":false,"cover":"image"}],"set":{"phoneStats":{},"custom":12},"files":["keep.wav",false]}"#).unwrap();
    let state = DataFolder::new(tmp.path(), tmp.path().join("backup"))
        .read::<State>()
        .unwrap()
        .value;
    assert_eq!(
        state.fields["playlists"],
        json!([{"id":"p","name":"Mix","items":[],"cover":"image"}])
    );
    assert!(
        state.fields["set"]["phoneStats"].is_null() || state.fields["set"]["phoneStats"] == true
    );
    assert_eq!(state.fields["set"]["custom"], 12);
    assert_eq!(state.fields["files"], json!(["keep.wav"]));
}

#[test]
fn broken_cache_rebuilds_and_good_neighbors_survive_a_bad_row() {
    let t = tempdir().unwrap();
    let dir = t.path();
    let path = dir.join("library.json");
    let folder = DataFolder::new(dir, dir.join("copies"));
    fs::write(&path, b"broken").unwrap();
    assert!(folder.open().unwrap().library.value.tracks.is_empty());
    fs::write(&path,br#"{"v":1,"extension":17,"tracks":[null,{"path":"good","title":"kept","custom":3},{"path":"bad","title":12}]}"#).unwrap();
    let library = folder.open().unwrap().library.value;
    assert_eq!(library.extra["extension"], 17);
    assert_eq!(library.tracks[0].tags.title, "kept");
    assert_eq!(library.tracks[0].extra["custom"], 3);
    assert!(
        !library
            .tracks
            .iter()
            .any(|t| t.path == "bad" && t.read_error == 0)
    );
    let song = dir.join("reread.webm");
    fs::write(&song,[0x1a,0x45,0xdf,0xa3]).unwrap();
    fs::write(&path,json!({"v":1,"tracks":[{"path":song,"title":12,"extension":9}]}).to_string()).unwrap();
    let mut cache = folder.open().unwrap().library;
    assert_eq!(cache.value.scan(&[], &[song]).read,1);
    assert_eq!(cache.value.tracks[0].extra["extension"],9);
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    assert!(
        folder.open().is_err(),
        "an I/O error must still block every writer"
    );
}

#[test]
fn lone_utf16_units_survive_in_paths_names_and_unknown_keys() {
    let t = tempdir().unwrap();
    let dir = t.path();
    fs::write(dir.join("state.json"),br#"{"v":1,"queue":{"paths":[[0,"a\ud800x",0],[2,"\udc00y",0]],"items":[0,1],"orig":null},"playlists":[{"name":"hi\udfff","items":[]}],"extension":{"\udfff":"\\udfff","\u0000\ue7ff":true}}"#).unwrap();
    fs::write(
        dir.join("library.json"),
        br#"{"v":1,"tracks":[{"path":"a\ud800x"}]}"#,
    )
    .unwrap();
    let folder = DataFolder::new(dir, dir.join("copies"));
    let mut p = folder.open().unwrap();
    assert_eq!(
        p.state.value.queue.items()[0].id,
        p.library.value.tracks[0].path
    );
    assert_eq!(p.state.value.queue.items()[1].id, "a\u{10000}y");
    p.state.save().unwrap();
    p.library.save().unwrap();
    let saved = fs::read_to_string(dir.join("state.json")).unwrap();
    assert!(saved.contains("\"\\u0000\u{e7ff}\":true"));
    assert!(saved.contains(r#""hi\udfff""#) && saved.contains(r#""\udfff":"\\udfff""#));
    assert_eq!(
        folder.open().unwrap().state.value.queue.items(),
        p.state.value.queue.items()
    );
}

#[cfg(windows)]
#[test]
fn scan_keeps_a_cached_loose_filename_with_an_isolated_utf16_unit() {
    use std::os::windows::ffi::OsStringExt;
    let t = tempdir().unwrap();
    let name = std::ffi::OsString::from_wide(&[0xd800, 46, 119, 97, 118]);
    let song = t.path().join(name);
    fs::write(&song, b"song").unwrap();
    let prefix = serde_json::to_string(t.path().to_str().unwrap()).unwrap();
    let path_json = format!(r#"{}\\\ud800.wav""#, &prefix[..prefix.len() - 1]);
    fs::write(
        t.path().join("library.json"),
        format!(r#"{{"v":1,"tracks":[{{"path":{path_json},"title":"Keep"}}]}}"#),
    )
    .unwrap();
    let mut p = DataFolder::new(t.path(), t.path().join("backup"))
        .open()
        .unwrap();
    p.library.value.scan(&[], std::slice::from_ref(&song));
    assert_eq!(p.library.value.tracks.len(), 1);
    p.library.save().unwrap();
    assert!(
        fs::read_to_string(t.path().join("library.json"))
            .unwrap()
            .contains(r"\ud800.wav")
    );
}

#[test]
fn cli_scan_inspect_and_stable_second_roundtrip() {
    let t = tempdir().unwrap();
    let dir = t.path().join("data");
    let music = t.path().join("music");
    fs::create_dir(&music).unwrap();
    fs::write(music.join("test.webm"), [0x1a, 0x45, 0xdf, 0xa3]).unwrap();
    let r = ctl("scan", &dir, &[&music]);
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    assert!(String::from_utf8(r.stdout).unwrap().contains("1 read"));
    assert!(
        String::from_utf8(ctl("scan", &dir, &[&music]).stdout)
            .unwrap()
            .contains("1 reused")
    );
    assert!(
        String::from_utf8(ctl("inspect", &dir, &[]).stdout)
            .unwrap()
            .contains("1 tracks")
    );
    let f = DataFolder::new(&dir, dir.join("backup"));
    f.read::<State>().unwrap().save().unwrap();
    f.read::<Listening>().unwrap().save().unwrap();
    let r = ctl("roundtrip", &dir, &[]);
    assert!(r.status.success());
    assert_eq!(
        String::from_utf8(r.stdout)
            .unwrap()
            .matches("bytes unchanged")
            .count(),
        3
    );
}
#[test]
fn opening_copy_keeps_the_files_as_found_once_a_day() {
    let t = tempdir().unwrap();
    let (dir, backup) = (t.path().join("data"), t.path().join("backup"));
    fs::create_dir_all(&dir).unwrap();
    let before = br#"{"v":1,"saved_by":"another version"}"#;
    fs::write(dir.join("state.json"), before).unwrap();
    let folder = DataFolder::new(&dir, &backup);
    folder.keep_opening_copy().unwrap();
    let today = backup.join("opened").join(Local::now().date_naive().to_string());
    assert_eq!(fs::read(today.join("state.json")).unwrap(), before);
    assert!(!today.join("library.json").exists(), "a file that isn't there isn't invented");
    // a later save, then opening again the same day: the copy from before the first save stays
    fs::write(dir.join("state.json"), b"{\"v\":1}").unwrap();
    folder.keep_opening_copy().unwrap();
    assert_eq!(fs::read(today.join("state.json")).unwrap(), before);
    // ten days are kept
    for d in 1..=12u64 {
        let day = Local::now().date_naive().checked_sub_days(Days::new(d)).unwrap();
        fs::create_dir_all(backup.join("opened").join(day.to_string())).unwrap();
    }
    fs::remove_dir_all(&today).unwrap();
    folder.keep_opening_copy().unwrap();
    let left = fs::read_dir(backup.join("opened")).unwrap().count();
    assert_eq!(left, 10);
    assert!(today.exists());
}
#[test]
fn opening_copy_keeps_todays_backups_and_refuses_a_file_in_its_place() {
    let t = tempdir().unwrap();
    let (dir, backup) = (t.path().join("data"), t.path().join("backup"));
    fs::create_dir_all(&dir).unwrap();
    fs::create_dir_all(&backup).unwrap();
    let date = Local::now().date_naive().to_string();
    // state.json is gone, but today's backup of it is there: the opening copy keeps that backup
    let daily = backup.join(format!("state-{date}.json"));
    fs::write(&daily, br#"{"v":1,"only":"copy"}"#).unwrap();
    let folder = DataFolder::new(&dir, &backup);
    folder.keep_opening_copy().unwrap();
    let today = backup.join("opened").join(&date);
    assert_eq!(fs::read(today.join(format!("state-{date}.json"))).unwrap(), br#"{"v":1,"only":"copy"}"#);
    // a file where today's copy goes is an error, not a copy already made
    let t2 = tempdir().unwrap();
    let blocked = DataFolder::new(t2.path().join("data"), t2.path().join("backup"));
    fs::create_dir_all(t2.path().join("backup").join("opened")).unwrap();
    fs::write(t2.path().join("backup").join("opened").join(&date), b"in the way").unwrap();
    assert!(blocked.keep_opening_copy().is_err());
}
