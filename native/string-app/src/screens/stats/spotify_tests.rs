use super::*;
use crate::{
    playback::Session,
    storage::{self, Storage},
};
use chrono::{Days, NaiveDate, NaiveDateTime};
use std::fs;
use string_core::{
    data::{SpotifyHistory, SpotifyMeta},
    library::Library,
    stats::Track,
};

fn at(day: NaiveDate, hour: u32) -> NaiveDateTime {
    day.and_hms_opt(hour, 0, 0).unwrap()
}

fn song(path: &str, title: &str, artist: &str) -> string_core::library::Track {
    let mut t = string_core::library::Track {
        path: path.into(),
        ..Default::default()
    };
    t.tags.title = title.into();
    t.tags.artist = artist.into();
    t
}

/// Spotify plays: the library's "Song" by "Band" (under a remaster title) today and 40 days ago, and a song that
/// isn't in the library yesterday.
fn history(today: NaiveDate) -> SpotifyHistory {
    let mut h = SpotifyHistory::default();
    let mine = Track(
        "spotify:track:song".into(),
        "Song - 2011 Remaster".into(),
        "Band".into(),
    );
    let other = Track(
        "spotify:track:other".into(),
        "Elsewhere".into(),
        "Other".into(),
    );
    h.0.add_imported(&mine, 40., at(today, 9), "Album");
    h.0.add_imported(&other, 300., at(today - Days::new(1), 22), "Away");
    h.0.add_imported(&mine, 1200., at(today - Days::new(40), 12), "Album");
    h.set_meta(SpotifyMeta {
        plays: 3,
        ..Default::default()
    });
    h
}

#[test]
fn stats_include_the_spotify_layer() {
    let dir = tempfile::tempdir().unwrap();
    let folder = storage::folder(dir.path(), true);
    let today = Local::now().date_naive();
    let first = today - Days::new(40);
    let mut library = Library::default();
    library
        .tracks
        .push(song(r"C:\Music\Song.mp3", "Song", "Band"));
    let id = library.tracks[0].id();
    let mut s = Session::new(Default::default(), Default::default(), library);
    s.listening.add(
        &Track(id.clone(), "Song".into(), "Band".into()),
        30.,
        at(today, 9),
    );
    let phone = Track(
        "phone-only".into(),
        "Old Song".into(),
        "Someone Else".into(),
    );
    s.listening
        .add_phone(&phone, 416., at(today - Days::new(3), 18));
    let mut layer = folder.read::<SpotifyHistory>().unwrap();
    layer.value = history(today);
    layer.save().unwrap();
    let revision = s.spotify_revision;
    s.set_spotify(Some(layer), None);
    assert_ne!(s.spotify_revision, revision);
    let stats = StatsView::default();

    let day = Snapshot::read(&s, &stats, Day);
    // 30 s of String's own and 40 s of Spotify today: neither reaches a minute alone, so the streak needs both.
    assert_eq!(
        day.tiles[0],
        ("1 min".to_string(), "Yesterday 5 min".to_string())
    );
    assert_eq!(day.buckets[9].seconds, 70.);
    assert_eq!(
        day.subtitle,
        format!("Since {} · 2-day streak", first.format("%b %-d, %Y"))
    );
    assert_eq!(day.note, "1 min in all · < 1 min from Spotify");

    let all = Snapshot::read(&s, &stats, All);
    assert_eq!(
        all.note,
        "33 min in all · 6 min on your phone · 25 min from Spotify"
    );
    assert_eq!(all.buckets.iter().map(|b| b.seconds).sum::<f64>(), 1986.);
    let songs: Vec<_> = all
        .tracks
        .iter()
        .map(|r| (r.title.as_str(), r.seconds, r.key.clone()))
        .collect();
    assert_eq!(
        songs,
        [
            ("Song", 1270., Some(id.clone())),
            ("Old Song", 416., None),
            ("Elsewhere", 300., None)
        ]
    );
    assert_eq!(all.tracks[1].missing, "Not in your library any more");
    assert_eq!(all.tracks[2].missing, "Not in your library");
    let artists: Vec<_> = all
        .artists
        .iter()
        .map(|r| (r.title.as_str(), r.seconds))
        .collect();
    assert_eq!(
        artists,
        [("Band", 1270.), ("Someone Else", 416.), ("Other", 300.)]
    );

    let summary = s.spotify_summary().unwrap();
    assert_eq!(
        (summary.first, summary.last, summary.seconds, summary.plays),
        (first, today, 1540., 3)
    );
    assert!((summary.matched_share - 1240. / 1540.).abs() < 1e-9);

    // A song added to the library later picks up its Spotify plays when the models rebuild.
    s.library
        .tracks
        .push(song(r"C:\Music\Elsewhere.mp3", "Elsewhere", "Other"));
    let revision = s.spotify_revision;
    s.rebuild();
    assert_ne!(s.spotify_revision, revision);
    let all = Snapshot::read(&s, &stats, All);
    assert_eq!(all.tracks[2].title, "Elsewhere");
    assert_eq!(all.tracks[2].key, Some(s.library.tracks[1].id()));

    let revision = s.spotify_revision;
    s.set_spotify(None, None);
    assert_ne!(s.spotify_revision, revision);
    assert!(s.spotify_view().is_none() && s.spotify_summary().is_none());
    let day = Snapshot::read(&s, &stats, Day);
    assert_eq!(
        day.tiles[0],
        ("< 1 min".to_string(), "Yesterday 0 min".to_string())
    );
    assert_eq!(day.note, "< 1 min in all");
}

#[test]
fn listening_json_is_untouched_by_the_layer() {
    let dir = tempfile::tempdir().unwrap();
    let folder = storage::folder(dir.path(), true);
    let today = Local::now().date_naive();
    let mut own = folder.read::<Listening>().unwrap();
    let mine = Track(r"c:\music\song.mp3".into(), "Song".into(), "Band".into());
    own.value.add(&mine, 30., at(today, 9));
    own.save().unwrap();
    let mut layer = folder.read::<SpotifyHistory>().unwrap();
    layer.value = history(today);
    layer.save().unwrap();
    let path = dir.path().join("listening.json");
    let before = fs::read(&path).unwrap();

    let (mut store, mut s) = Storage::open(&folder, &egui::Context::default()).unwrap();
    assert!(s.spotify_view().is_some());
    let all = Snapshot::read(&s, &StatsView::default(), All);
    assert!(all.note.ends_with("from Spotify"));
    s.rebuild();
    assert!(!s.listening_dirty);
    s.listening_dirty = true;
    assert!(!store.flush(&mut s).failed());
    assert_eq!(fs::read(&path).unwrap(), before);
}

#[test]
fn an_unreadable_spotify_json_doesnt_stop_the_library_opening() {
    let dir = tempfile::tempdir().unwrap();
    let folder = storage::folder(dir.path(), true);
    let mut own = folder.read::<Listening>().unwrap();
    let mine = Track(r"c:\music\song.mp3".into(), "Song".into(), "Band".into());
    own.value.add(&mine, 30., at(Local::now().date_naive(), 9));
    own.save().unwrap();
    let spotify = dir.path().join("spotify.json");
    for broken in [&br#"{"v":1,"days":"#[..], br#"{"v":2,"tracks":[]}"#] {
        fs::write(&spotify, broken).unwrap();
        let (_, s) = Storage::open(&folder, &egui::Context::default()).unwrap();
        assert_eq!(s.listening.days().len(), 1);
        assert!(s.spotify_error.is_some());
        assert!(s.spotify.is_none() && s.spotify_view().is_none());
        assert_eq!(fs::read(&spotify).unwrap(), broken);
    }
    fs::remove_file(&spotify).unwrap();
    let (_, s) = Storage::open(&folder, &egui::Context::default()).unwrap();
    assert!(s.spotify_error.is_none() && s.spotify.is_none());
}

#[test]
fn a_full_size_spotify_layer_opens_and_draws_quickly() {
    let dir = tempfile::tempdir().unwrap();
    let folder = storage::folder(dir.path(), true);
    let mut library = Library::default();
    for i in 0..20_000 {
        let path = format!(r"C:\Music\{i}.mp3");
        let artist = format!("Artist {}", i % 2_000);
        library
            .tracks
            .push(song(&path, &format!("Title {i}"), &artist));
    }
    let first = Local::now().date_naive() - Days::new(2_199);
    let mut layer = folder.read::<SpotifyHistory>().unwrap();
    for n in 0..170_000u64 {
        let i = n * 7 % 20_000;
        let title = if i < 12_000 { "Title" } else { "Elsewhere" };
        let t = Track(
            format!("spotify:track:{i}"),
            format!("{title} {i}"),
            format!("Artist {}", i % 2_000),
        );
        let when = at(first + Days::new(n * 2_200 / 170_000), (n % 24) as u32);
        assert!(layer.value.0.add_imported(&t, 180., when, "Album"));
    }
    layer.save().unwrap();
    let mut s = Session::new(Default::default(), Default::default(), library);

    let t = Instant::now();
    let layer = folder.read::<SpotifyHistory>().unwrap();
    let read = t.elapsed();
    s.set_spotify(Some(layer), None);
    let resolve = t.elapsed() - read;
    let t = Instant::now();
    let all = Snapshot::read(&s, &StatsView::default(), All);
    let snapshot = t.elapsed();
    let t = Instant::now();
    s.rebuild();
    let rebuild = t.elapsed();
    eprintln!(
        "170k Spotify plays: read {read:?}, resolve {resolve:?}, All snapshot {snapshot:?}, rebuild {rebuild:?}"
    );
    assert!(all.note.ends_with("from Spotify"));
    assert!((s.spotify_summary().unwrap().matched_share - 0.6).abs() < 0.01);
    assert!(
        read + resolve < Duration::from_secs(10),
        "{read:?} + {resolve:?}"
    );
    assert!(snapshot < Duration::from_secs(5), "{snapshot:?}");
}
