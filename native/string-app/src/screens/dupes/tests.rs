use super::*;
use serde_json::json;
use std::{
    fs, thread,
    time::{Duration, Instant, UNIX_EPOCH},
};
use string_core::{
    library::{Library, Track},
    stats,
};
use string_extras::edit::Info;

// An ordinary PCM file with readable tags. The first copy has a higher sample rate.
fn recording(title: &[u8; 4], rate: u32) -> Vec<u8> {
    let mut body = b"WAVEfmt \x10\0\0\0\x01\0\x01\0".to_vec();
    body.extend(rate.to_le_bytes());
    body.extend((rate * 2).to_le_bytes());
    body.extend(b"\x02\0\x10\0LIST\x20\0\0\0INFOINAM\x06\0\0\0");
    body.extend(title);
    body.extend(b"\0\0IART\x06\0\0\0Artist");
    body.extend(b"data");
    body.extend((rate * 2).to_le_bytes());
    body.resize(body.len() + rate as usize * 2, 0);
    let mut out = b"RIFF".to_vec();
    out.extend((body.len() as u32).to_le_bytes());
    out.extend(body);
    out
}

struct Fixture {
    _dir: tempfile::TempDir,
    session: Session,
    page: State,
    ctx: Context,
    ids: Vec<String>,
}
impl Fixture {
    fn with_view(&mut self, action: impl FnOnce(&mut State, &mut View<'_>)) {
        use string_platform::{
            errors_log::ErrorLog,
            instance::{self, Claim, Names},
        };
        struct Quiet;
        impl string_audio::Host for Quiet {
            fn next_after(&mut self, _: &string_audio::Track) -> Option<string_audio::Track> {
                None
            }
            fn event(&mut self, _: string_audio::Event) {}
        }
        let args = crate::Args {
            root: self._dir.path().into(),
            explicit: true,
            smoke: Some(self._dir.path().into()),
            quit: false,
            add: None,
            playlist: None,
        };
        let Claim::First(instance) = instance::claim_quietly(&Names::custom(&format!(
            "StringCopiesTest{:x}",
            rand::random::<u64>()
        )))
        .unwrap() else {
            panic!()
        };
        let adds = crate::add_work::Adds::new().unwrap();
        action(
            &mut self.page,
            &mut View {
                adds: &adds.input,
                platform: &mut crate::platform::Platform::new(
                    instance,
                    &args,
                    std::sync::Arc::new(ErrorLog::new(self._dir.path())),
                ),
                s: &mut self.session,
                player: &string_audio::Player::new(
                    string_audio::Config {
                        muted: true,
                        ..Default::default()
                    },
                    Quiet,
                ),
                st: Default::default(),
                views: &mut Default::default(),
                covers: &mut ui::covers::CoverCache::new(&self.ctx),
                viz: &Default::default(),
                scan: &mut Default::default(),
                tint: Tint::Mono,
                mono: true,
                force_mute: true,
            },
        );
    }
    fn new() -> Self {
        Self::in_dir(tempfile::tempdir().unwrap())
    }
    fn in_dir(dir: tempfile::TempDir) -> Self {
        let mut library = Library::default();
        for (i, name) in ["a.wav", "b.wav", "c.wav", "d.wav"].iter().enumerate() {
            let path = dir.path().join(name);
            fs::write(&path, recording(b"Same", if i == 0 { 16000 } else { 8000 })).unwrap();
            let m = fs::metadata(&path).unwrap();
            let mut t = Track {
                path: path.to_str().unwrap().to_owned(),
                size: m.len(),
                mtime: m
                    .modified()
                    .unwrap()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as i64,
                ..Track::default()
            };
            t.tags = string_core::tags::read(&path).unwrap().tags;
            assert_eq!(t.tags.title, "Same");
            library.tracks.push(t);
        }
        let ids: Vec<_> = library.tracks.iter().map(Track::id).collect();
        let mut session = Session::new(Default::default(), Default::default(), library);
        session.set_field("hidden", json!({(ids[3].clone()):1}));
        session.set_field("liked", json!({(ids[1].clone()):2}));
        session.set_field("loops", json!({(ids[1].clone()):{"a":2,"b":7}}));
        session.set_field("plays", json!({(ids[1].clone()):[3,100]}));
        session.new_playlist("Mix", &ids[0..2]);
        session.listening.add(
            &stats::Track(ids[1].clone(), "Same".into(), "Artist".into()),
            17.,
            Local::now().naive_local(),
        );
        session.rebuild();
        let mut f = Self {
            _dir: dir,
            session,
            page: State::default(),
            ctx: Context::default(),
            ids,
        };
        f.refresh();
        assert_eq!(f.page.drop.len(), 2);
        f
    }
    fn refresh(&mut self) {
        let start = Instant::now();
        loop {
            self.page.refresh(&self.ctx, &self.session);
            if self.page.looking.is_none() {
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(10));
            thread::sleep(Duration::from_millis(2));
        }
    }
    fn history(&self) -> Value {
        json!([
            self.session.field("liked"),
            self.session.field("plays"),
            self.session.field("loops"),
            self.session.field("playlists"),
            self.session.listening
        ])
    }
    fn act(&mut self, intent: Intent) -> Option<Checked> {
        self.page.request(&self.session, intent);
        let start = Instant::now();
        while self.page.checking.is_some() {
            if let Some((_, checked)) = self.page.checked(&self.session) {
                return Some(checked);
            }
            assert!(start.elapsed() < Duration::from_secs(10));
            thread::sleep(Duration::from_millis(2));
        }
        None
    }
    fn hide(&mut self) {
        if let Some(checked) = self.act(Intent::Hide) {
            State::merge_hidden(&mut self.session, &checked.plan);
        }
    }
}

#[test]
fn audit_renamed_copy_leaves_group_and_selection() {
    let mut f = Fixture::new();
    let history = f.history();
    crate::ui::edit_info::save_info(
        &mut f.session,
        &f.ids[1],
        &Info {
            title: "Another song".into(),
            ..Info::default()
        },
    );
    f.refresh();
    assert_eq!(f.history(), history);
    assert!(!f.page.drop.contains(&f.ids[1]));
    assert!(
        !f.page
            .groups
            .iter()
            .any(|g| g.found.ids.contains(&f.ids[1]))
    );
}
#[test]
fn audit_hide_after_scan_changes_one_copy_is_all_or_nothing() {
    let mut f = Fixture::new();
    let history = f.history();
    f.session.library.tracks[2].tags.duration += 30.;
    f.session.rebuild();
    f.hide();
    assert_eq!(f.history(), history);
    assert!(!f.session.state.hidden(&f.ids[1]));
    assert!(!f.page.drop.contains(&f.ids[2]));
}
#[test]
fn audit_equal_hidden_counts_replace_the_group_member() {
    let mut f = Fixture::new();
    let history = f.history();
    f.session.set_field("hidden", json!({(f.ids[1].clone()):2}));
    f.session.rebuild();
    f.refresh();
    assert_eq!(f.history(), history);
    assert!(!f.page.drop.contains(&f.ids[1]));
    assert!(f.page.drop.contains(&f.ids[3]));
}
#[test]
fn audit_file_changed_without_scan_cancels_whole_hide() {
    let mut f = Fixture::new();
    let history = f.history();
    fs::write(&f.session.library.tracks[2].path, b"another recording").unwrap();
    f.hide();
    assert_eq!(f.history(), history);
    assert!(!f.page.drop.contains(&f.ids[2]));
    f.refresh();
    assert!(!f.page.drop.contains(&f.ids[2]));
}
#[test]
fn audit_webm_can_be_picked_but_only_audio_files_can_be_recycled() {
    use crate::integration::files::recyclable_song;
    assert!(recyclable_song(std::path::Path::new("song.WeBa")));
    assert!(!recyclable_song(std::path::Path::new("song.WeBm")));
}

#[test]
fn confirmation_rechecks_the_kept_file_and_cancels_all_moves() {
    let mut f = Fixture::new();
    let history = f.history();
    assert_eq!(f.act(Intent::Ask).unwrap().plan.len(), 2);
    // Replace the keeper with the same length and last-write time, as a restore tool can do.
    let path = &f.session.library.tracks[0].path;
    let replacement = f._dir.path().join("replacement");
    fs::write(&replacement, recording(b"Else", 16000)).unwrap();
    let modified = fs::metadata(path).unwrap().modified().unwrap();
    fs::File::options()
        .write(true)
        .open(&replacement)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    fs::rename(replacement, path).unwrap();
    assert!(f.act(Intent::Recycle).is_none());
    assert!(f.page.drop.is_empty());
    assert_eq!(f.history(), history);
    assert!(
        f.session
            .library
            .tracks
            .iter()
            .all(|t| std::path::Path::new(&t.path).exists())
    );
}

#[test]
fn confirmation_cannot_adopt_a_new_list_or_an_indirect_match() {
    let mut f = Fixture::new();
    let history = f.history();
    assert!(f.act(Intent::Ask).is_some());
    f.page.ask = true;
    f.session.library.tracks[2].tags.duration += 40.;
    f.session.rebuild();
    f.refresh();
    assert!(!f.page.ask);
    assert!(f.act(Intent::Recycle).is_none());
    // All three are connected through the middle song, but the two ends differ by four seconds.
    for (i, track) in f.session.library.tracks.iter_mut().enumerate() {
        track.tags.duration = 20. + 2. * i as f64;
    }
    f.session.rebuild();
    f.refresh();
    f.page.keep[0] = f.ids[0].clone();
    f.page.drop.insert(f.ids[2].clone());
    assert!(f.act(Intent::Hide).is_none());
    assert_eq!(f.history(), history);
}

#[test]
fn unchanged_copies_can_still_merge() {
    let mut f = Fixture::new();
    let history = f.history();
    f.hide();
    assert_ne!(f.history(), history);
    assert!(f.session.state.hidden(&f.ids[1]) && f.session.state.hidden(&f.ids[2]));
    assert_eq!(f.session.field("plays")[&f.ids[0]][0], 3);
}

#[test]
fn pending_file_checks_discard_a_new_revision() {
    let mut f = Fixture::new();
    let history = f.history();
    f.page.request(&f.session, Intent::Recycle);
    assert!(f.page.checking.is_some());
    f.session.library.tracks[1].tags.title = "New title".into();
    f.session.rebuild();
    let start = Instant::now();
    while f.page.checking.is_some() {
        assert!(f.page.checked(&f.session).is_none());
        assert!(start.elapsed() < Duration::from_secs(10));
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(f.history(), history);
    assert!(f.page.drop.is_empty());
}

#[test]
fn mixed_webm_batch_is_refused_before_confirmation_but_can_be_hidden() {
    let mut f = Fixture::new();
    let old = &f.session.library.tracks[2].path;
    let path = std::path::Path::new(old).with_extension("WEBM");
    fs::rename(old, &path).unwrap();
    f.session.library.tracks[2].path = path.to_str().unwrap().to_owned();
    f.session.rebuild();
    f.refresh();
    let history = f.history();
    assert!(f.act(Intent::Ask).is_none());
    assert_eq!(f.history(), history);
    assert!(path.is_file());
    f.refresh();
    let id = f.session.library.tracks[2].id();
    assert!(!f.page.drop.contains(&id));
    f.page.drop.insert(id.clone());
    f.hide();
    assert!(f.session.state.hidden(&id));
    assert!(path.is_file());
}

#[test]
fn twelve_thousand_songs_reuse_the_completed_look_on_idle_frames() {
    let mut f = Fixture::new();
    for i in 0..11_996 {
        let mut t = Track {
            path: format!("library/track{i}.mp3"),
            ..Track::default()
        };
        t.tags.title = format!("Recording {i}");
        f.session.library.tracks.push(t);
    }
    f.session.rebuild();
    f.refresh();
    let groups = f.page.groups.as_ptr();
    for _ in 0..12_000 {
        f.page.refresh(&f.ctx, &f.session);
        assert!(f.page.looking.is_none());
        assert_eq!(f.page.groups.as_ptr(), groups);
    }
}

#[test]
fn confirmed_batch_moves_only_validated_files_and_guards_the_keeper() {
    let mut f = Fixture::new();
    assert!(f.act(Intent::Ask).is_some());
    let mut checked = f.act(Intent::Recycle).unwrap();
    let keeper = std::path::Path::new(&f.session.library.tracks[0].path);
    let bin = f._dir.path().join("bin");
    fs::create_dir(&bin).unwrap();
    for index in 0..checked.plan.len() {
        checked
            .move_one(
                index,
                |path| {
                    assert!(fs::OpenOptions::new().write(true).open(path).is_err());
                    assert!(fs::rename(keeper, bin.join("keeper")).is_err());
                    fs::rename(path, bin.join(path.file_name().unwrap()))
                },
                |path| path.parent().unwrap().ends_with("bin"),
            )
            .unwrap();
    }
    assert_eq!(fs::read_dir(bin).unwrap().count(), 2);
    assert!(keeper.is_file());
}

#[test]
fn replacement_with_cached_size_and_time_forces_its_tags_to_be_read_again() {
    let mut f = Fixture::new();
    let history = f.history();
    let path = &f.session.library.tracks[2].path;
    let stamp = fs::metadata(path).unwrap().modified().unwrap();
    let other = f._dir.path().join("replacement");
    fs::write(&other, recording(b"Else", 8000)).unwrap();
    fs::File::options()
        .write(true)
        .open(&other)
        .unwrap()
        .set_modified(stamp)
        .unwrap();
    fs::rename(other, path).unwrap();
    assert!(f.act(Intent::Hide).is_none());
    let mut scan = crate::scan::Scanner::default();
    f.session.set_field("roots", json!([f._dir.path()]));
    scan.reread_files(std::mem::take(&mut f.page.reread));
    let start = Instant::now();
    while scan.requested || scan.running() {
        scan.poll(&mut f.session, &f.ctx);
        assert!(start.elapsed() < Duration::from_secs(30));
        thread::sleep(Duration::from_millis(2));
    }
    f.refresh();
    assert!(
        !f.page
            .groups
            .iter()
            .any(|g| g.found.ids.contains(&f.ids[2]))
    );
    assert!(
        f.page
            .groups
            .iter()
            .any(|g| g.found.ids.contains(&f.ids[1]))
    );
    assert_eq!(f.history(), history);
}

#[test]
fn review_replacement_before_look_cannot_inherit_cached_tags() {
    let mut f = Fixture::new();
    let history = f.history();
    let path = f.session.library.tracks[1].path.clone();
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    let other = f._dir.path().join("restored.wav");
    fs::write(&other, recording(b"Else", 8000)).unwrap();
    fs::File::options()
        .write(true)
        .open(&other)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    fs::rename(other, &path).unwrap();
    f.page = State::default();
    f.refresh();
    f.hide();
    assert_eq!(f.history(), history);
    assert!(!f.session.state.hidden(&f.ids[1]));
    assert!(f.page.reread.contains(&path));
}

#[test]
fn review_shell_success_for_a_substituted_file_must_not_authorize_a_merge() {
    let mut f = Fixture::new();
    let history = f.history();
    f.page.drop = HashSet::from([f.ids[1].clone()]);
    let mut checked = f.act(Intent::Recycle).unwrap();
    let saved = f._dir.path().join("saved-original.wav");
    let replacement = f._dir.path().join("other.wav");
    fs::write(&replacement, recording(b"Else", 8000)).unwrap();
    let moved = checked.move_one(
        0,
        |path| {
            fs::rename(path, &saved)?;
            fs::rename(&replacement, path)?;
            fs::rename(path, f._dir.path().join("wrong-bin-item.wav"))
        },
        |path| path.ends_with("wrong-bin-item.wav"),
    );
    assert!(moved.is_err());
    let original = f.session.library.tracks[1].clone();
    f.with_view(|page, v| {
        let mut progress = Progress::default();
        page.finish_move(
            &checked.plan[0],
            &original,
            moved.map_err(|e| e.to_string()),
            &mut progress,
            v,
        );
        assert_eq!((progress.moved, progress.failed.len()), (0, 1));
    });
    assert_eq!(f.history(), history);
}

#[test]
#[ignore = "uses the Windows Recycle Bin and restores one generated song"]
fn windows_recycle_restores_the_checked_song_to_its_original_path() {
    use std::os::windows::process::CommandExt;
    let runs = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-runs");
    fs::create_dir_all(&runs).unwrap();
    let mut f = Fixture::in_dir(tempfile::tempdir_in(std::path::absolute(runs).unwrap()).unwrap());
    f.page.drop = HashSet::from([f.ids[1].clone()]);
    let mut checked = f.act(Intent::Recycle).unwrap();
    let source = PathBuf::from(&checked.plan[0].path);
    let before = fs::read(&source).unwrap();
    let moved = checked.recycle(0, string_platform::files::recycle);
    drop(checked);
    if !source.exists() {
        // Restrict Restore to this fixture's unique directory and original filename.
        let restore = std::process::Command::new("powershell.exe")
            .creation_flags(0x08000000)
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                r#"
$ErrorActionPreference = 'Stop'
$bin = (New-Object -ComObject Shell.Application).Namespace(10)
$items = @($bin.Items() | Where-Object {
    $_.ExtendedProperty('System.Recycle.DeletedFrom') -eq $env:STRING_TEST_RESTORE_DIR -and
    $_.ExtendedProperty('System.FileName') -eq $env:STRING_TEST_RESTORE_NAME
})
if ($items.Count -ne 1) { throw 'Expected one matching disposable song' }
$items[0].InvokeVerb('undelete')
"#,
            ])
            .env(
                "STRING_TEST_RESTORE_DIR",
                source
                    .parent()
                    .unwrap()
                    .as_os_str()
                    .to_string_lossy()
                    .trim_start_matches(r"\\?\"),
            )
            .env("STRING_TEST_RESTORE_NAME", source.file_name().unwrap())
            .output()
            .unwrap();
        assert!(
            restore.status.success(),
            "{}",
            String::from_utf8_lossy(&restore.stderr)
        );
        let until = Instant::now() + Duration::from_secs(5);
        while !source.exists() && Instant::now() < until {
            thread::sleep(Duration::from_millis(50));
        }
    }
    assert_eq!(fs::read(source).unwrap(), before);
    moved.unwrap();
}

#[test]
fn review_edit_after_first_move_preserves_the_pending_file() {
    let mut f = Fixture::new();
    let mut checked = f.act(Intent::Recycle).unwrap();
    let pending = checked.plan[1].clone();
    let bin = f._dir.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let mut progress = Progress::default();
    let (at, _) = f
        .page
        .next_pair(&checked, &mut progress, &f.session)
        .unwrap();
    checked
        .move_one(
            at,
            |path| fs::rename(path, bin.join(path.file_name().unwrap())),
            |path| path.parent().unwrap().ends_with("bin"),
        )
        .unwrap();
    crate::ui::edit_info::save_info(
        &mut f.session,
        &pending.drop_id,
        &Info {
            title: "Other song".into(),
            ..Default::default()
        },
    );
    assert!(
        f.page
            .next_pair(&checked, &mut progress, &f.session)
            .is_none()
    );
    assert!(std::path::Path::new(&pending.path).is_file());
    assert_eq!(progress.failed.len(), 1);
    assert!(f.page.unticked.contains(&pending.drop_id));
}

#[test]
fn review_scan_observing_a_move_preserves_its_history() {
    let mut f = Fixture::new();
    let mut checked = f.act(Intent::Recycle).unwrap();
    let bin = f._dir.path().join(".bin");
    fs::create_dir(&bin).unwrap();
    for at in 0..checked.plan.len() {
        checked
            .move_one(
                at,
                |path| fs::rename(path, bin.join(path.file_name().unwrap())),
                |path| path.parent().unwrap().ends_with(".bin"),
            )
            .unwrap();
    }
    let originals = f.session.library.tracks.clone();
    let paths: Vec<PathBuf> = f
        .session
        .library
        .tracks
        .iter()
        .map(|t| PathBuf::from(&t.path))
        .collect();
    assert_eq!(f.session.library.scan(&[], &paths).removed, 2);
    f.session.rebuild();
    f.with_view(|page, v| {
        let mut progress = Progress::default();
        for copy in &checked.plan {
            let original = originals.iter().find(|t| t.path == copy.path).unwrap();
            page.finish_move(copy, original, Ok(()), &mut progress, v);
        }
        assert_eq!(progress.moved, 2);
    });
    let keep = &f.ids[0];
    assert_eq!(f.session.field("plays")[keep][0], 3);
    assert_eq!(f.session.field("liked")[keep], 2);
    assert_eq!(f.session.field("loops")[keep]["b"], 7);
    assert_eq!(
        f.session.field("playlists")[0]["items"][0]["id"],
        keep.as_str()
    );
    let listening = json!(f.session.listening);
    assert_eq!(listening["tracks"][0][0], keep.as_str());
    assert_eq!(
        listening["days"]
            .as_object()
            .unwrap()
            .values()
            .next()
            .unwrap()["t"]["0"],
        17.
    );
    assert!(
        !f.history()
            .to_string()
            .contains(&json!(f.ids[1]).to_string())
    );
}

#[test]
fn review_refresh_keeps_the_chosen_keeper_and_unticked_copy() {
    let mut f = Fixture::new();
    f.page.choose_keep(0, f.ids[1].clone());
    f.page.choose_drop(f.ids[2].clone(), false);
    f.session.plays_revision += 1;
    f.refresh();
    assert_eq!(f.page.keep, [f.ids[1].clone()]);
    assert_eq!(f.page.drop, HashSet::from([f.ids[0].clone()]));
}

#[test]
fn missing_recycled_row_cannot_authorize_a_changed_keeper() {
    let mut f = Fixture::new();
    let history = f.history();
    f.page.drop = HashSet::from([f.ids[1].clone()]);
    let mut checked = f.act(Intent::Recycle).unwrap();
    checked
        .move_one(
            0,
            |source| fs::rename(source, f._dir.path().join("moved.wav")),
            |path| path.ends_with("moved.wav"),
        )
        .unwrap();
    let original = f.session.library.tracks.remove(1);
    f.session.library.tracks[0].tags.title = "Changed keeper".into();
    f.session.rebuild();
    f.with_view(|page, v| {
        let mut progress = Progress::default();
        page.finish_move(&checked.plan[0], &original, Ok(()), &mut progress, v);
        assert_eq!((progress.moved, progress.failed.len()), (1, 1));
    });
    assert_eq!(f.history(), history);
}

#[test]
fn scanner_waits_for_the_move_and_discards_its_pre_move_result() {
    let mut f = Fixture::new();
    let copy = f.session.library.tracks[1].path.clone();
    f.session.set_field("roots", json!([f._dir.path()]));
    let mut scan = crate::scan::Scanner::default();
    scan.requested = true;
    scan.poll(&mut f.session, &f.ctx);
    assert!(scan.running());
    scan.recycling = true;
    fs::rename(copy, f._dir.path().join("moved.out")).unwrap();
    f.session.removals += 1;
    scan.poll(&mut f.session, &f.ctx);
    assert_eq!(scan.completed, 0);
    scan.recycling = false;
    let start = Instant::now();
    while scan.requested || scan.running() {
        scan.poll(&mut f.session, &f.ctx);
        assert!(start.elapsed() < Duration::from_secs(30));
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(scan.completed, 2);
    assert!(!f.session.models.by_id.contains_key(&f.ids[1]));
}

#[test]
fn review_drop_resolves_dragged_ids_after_a_scan() {
    let mut f = Fixture::new();
    let list = f.session.new_playlist("Destination", &[]);
    let mut selection = ui::songs::Selection::default();
    selection.bind(&f.ids[..2].to_vec().into(), None);
    selection.choose(0, false, false);
    selection.choose(1, true, false);
    let payload = selection.ids();
    f.session.library.tracks.remove(1);
    f.session.rebuild();
    selection.bind(&f.ids[..1].to_vec().into(), None);
    f.with_view(|_, v| ui::side_playlists::drop_songs(v, &list, &payload));
    let dest = f
        .session
        .field("playlists")
        .as_array()
        .unwrap()
        .iter()
        .find(|pl| pl["id"] == list)
        .unwrap();
    assert_eq!(dest["items"].as_array().unwrap().len(), 1);
    assert_eq!(dest["items"][0]["id"], f.ids[0]);
}
