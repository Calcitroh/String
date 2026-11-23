use chrono::{Local, TimeZone, Timelike};
use rusqlite::Connection;
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
use string_core::{data::DataFolder, library::Track, tags::Tags};
use string_extras::phone::*;
use tempfile::{TempDir, tempdir};

fn fixture() -> (TempDir, PathBuf, PathBuf, DataFolder) {
    let tmp = tempdir().unwrap();
    let db = tmp.path().join("fixture.db");
    let media = tmp.path().join("media");
    let output = Command::new("node")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fake-navidrome.js"))
        .arg(&db)
        .arg(&media)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let folder = DataFolder::new(tmp.path().join("profile"), tmp.path().join("backup"));
    (tmp, db, media, folder)
}
#[test]
fn path_configuration_and_absent_database() {
    let tmp = tempdir().unwrap();
    let ini = tmp.path().join("navidrome.ini");
    assert_eq!(
        database_from_ini(&ini, tmp.path()),
        tmp.path().join("navidrome.db")
    );
    fs::write(
        &ini,
        "\u{feff} dataFolder = 'X:\\First'\r\n DATAFOLDER= \"Y:\\Media Data\"  \n",
    )
    .unwrap();
    assert_eq!(
        database_from_ini(&ini, tmp.path()),
        Path::new(r"Y:\Media Data\navidrome.db")
    );
    let ini16: Vec<_> = "\u{feff}DataFolder='Z:\\Audio'"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    fs::write(&ini, ini16).unwrap();
    assert_eq!(
        database_from_ini(&ini, tmp.path()),
        Path::new(r"Z:\Audio\navidrome.db")
    );
    assert!(
        !navidrome_plays(&tmp.path().join("absent"), 0, "")
            .unwrap()
            .found
    );
}
#[test]
fn account_choice_wal_paths_time_names_and_replay() {
    let (_tmp, db, media, folder) = fixture();
    let mut p = folder.open().unwrap();
    let song = media.join("glass orbit.mp3").to_str().unwrap().to_owned();
    p.library.value.tracks.push(Track {
        path: song.clone(),
        tags: Tags {
            title: "Embedded".into(),
            duration: 36.5,
            ..Default::default()
        },
        ..Default::default()
    });
    p.state.value.fields["edits"][song.to_lowercase()] =
        json!({"title":"String title","artist":"String artist"});
    let conn = Connection::open(&db).unwrap();
    assert_eq!(
        conn.query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "wal"
    );
    conn.execute("UPDATE scrobbles SET submission_time=1700010000", [])
        .unwrap();
    conn.execute_batch("BEGIN IMMEDIATE; DELETE FROM scrobbles")
        .unwrap();
    let report = sync(&mut p, &db).unwrap();
    assert_eq!(report.added, 0);
    assert_eq!(report.users.len(), 2);
    assert_eq!(report.users[0].0, "u2");
    let page = navidrome_plays(&db, 2, "u1").unwrap();
    assert_eq!(page.plays.len(), 2);
    assert_eq!(
        page.plays[1].path,
        media
            .join("Gone/old song.mp3")
            .to_string_lossy()
            .replace('/', r"\")
    );
    let report = choose(&mut p, &db, "u1").unwrap();
    assert_eq!(report.added, 4);
    conn.execute_batch("ROLLBACK").unwrap();
    let when = Local.timestamp_opt(1700010000, 0).unwrap();
    let day = &p.listening.value.days()[&when.date_naive()];
    assert_eq!(day.p, Some(291.));
    assert_eq!(day.h[when.hour() as usize], 291.);
    let ranked = p
        .listening
        .value
        .range(when.date_naive(), when.date_naive());
    let (track, secs) = ranked
        .tracks
        .iter()
        .find(|(t, _)| t.0 == song.to_lowercase())
        .unwrap();
    assert_eq!(
        (&track.1[..], &track.2[..], *secs),
        ("String title", "String artist", 73.)
    );
    assert_eq!(p.state.value.fields["phone"]["after"], 4);
    assert_eq!(choose(&mut p, &db, "u2").unwrap().added, 1);
    assert_eq!(choose(&mut p, &db, "u1").unwrap().added, 0);
    // Emulate a crash between publishing the listening and state files.
    p.state.value.fields["phone"] = json!({"after":0,"plays":0});
    p.state.save().unwrap();
    let mut p = folder.open().unwrap();
    assert_eq!(sync(&mut p, &db).unwrap().added, 0);
    assert_eq!(p.state.value.fields["phone"]["plays"], 5);
    let history = p.listening.value.phone_import().unwrap();
    assert_eq!(history.after["u1"], 4);
    assert_eq!(history.after["u2"], 5);
}
#[test]
fn single_account_legacy_cursor_disabled_and_rejected_lengths() {
    let (_tmp, db, _, folder) = fixture();
    let conn = Connection::open(&db).unwrap();
    conn.execute("DELETE FROM user WHERE id='u2'", []).unwrap();
    conn.execute("UPDATE media_file SET duration=20000 WHERE id='m3'", [])
        .unwrap();
    let mut p = folder.open().unwrap();
    p.state
        .value
        .fields
        .insert("phone".into(), json!({"after":2,"plays":9}));
    p.state.value.fields["set"]["phoneStats"] = false.into();
    assert_eq!(sync(&mut p, &db).unwrap().added, 0);
    assert!(p.listening.value.phone_import().is_none());
    p.state.value.fields["set"]["phoneStats"] = true.into();
    assert_eq!(sync(&mut p, &db).unwrap().added, 1);
    assert_eq!(p.state.value.fields["set"]["phoneUser"], "u1");
    let h = p.listening.value.phone_import().unwrap();
    assert_eq!((h.legacy_after, h.plays, h.after["u1"]), (2, 10, 4));
}
#[test]
fn failed_listening_save_leaves_page_retryable() {
    let (tmp, db, _, folder) = fixture();
    let mut p = folder.open().unwrap();
    let root = tmp.path().join("profile");
    fs::create_dir_all(root.join(".listening.lock")).unwrap();
    assert!(choose(&mut p, &db, "u1").is_err());
    assert!(p.listening.value.phone_import().is_none());
    assert!(p.listening.value.days().is_empty());
    assert_eq!(p.state.value.fields["phone"]["after"], 0);
    fs::remove_dir(root.join(".listening.lock")).unwrap();
    assert_eq!(sync(&mut p, &db).unwrap().added, 4);
    let mut p = folder.open().unwrap();
    assert_eq!(sync(&mut p, &db).unwrap().added, 0);
}
#[test]
fn reads_multiple_pages_in_id_order() {
    let (_tmp, db, _, folder) = fixture();
    let con = Connection::open(&db).unwrap();
    con.execute_batch("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<5000)
        INSERT INTO scrobbles(media_file_id,user_id,submission_time) SELECT 'm1','u1',1700000000 FROM n").unwrap();
    let mut p = folder.open().unwrap();
    assert_eq!(choose(&mut p, &db, "u1").unwrap().added, 5004);
    assert_eq!(p.listening.value.phone_import().unwrap().after["u1"], 5005);
}
