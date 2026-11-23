use crate::{
    route::{History, Route},
    storage::{self, Storage},
    ui::songs::Selection,
};
use std::{fs, path::PathBuf};
fn fixture() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-runs/profile-checks")
        .join(format!("{:016x}", rand::random::<u64>()));
    fs::create_dir_all(&dir).unwrap();
    dir
}
#[test]
fn damaged_document_blocks_all_profile_writes_and_can_be_retried() {
    let dir = fixture();
    let folder = storage::folder(&dir, true);
    let state = br#"{"v":1,"untouched":"keep"}"#;
    fs::write(dir.join("state.json"), state).unwrap();
    fs::write(dir.join("listening.json"), b"broken").unwrap();
    let ctx = egui::Context::default();
    assert!(Storage::open(&folder, &ctx).is_err());
    assert_eq!(fs::read(dir.join("state.json")).unwrap(), state);
    assert!(!dir.join("library.json").exists());
    fs::write(
        dir.join("listening.json"),
        br#"{"v":1,"tracks":[],"artists":[],"days":{}}"#,
    )
    .unwrap();
    let (mut writer, mut s) = Storage::open(&folder, &ctx).unwrap();
    s.set_field("route", "liked".into());
    writer.schedule(&mut s);
    s.set_field("route", "songs".into());
    assert!(!writer.flush(&mut s).failed());
    let loaded = folder.open().unwrap();
    assert_eq!(loaded.state.value.fields["route"], "songs");
    assert_eq!(loaded.state.value.fields["untouched"], "keep");
    assert!(dir.join("backup").read_dir().unwrap().count() > 0);
}
#[test]
fn close_refuses_to_overwrite_newer_disk_state() {
    let dir = fixture();
    let folder = storage::folder(&dir, true);
    let ctx = egui::Context::default();
    let (mut writer, mut s) = Storage::open(&folder, &ctx).unwrap();
    let replacement = br#"{"v":1,"external":"keep"}"#;
    fs::write(dir.join("state.json"), replacement).unwrap();
    s.set_field("vol", 0.2.into());
    let report = writer.flush(&mut s);
    assert!(report.failed() && s.state_dirty);
    assert_eq!(fs::read(dir.join("state.json")).unwrap(), replacement);
}
#[test]
fn route_keys_and_range_selection_survive_navigation() {
    let r = Route::Album("Live / Session\u{1}été%".into());
    assert_eq!(Route::parse(&r.path()), r);
    let mut h = History::default();
    h.go(r.clone());
    h.go(Route::Liked);
    h.back();
    assert_eq!(h.current, r);
    h.forward();
    assert_eq!(h.current, Route::Liked);
    h.back();
    h.go(Route::Settings);
    assert!(!h.has_forward());
    let mut s = Selection::default();
    s.choose(4, false, false);
    s.choose(6, false, true);
    s.choose(1, true, false);
    s.choose(5, true, false);
    assert_eq!(s.indices.into_iter().collect::<Vec<_>>(), [1, 4, 6]);
}
#[test]
fn missing_opening_copy_allows_saves_and_a_damaged_profile_keeps_no_copy() {
    let today = chrono::Local::now().date_naive().to_string();
    // A file occupies the opening-copy directory, but the live profile can still be saved.
    let dir = fixture();
    let folder = storage::folder(&dir, true);
    fs::write(dir.join("state.json"), br#"{"v":1}"#).unwrap();
    fs::create_dir_all(dir.join("backup")).unwrap();
    fs::write(dir.join("backup").join("opened"), b"in the way").unwrap();
    let ctx = egui::Context::default();
    let (mut store, mut session) = Storage::open(&folder, &ctx).unwrap();
    session.set_field("route", "songs".into());
    assert!(!store.flush(&mut session).failed());
    assert_eq!(folder.open().unwrap().state.value.fields["route"], "songs");
    // a damaged file: no copy is kept for the day, so the retry after it's fixed keeps the good one
    let dir = fixture();
    let folder = storage::folder(&dir, true);
    fs::write(dir.join("state.json"), br#"{"v":1,"mine":true}"#).unwrap();
    fs::write(dir.join("listening.json"), b"broken").unwrap();
    assert!(Storage::open(&folder, &ctx).is_err());
    assert!(!dir.join("backup").join("opened").join(&today).exists());
    fs::write(
        dir.join("listening.json"),
        br#"{"v":1,"artists":[],"tracks":[],"days":{}}"#,
    )
    .unwrap();
    assert!(Storage::open(&folder, &ctx).is_ok());
    let kept = dir.join("backup").join("opened").join(&today);
    assert_eq!(
        fs::read(kept.join("listening.json")).unwrap(),
        fs::read(dir.join("listening.json")).unwrap()
    );
}
