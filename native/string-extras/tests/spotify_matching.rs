//! Matching Spotify plays to the library's songs and artists.

use chrono::NaiveDate;
use string_core::stats::{Listening, Track};
use string_extras::spotify::{LibrarySong, Matcher, main_artist, normalize_title};

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
fn on_album(id: &str, title: &str, artist: &str, album: &str) -> LibrarySong {
    LibrarySong {
        track: song(id, title, artist),
        album: album.into(),
    }
}
/// Plays with their Spotify album: (id, title, artist, album, seconds).
fn history_on(plays: &[(&str, &str, &str, &str, f64)]) -> Listening {
    let mut l = Listening::default();
    for &(id, title, artist, album, secs) in plays {
        let t = Track(format!("spotify:track:{id}"), title.into(), artist.into());
        l.add_imported(&t, secs, at(), album);
    }
    l
}
fn artist_secs(l: &Listening, name: &str) -> f64 {
    let r = l.range(at().date(), at().date());
    r.artists
        .iter()
        .find(|(a, _)| a == name)
        .map_or(0., |(_, s)| *s)
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
    let (before, r0) = Matcher::new(Vec::<Track>::new()).resolve(&h);
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

#[test]
fn the_same_title_on_the_same_album_matches_under_another_spelling_of_the_artist() {
    let m = Matcher::new([on_album(
        "lib/a.flac",
        "Ending Theme",
        "Rin Aoba",
        "Night Ferry OST",
    )]);
    let h = history_on(&[("1", "Ending Theme", "青葉 凛", "Night Ferry OST", 40.)]);
    let (out, resolved) = m.resolve(&h);
    assert_eq!(secs_of(&out, "lib/a.flac"), 40.);
    assert_eq!(resolved.matched_seconds, 40.);
}

#[test]
fn a_single_named_after_its_song_proves_nothing() {
    let m = Matcher::new([on_album("lib/kiss.mp3", "Kiss", "Band A", "Kiss")]);
    let h = history_on(&[("1", "Kiss", "Band B", "Kiss", 30.)]);
    let (out, resolved) = m.resolve(&h);
    assert_eq!(out.tracks().next().unwrap().0, "spotify:track:1");
    assert_eq!(resolved.matched_seconds, 0.);
    assert_eq!(artist_secs(&out, "Band B"), 30.);
}

#[test]
fn two_proven_songs_make_two_spellings_one_artist() {
    let m = Matcher::new([
        on_album("lib/1.flac", "Opening", "Rin Aoba", "Night Ferry OST"),
        on_album("lib/2.flac", "Harbor", "Rin Aoba", "Night Ferry OST"),
        on_album("lib/3.flac", "Lantern", "Rin Aoba", "Best of Rin Aoba"),
    ]);
    let h = history_on(&[
        ("1", "Opening", "青葉 凛", "Night Ferry OST", 10.),
        ("2", "Harbor", "青葉 凛", "Night Ferry OST", 20.),
        // On another album in Spotify: matched by title, now that the artist is known.
        ("3", "Lantern", "青葉 凛", "Lantern", 30.),
        // Not in the library: still counted under the library's spelling.
        ("4", "Tide", "青葉 凛", "Tide", 40.),
    ]);
    let (out, resolved) = m.resolve(&h);
    assert_eq!(secs_of(&out, "lib/3.flac"), 30.);
    assert_eq!(resolved.matched_seconds, 60.);
    assert_eq!(secs_of(&out, "spotify:track:4"), 40.);
    assert_eq!(artist_secs(&out, "Rin Aoba"), 100.);
    assert_eq!(artist_secs(&out, "青葉 凛"), 0.);
}

#[test]
fn one_proven_song_does_not_join_two_artists() {
    let m = Matcher::new([
        on_album("lib/1.flac", "Glass Tower", "Studio Band", "Game OST"),
        on_album("lib/2.flac", "Old Pier", "Studio Band", "Another OST"),
    ]);
    let h = history_on(&[
        ("1", "Glass Tower", "Big Label Music", "Game OST", 10.),
        ("2", "Old Pier", "Big Label Music", "Label Hits", 20.),
    ]);
    let (out, resolved) = m.resolve(&h);
    assert_eq!(secs_of(&out, "lib/1.flac"), 10.);
    assert_eq!(secs_of(&out, "spotify:track:2"), 20.);
    assert_eq!(resolved.matched_seconds, 10.);
    assert_eq!(artist_secs(&out, "Big Label Music"), 20.);
}

#[test]
fn a_spelling_joins_the_artist_with_the_most_proof_and_a_tie_joins_none() {
    let lib = |extra: bool| {
        let mut songs = vec![
            on_album("k1", "One", "Rin Aoba", "OST"),
            on_album("k2", "Two", "Rin Aoba", "OST"),
            on_album("k3", "Three", "Rin Aoba", "OST"),
            on_album("v1", "Four", "Mia Sato", "OST"),
            on_album("v2", "Five", "Mia Sato", "OST"),
        ];
        if extra {
            songs.push(on_album("v3", "Six", "Mia Sato", "OST"));
        }
        songs
    };
    let plays = |n: usize| {
        let all = [
            ("1", "One", "青葉 凛", "OST", 1.),
            ("2", "Two", "青葉 凛", "OST", 1.),
            ("3", "Three", "青葉 凛", "OST", 1.),
            ("4", "Four", "青葉 凛", "OST", 1.),
            ("5", "Five", "青葉 凛", "OST", 1.),
            ("6", "Six", "青葉 凛", "OST", 1.),
            ("7", "Not Here", "青葉 凛", "Elsewhere", 5.),
        ];
        history_on(&[&all[..n], &all[6..]].concat())
    };
    // Three proven songs for Rin Aoba, two for the singer credited on two of them: Rin Aoba.
    let (out, _) = Matcher::new(lib(false)).resolve(&plays(5));
    assert_eq!(artist_secs(&out, "Rin Aoba"), 8.);
    assert_eq!(artist_secs(&out, "Mia Sato"), 2.);
    // Three each: no telling, so the Spotify spelling stays.
    let (out, _) = Matcher::new(lib(true)).resolve(&plays(6));
    assert_eq!(artist_secs(&out, "青葉 凛"), 5.);
}

fn linked(plays: &[(&str, &str, &str, &str, f64)], links: serde_json::Value) -> Listening {
    let mut h = history_on(plays);
    h.set_top_field("sameAs", links);
    h
}

#[test]
fn a_link_set_by_hand_counts_another_recording_as_a_library_song() {
    let m = Matcher::new([on_album(
        "lib/theme.flac",
        "Hero MainTheme",
        "Composer A",
        "Show OST",
    )]);
    let h = linked(
        &[
            (
                "1",
                "Hero (Main Theme)",
                "Cover Artist",
                "Hero (Main Theme)",
                60.,
            ),
            (
                "2",
                "Hero (Main Theme)",
                "Cover Artist",
                "Hero (Main Theme)",
                30.,
            ),
            (
                "3",
                "Hero (Main Theme)",
                "Another Cover",
                "Hero (Main Theme)",
                5.,
            ),
        ],
        serde_json::json!([{
            "title": "hero (main theme)",
            "artist": "COVER ARTIST",
            "as": { "title": "Hero MainTheme", "artist": "Composer A" }
        }]),
    );
    let (out, resolved) = m.resolve(&h);
    assert_eq!(secs_of(&out, "lib/theme.flac"), 90.);
    assert_eq!(resolved.matched_seconds, 90.);
    assert_eq!(artist_secs(&out, "Composer A"), 90.);
    assert_eq!(artist_secs(&out, "Another Cover"), 5.);
}

#[test]
fn a_link_to_a_song_the_library_no_longer_has_changes_nothing() {
    let m = Matcher::new([song("lib/other.flac", "Other", "Composer A")]);
    let h = linked(
        &[("1", "Hero (Main Theme)", "Cover Artist", "Single", 60.)],
        serde_json::json!([{
            "title": "Hero (Main Theme)",
            "artist": "Cover Artist",
            "as": { "title": "Hero MainTheme", "artist": "Composer A" }
        }]),
    );
    let (out, resolved) = m.resolve(&h);
    assert_eq!(secs_of(&out, "spotify:track:1"), 60.);
    assert_eq!(resolved.matched_seconds, 0.);
    assert_eq!(artist_secs(&out, "Cover Artist"), 60.);
}

#[test]
fn damaged_links_are_passed_over() {
    let m = Matcher::new([song("lib/song.flac", "Song", "Artist A")]);
    for links in [
        serde_json::json!("not a list"),
        serde_json::json!([1, null, {"title": "Song"}, {"title": 5, "artist": "x", "as": {}}]),
    ] {
        let h = linked(&[("1", "Song", "Artist A", "Album", 10.)], links);
        let (out, resolved) = m.resolve(&h);
        assert_eq!(secs_of(&out, "lib/song.flac"), 10.);
        assert_eq!(resolved.matched_seconds, 10.);
    }
}
