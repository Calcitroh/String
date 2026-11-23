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

struct Fixture {
    _dir: tempfile::TempDir,
    session: Session,
    page: State,
    ctx: Context,
    ids: Vec<String>,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut library = Library::default();
        for (i, name) in ["a.flac", "b.mp3", "c.mp3", "d.mp3"].iter().enumerate() {
            let path = dir.path().join(name);
            fs::write(&path, b"test song").unwrap();
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
            t.tags.title = "Same".into();
            t.tags.artist = "Artist".into();
            t.tags.duration = 20.;
            t.tags.format = if i == 0 { "flac" } else { "mp3" }.into();
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
    fs::write(&replacement, b"different").unwrap();
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
    let checked = f.act(Intent::Recycle).unwrap();
    let keeper = std::path::Path::new(&f.session.library.tracks[0].path);
    let bin = f._dir.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let moved = checked.recycle(|path| {
        assert!(fs::OpenOptions::new().write(true).open(path).is_err());
        assert!(fs::rename(keeper, bin.join("keeper")).is_err());
        fs::rename(path, bin.join(path.file_name().unwrap()))
    });
    assert!(moved.outcomes.iter().all(Result::is_ok));
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
    fs::write(&other, b"different").unwrap();
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
