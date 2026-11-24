//! Matching Spotify plays to the library's songs and artists.

use chrono::NaiveDate;
use string_core::stats::{Listening, Track};
use string_extras::spotify::{Matcher, main_artist, normalize_title};

fn at() -> chrono::NaiveDateTime {
    NaiveDate::from_ymd_opt(2020, 3, 1)
        .unwrap()
        .and_hms_opt(12, 0, 0)
        .unwrap()
}
fn spotify(id: &str, title: &str, artist: &str, secs: f64) -> (Track, f64) {
    (
        Track(format!("spotify:track:{id}"), title.into(), artist.into()),
        secs,
    )
}
fn history(plays: &[(Track, f64)]) -> Listening {
    let mut l = Listening::default();
    for (t, s) in plays {
        l.add_imported(t, *s, at(), "Album");
    }
    l
}
fn song(id: &str, title: &str, artist: &str) -> Track {
    Track(id.into(), title.into(), artist.into())
}
fn secs_of(l: &Listening, id: &str) -> f64 {
    let r = l.range(at().date(), at().date());
    r.tracks
        .iter()
        .find(|(t, _)| t.0 == id)
        .map_or(0., |(_, s)| *s)
}

#[test]
fn normalizes_feat_and_remaster_but_not_live_or_remix() {
    let song = normalize_title("Song");
    for same in [
        "Song (feat. X)",
        "Song [ft. X]",
        "Song (with X)",
        "Song - 2011 Remaster",
        "Song - Remastered 2009",
        "Song (Remastered 2011)",
        "Song - 2011 Remastered Version",
        "song",
        "SONG!",
        "Sóng",
    ] {
        assert_eq!(normalize_title(same), song, "{same}");
    }
    for other in [
        "Song (Live, Remastered)",
        "Song - Live / 2011 Remaster",
        "Song - Live",
        "Song (Remix)",
        "Song (Radio Edit)",
    ] {
        assert_ne!(normalize_title(other), song, "{other}");
    }
}

#[test]
fn main_artist_takes_the_first_credit() {
    for credit in [
        "A, B",
        "A & B",
        "A feat. B",
        "A ft. B",
        "A x B",
        "a X b",
        "A",
    ] {
        assert_eq!(main_artist(credit), "a", "{credit}");
    }
    assert_eq!(main_artist("Max Band"), "max band");
}

#[test]
fn matched_plays_take_the_library_song() {
    let m = Matcher::new([song("lib/song.mp3", "Song", "Artist A")]);
    let h = history(&[
        spotify("1", "Song - 2011 Remaster", "Artist A, Artist B", 100.),
        spotify("2", "Song (feat. X)", "Artist A", 50.),
        spotify("3", "Other", "Artist C", 25.),
    ]);
    let (out, resolved) = m.resolve(&h);
    assert_eq!(secs_of(&out, "lib/song.mp3"), 150.);
    assert!(
        out.tracks()
            .any(|t| *t == song("lib/song.mp3", "Song", "Artist A"))
    );
    assert_eq!(out.tracks().count(), 2);
    assert_eq!((resolved.matched_seconds, resolved.seconds), (150., 175.));
}

#[test]
fn unmatched_plays_keep_spotify_names_but_merge_their_artist() {
    let m = Matcher::new([song("lib/song.mp3", "Song", "Ärtist  A")]);
    let h = history(&[spotify("3", "Other", "artist a", 25.)]);
    let (out, resolved) = m.resolve(&h);
    let t = out.tracks().next().unwrap();
    assert_eq!(
        t,
        &Track("spotify:track:3".into(), "Other".into(), "Ärtist  A".into())
    );
    assert_eq!(resolved.matched_seconds, 0.);
    let r = out.range(at().date(), at().date());
    assert_eq!(r.artists, vec![("Ärtist  A".to_string(), 25.)]);
}

#[test]
fn duplicate_library_copies_pick_the_first() {
    let m = Matcher::new([
        song("a.mp3", "Song", "Artist A"),
        song("b.mp3", "Song", "Artist A"),
    ]);
    let (out, _) = m.resolve(&history(&[spotify("1", "Song", "Artist A", 10.)]));
    assert_eq!(out.tracks().next().unwrap().0, "a.mp3");
}

#[test]
fn a_song_added_later_matches_without_reimport() {
    let h = history(&[spotify("1", "Song", "Artist A", 10.)]);
    let (before, r0) = Matcher::new([]).resolve(&h);
    assert_eq!(before.tracks().next().unwrap().0, "spotify:track:1");
    assert_eq!(r0.matched_seconds, 0.);
    let (after, r1) = Matcher::new([song("new.mp3", "Song", "Artist A")]).resolve(&h);
    assert_eq!(after.tracks().next().unwrap().0, "new.mp3");
    assert_eq!(r1.matched_seconds, 10.);
}

#[test]
fn titles_that_normalize_to_nothing_never_match() {
    let m = Matcher::new([
        song("a.mp3", "!!!", "Artist A"),
        song("b.mp3", "(feat. X)", "Artist A"),
    ]);
    let (out, resolved) = m.resolve(&history(&[spotify("1", "???", "Artist A", 10.)]));
    assert_eq!(out.tracks().next().unwrap().0, "spotify:track:1");
    assert_eq!(resolved.matched_seconds, 0.);
}
