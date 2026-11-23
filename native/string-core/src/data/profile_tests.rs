use super::*;
use crate::stats;
use tempfile::tempdir;

#[test]
fn failed_opening_copy_protects_existing_daily_across_saves_and_restart() {
    let dir = tempdir().unwrap();
    let copies = dir.path().join("backup");
    fs::create_dir(&copies).unwrap();
    fs::write(copies.join("opened"), b"blocked").unwrap();
    let date = Local::now().date_naive();
    let daily = copies.join(format!("state-{date}.json"));
    let original = br#"{ "playlists":[{"name":"Only copy"}] }"#;
    fs::write(&daily, original).unwrap();
    let folder = DataFolder::new(dir.path(), &copies);
    for route in ["songs", "liked"] {
        let mut p = folder.open().unwrap();
        assert!(folder.keep_opening_copy().is_err());
        p.state.value.fields.insert("route".into(), route.into());
        assert!(p.state.save().unwrap().backup_error.is_some());
        assert_eq!(folder.read::<State>().unwrap().value.fields["route"], route);
        assert_eq!(fs::read(&daily).unwrap(), original);
    }
    fs::remove_file(copies.join("opened")).unwrap();
    let mut p = folder.open().unwrap();
    folder.keep_opening_copy().unwrap();
    assert!(p.state.save().unwrap().backup_error.is_none());
    assert_eq!(
        fs::read(
            copies
                .join("opened")
                .join(date.to_string())
                .join(daily.file_name().unwrap())
        )
        .unwrap(),
        original
    );
    assert_eq!(
        fs::read(daily).unwrap(),
        fs::read(dir.path().join("state.json")).unwrap()
    );
}

#[test]
fn a_later_daily_can_be_the_only_copy_even_with_an_earlier_opening_snapshot() {
    let dir = tempdir().unwrap();
    let copies = dir.path().join("backup");
    let folder = DataFolder::new(dir.path(), &copies);
    let date = Local::now().date_naive();
    let name = format!("state-{date}.json");
    fs::create_dir(&copies).unwrap();
    fs::write(copies.join(&name), b"morning").unwrap();
    folder.keep_opening_copy().unwrap();
    fs::write(copies.join(&name), b"afternoon").unwrap();
    assert!(
        folder
            .read::<State>()
            .unwrap()
            .save()
            .unwrap()
            .backup_error
            .is_none()
    );
    let kept: Vec<_> = fs::read_dir(copies.join("opened").join(date.to_string()))
        .unwrap()
        .map(|entry| fs::read(entry.unwrap().path()).unwrap())
        .collect();
    assert!(kept.iter().any(|b| b == b"morning"));
    assert!(kept.iter().any(|b| b == b"afternoon"));
}

#[test]
fn unusable_state_cannot_authorize_a_cache_prune() {
    let dir = tempdir().unwrap();
    let folder = DataFolder::new(dir.path(), dir.path().join("backup"));
    let cache = br#"{ "tracks":[{"path":"offline.wav","added":123,"extension":true}] }"#;
    fs::write(folder.path::<Library>(), cache).unwrap();
    for contents in ["[]", "null"] {
        fs::write(folder.path::<State>(), contents).unwrap();
        if let Ok(mut p) = folder.open() {
            if p.library.value.scan(&[], &[]).changed() {
                p.library.save().unwrap();
            }
            let _ = p.state.save();
        }
        assert_eq!(fs::read(folder.path::<Library>()).unwrap(), cache);
        assert_eq!(
            fs::read_to_string(folder.path::<State>()).unwrap(),
            contents
        );
    }
    fs::write(folder.path::<State>(), b"{}").unwrap();
    assert_eq!(
        folder.open().unwrap().library.value.scan(&[], &[]).removed,
        1
    );
}

#[test]
fn unsupported_documents_are_refused_before_a_writer_is_created() {
    let dir = tempdir().unwrap();
    let folder = DataFolder::new(dir.path(), dir.path().join("backup"));
    for name in ["library", "listening", "state"] {
        let path = dir.path().join(format!("{name}.json"));
        for contents in [
            r#"{ "v":2, "future":[123] }"#,
            r#"{"v":"2"}"#,
            "[1,2]",
            "null",
        ] {
            fs::write(&path, contents).unwrap();
            assert!(folder.open().is_err(), "{name} accepted {contents}");
            assert_eq!(fs::read_to_string(&path).unwrap(), contents);
        }
        fs::remove_file(path).unwrap();
    }
}

#[test]
fn well_formed_cache_beyond_the_value_reader_limits_is_not_rebuilt() {
    let dir = tempdir().unwrap();
    let folder = DataFolder::new(dir.path(), dir.path().join("backup"));
    let cache = format!(
        r#"{{"v":2,"extension":{}0{}}}"#,
        "[".repeat(160),
        "]".repeat(160)
    );
    fs::write(folder.path::<Library>(), &cache).unwrap();
    assert!(folder.read::<Library>().is_err());
    assert_eq!(fs::read_to_string(folder.path::<Library>()).unwrap(), cache);
}

#[test]
fn older_state_only_phone_cursor_is_recovered_before_the_first_import() {
    let dir = tempdir().unwrap();
    let folder = DataFolder::new(dir.path(), dir.path().join("backup"));
    for (after, expected) in [(json!("125"), 125), (Value::Null, i64::MAX)] {
        let state = json!({"phone":{"after":after,"plays":4}});
        fs::write(folder.path::<State>(), state.to_string()).unwrap();
        let p = folder.open().unwrap();
        let h = p.listening.value.phone_import().unwrap();
        assert_eq!((h.legacy_after, h.plays), (expected, 4));
        assert_eq!(p.state.value.fields["phone"], state["phone"]);
    }
}

#[test]
fn damaged_playlist_ids_are_unique_stable_and_leave_other_lists_intact() {
    let dir = tempdir().unwrap();
    let folder = DataFolder::new(dir.path(), dir.path().join("backup"));
    let lists = json!([
        {"id":7,"name":"First","items":["a"],"extension":[1]},
        {"id":null,"name":"Second","items":["b"],"extension":[2]},
        {"id":"recovered-1","name":"Valid","items":[]},
        {"id":"","name":"Empty","items":[]}, {"name":"Missing","items":[]},
        {"id":"recovered-1","name":"Duplicate","items":[]}
    ]);
    fs::write(
        folder.path::<State>(),
        json!({"playlists":lists}).to_string(),
    )
    .unwrap();
    let mut doc = folder.read::<State>().unwrap();
    let rows = doc.value.fields["playlists"].as_array().unwrap();
    let ids: std::collections::HashSet<_> = rows
        .iter()
        .map(|p| p["id"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(ids.len(), rows.len());
    assert!(!ids.contains(""));
    assert_eq!(rows[2], lists[2]);
    assert_eq!(
        doc.value.fields["playlists"],
        folder.read::<State>().unwrap().value.fields["playlists"]
    );
    let id = rows[1]["id"].clone();
    let rows = doc.value.fields["playlists"].as_array_mut().unwrap();
    rows.iter_mut().find(|p| p["id"] == id).unwrap()["name"] = "Renamed".into();
    doc.save().unwrap();
    let reloaded = folder.read::<State>().unwrap();
    let saved = &reloaded.value.fields["playlists"];
    assert_eq!(saved, &doc.value.fields["playlists"]);
    assert_eq!(saved[0]["name"], "First");
    assert_eq!(saved[1]["items"], lists[1]["items"]);
    assert_eq!(saved[1]["extension"], lists[1]["extension"]);
}

#[test]
fn queue_position_belongs_to_the_selected_occurrence() {
    for items in [
        json!({"items":["A",null,"B","A"]}),
        json!({"paths":[[0,"A",0],[0,"B",0]],"items":[0,99,1,0]}),
    ] {
        for (at, pos) in [(1, 0.), (2, 80.), (3, 80.), (9, 0.)] {
            let mut queue = items.clone();
            queue["at"] = at.into();
            queue["pos"] = 80.into();
            let s: State = serde_json::from_value(json!({"v":1,"queue":queue})).unwrap();
            assert_eq!(s.queue.pos(), pos, "at {at}");
            if at == 2 {
                assert_eq!(s.queue.current(), Some("B"));
            }
            if at == 3 {
                assert_eq!(s.queue.at(), Some(2));
            }
        }
    }
}

#[test]
fn listening_edits_keep_extension_cells_on_their_dictionary_row() {
    let mut doc: Listening = serde_json::from_value(json!({
        "tracks":[["a","Old","Artist",{"source":"import"},null,17],["b","B","",["other"]]]
    }))
    .unwrap();
    doc.add(
        &stats::Track("a".into(), "New".into(), "Artist".into()),
        2.,
        "2026-01-02T01:02:03".parse().unwrap(),
    );
    doc.merge("b", "c");
    let saved = serde_json::to_value(doc).unwrap();
    assert_eq!(
        saved["tracks"],
        json!([["a","New","Artist",{"source":"import"},null,17],["c","B","",["other"]]])
    );
}

#[test]
fn saved_phone_cursors_never_replay_credited_rows() {
    let mut doc: Listening = serde_json::from_value(json!({
        "days":{"2026-01-02":{"s":120}},
        "phoneImport":{"legacyAfter":"80","after":{"known":"125","unresolved":{"bad":true},"other":7},"plays":3}
    })).unwrap();
    let mut history = doc.phone_import().unwrap().clone();
    assert_eq!(history.after["known"], 125);
    assert_eq!(history.legacy_after, 80);
    assert_eq!(history.after["unresolved"], i64::MAX);
    history.after.insert("other".into(), 8);
    doc.set_phone_import(history);
    let saved = serde_json::to_value(&doc).unwrap();
    assert_eq!(
        saved["phoneImport"]["after"]["unresolved"],
        json!({"bad":true})
    );
    assert_eq!(saved["phoneImport"]["after"]["other"], 8);
    assert_eq!(doc.days().values().next().unwrap().s, 120.);
}

#[cfg(windows)]
#[test]
fn wide_filenames_are_discovered_and_changed_tags_are_reread() {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt};
    let dir = tempdir().unwrap();
    let root = dir.path().join("music");
    fs::create_dir(&root).unwrap();
    let song = root.join(OsString::from_wide(&[0xd800, 46, 119, 101, 98, 109]));
    fs::write(&song, [0x1a, 0x45, 0xdf, 0xa3]).unwrap();
    let prefix = serde_json::to_string(root.to_str().unwrap()).unwrap();
    let path = format!(r#"{}\\\ud800.webm""#, &prefix[..prefix.len() - 1]);
    let folder = DataFolder::new(dir.path(), dir.path().join("backup"));
    fs::write(
        folder.path::<Library>(),
        format!(r#"{{"tracks":[{{"path":{path},"added":123,"extension":true,"title":"stale"}}]}}"#),
    )
    .unwrap();
    let mut doc = folder.read::<Library>().unwrap();
    let id = doc.value.tracks[0].id();
    let report = doc.value.scan(std::slice::from_ref(&root), &[]);
    assert_eq!(report.read, 1);
    assert!(report.issues.is_empty(), "{:?}", report.issues);
    let t = &doc.value.tracks[0];
    assert_eq!(t.id(), id);
    assert_eq!(t.tags.format, "webm");
    assert_eq!(t.added, Some(123));
    assert_eq!(t.extra["extension"], true);
    let saved_path = PathBuf::from(&t.path);
    assert_eq!(doc.value.scan(&[], &[saved_path]).reused, 1);
    let mut cold = Library::default();
    assert_eq!(cold.scan(&[], &[song]).read, 1);
    assert_eq!(cold.tracks[0].id(), id);
    doc.save().unwrap();
    assert!(
        fs::read_to_string(folder.path::<Library>())
            .unwrap()
            .contains(r"\ud800.webm")
    );
}
