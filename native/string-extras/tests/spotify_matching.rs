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
        "Song (feat. X - Live)",
        "Song (with X - Remix)",
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

#[test]
fn two_songs_on_one_album_match_a_name_written_in_another_script() {
    let m = Matcher::new([
        on_album("lib/a.flac", "Ending Theme", "Rin Aoba", "Night Ferry OST"),
        on_album("lib/b.flac", "Opening Theme", "Rin Aoba", "Night Ferry OST"),
    ]);
    let h = history_on(&[
        ("1", "Ending Theme", "青葉 凛", "Night Ferry OST", 40.),
        ("2", "Opening Theme", "青葉 凛", "Night Ferry OST", 20.),
    ]);
    let (out, resolved) = m.resolve(&h);
    assert_eq!(secs_of(&out, "lib/a.flac"), 40.);
    assert_eq!(secs_of(&out, "lib/b.flac"), 20.);
    assert_eq!(resolved.matched_seconds, 60.);
}

#[test]
fn one_shared_song_on_an_album_proves_nothing() {
    let m = Matcher::new([on_album(
        "lib/a.flac",
        "Ending Theme",
        "Rin Aoba",
        "Night Ferry OST",
    )]);
    let h = history_on(&[("1", "Ending Theme", "青葉 凛", "Night Ferry OST", 40.)]);
    let (out, resolved) = m.resolve(&h);
    assert_eq!(secs_of(&out, "spotify:track:1"), 40.);
    assert_eq!(resolved.matched_seconds, 0.);
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
fn two_names_in_the_same_letters_never_join_by_album() {
    // A label's credit, a compilation, a featured singer: all in Latin letters, all left alone.
    let m = Matcher::new([
        on_album("lib/1.flac", "Glass Tower", "Studio Band", "Game Score"),
        on_album("lib/2.flac", "Old Pier", "Studio Band", "Game Score"),
        on_album("lib/3.flac", "Solo", "Studio Band", "Other Score"),
    ]);
    let h = history_on(&[
        ("1", "Glass Tower", "Big Label Music", "Game Score", 10.),
        ("2", "Old Pier", "Big Label Music", "Game Score", 20.),
        ("3", "Solo", "Big Label Music", "Label Hits", 30.),
    ]);
    let (out, resolved) = m.resolve(&h);
    assert_eq!(resolved.matched_seconds, 0.);
    assert_eq!(artist_secs(&out, "Big Label Music"), 60.);
}

#[test]
fn albums_many_artists_share_prove_nothing() {
    for album in [
        "Greatest Hits",
        "Soundtrack",
        "Unknown Album",
        "Best of North",
        "North",
    ] {
        let m = Matcher::new([
            on_album("lib/intro.flac", "Intro", "North", album),
            on_album("lib/outro.flac", "Outro", "North", album),
        ]);
        let h = history_on(&[
            ("1", "Intro", "南", album, 10.),
            ("2", "Outro", "南", album, 20.),
            ("3", "Solo", "南", "Elsewhere", 40.),
        ]);
        let (out, resolved) = m.resolve(&h);
        assert_eq!(resolved.matched_seconds, 0., "{album}");
        assert_eq!(artist_secs(&out, "南"), 70., "{album}");
    }
}

#[test]
fn an_album_two_library_artists_share_with_a_spelling_proves_neither() {
    let m = Matcher::new([
        on_album("k1", "One", "Rin Aoba", "Night Ferry OST"),
        on_album("k2", "Two", "Rin Aoba", "Night Ferry OST"),
        on_album("v1", "Four", "Mia Sato", "Night Ferry OST"),
        on_album("v2", "Five", "Mia Sato", "Night Ferry OST"),
    ]);
    let h = history_on(&[
        ("1", "One", "青葉 凛", "Night Ferry OST", 1.),
        ("2", "Two", "青葉 凛", "Night Ferry OST", 1.),
        ("4", "Four", "青葉 凛", "Night Ferry OST", 1.),
        ("5", "Five", "青葉 凛", "Night Ferry OST", 1.),
        ("7", "Not Here", "青葉 凛", "Elsewhere", 5.),
    ]);
    let (out, resolved) = m.resolve(&h);
    assert_eq!(resolved.matched_seconds, 0.);
    assert_eq!(artist_secs(&out, "青葉 凛"), 9.);
}

#[test]
fn a_title_two_artists_have_on_a_proven_album_goes_to_the_proven_one() {
    let orion = [
        on_album("orion/home.flac", "Home", "Orion", "Lantern Sessions"),
        on_album("orion/dawn.flac", "Dawn", "Orion", "Lantern Sessions"),
    ];
    let mio = [
        on_album("mio/home.flac", "Home", "Mio", "Lantern Sessions"),
        on_album("mio/dusk.flac", "Dusk", "Mio", "Lantern Sessions"),
    ];
    let h = history_on(&[
        ("1", "Home", "ミオ", "Lantern Sessions", 60.),
        ("2", "Dusk", "ミオ", "Lantern Sessions", 10.),
    ]);
    for library in [
        [orion.clone(), mio.clone()].concat(),
        [mio.clone(), orion.clone()].concat(),
    ] {
        let (out, _) = Matcher::new(library).resolve(&h);
        assert_eq!(secs_of(&out, "mio/home.flac"), 60.);
        assert_eq!(secs_of(&out, "orion/home.flac"), 0.);
    }
}

#[test]
fn a_credit_proven_to_be_several_artists_never_joins_them() {
    // A label credits two composers' music; the library credits each composer.
    let m = Matcher::new([
        on_album("mira/a.flac", "Pier", "Mira", "Harbor Lights"),
        on_album("mira/b.flac", "Quay", "Mira", "Harbor Lights"),
        on_album("sol/c.flac", "Jetty", "Sol", "Harbor Lights"),
        on_album("mira/finale.flac", "Finale", "Mira", "Mira at Sea"),
    ]);
    let h = history_on(&[
        ("1", "Pier", "ハーバー", "Harbor Lights", 10.),
        ("2", "Quay", "ハーバー", "Harbor Lights", 10.),
        ("3", "Jetty", "ハーバー", "Harbor Lights", 10.),
        ("4", "Finale", "ハーバー", "Another Release", 40.),
    ]);
    let (out, resolved) = m.resolve(&h);
    assert_eq!(secs_of(&out, "mira/a.flac"), 10.);
    assert_eq!(secs_of(&out, "mira/b.flac"), 10.);
    assert_eq!(secs_of(&out, "sol/c.flac"), 0.);
    assert_eq!(secs_of(&out, "mira/finale.flac"), 0.);
    assert_eq!(resolved.matched_seconds, 20.);
}

#[test]
fn plays_without_an_artist_name_never_join() {
    let m = Matcher::new([
        on_album("lib/1.flac", "Opening", "Rin Aoba", "Night Ferry OST"),
        on_album("lib/2.flac", "Harbor", "Rin Aoba", "Night Ferry OST"),
        on_album("lib/3.flac", "Home", "Rin Aoba", "Elsewhere"),
        song("lib/bang.flac", "Bang", "!!!").into(),
    ]);
    let h = history_on(&[
        ("1", "Opening", "", "Night Ferry OST", 10.),
        ("2", "Harbor", "", "Night Ferry OST", 10.),
        ("3", "Home", "", "Other", 30.),
        ("4", "Moon", "🌙", "Moon Songs", 70.),
    ]);
    let (out, resolved) = m.resolve(&h);
    assert_eq!(resolved.matched_seconds, 0.);
    assert_eq!(artist_secs(&out, "Rin Aoba"), 0.);
    assert_eq!(artist_secs(&out, "!!!"), 0.);
    assert_eq!(artist_secs(&out, "🌙"), 70.);
}

#[test]
fn a_spelling_proven_to_be_a_duet_stays_off_the_singers_solo_songs() {
    let m = Matcher::new([
        on_album("duo/x.flac", "Twin", "North & South", "Two Harbors"),
        on_album("duo/y.flac", "Tide", "North & South", "Two Harbors"),
        on_album("north/z.flac", "Alone", "North", "Solo Days"),
    ]);
    let h = history_on(&[
        ("1", "Twin", "ノース & サウス", "Two Harbors", 10.),
        ("2", "Tide", "ノース & サウス", "Two Harbors", 10.),
        ("3", "Alone", "ノース & サウス", "Other", 30.),
        ("4", "Unknown", "ノース & サウス", "Other", 5.),
    ]);
    let (out, resolved) = m.resolve(&h);
    assert_eq!(resolved.matched_seconds, 20.);
    assert_eq!(secs_of(&out, "north/z.flac"), 0.);
    assert_eq!(artist_secs(&out, "North"), 0.);
    assert_eq!(artist_secs(&out, "North & South"), 20. + 30. + 5.);
}

#[test]
fn plays_a_link_covers_never_prove_an_artist() {
    let m = Matcher::new([
        on_album("north/quay.flac", "Quay", "North", "Lantern Sessions"),
        on_album("north/jetty.flac", "Jetty", "North", "Lantern Sessions"),
        on_album("north/solo.flac", "Solo", "North", "North Alone"),
        song("south/quay.flac", "Quay (Live)", "South").into(),
        song("south/jetty.flac", "Jetty (Live)", "South").into(),
    ]);
    let h = linked(
        &[
            ("1", "Quay", "ミオ", "Lantern Sessions", 10.),
            ("2", "Jetty", "ミオ", "Lantern Sessions", 10.),
            ("3", "Solo", "ミオ", "Elsewhere", 40.),
        ],
        serde_json::json!([
            {"title": "Quay", "artist": "ミオ", "as": {"title": "Quay (Live)", "artist": "South"}},
            {"title": "Jetty", "artist": "ミオ", "as": {"title": "Jetty (Live)", "artist": "South"}},
        ]),
    );
    let (out, _) = m.resolve(&h);
    assert_eq!(secs_of(&out, "south/quay.flac"), 10.);
    assert_eq!(secs_of(&out, "south/jetty.flac"), 10.);
    assert_eq!(secs_of(&out, "north/solo.flac"), 0.);
    assert_eq!(artist_secs(&out, "ミオ"), 40.);
}

#[test]
fn links_name_the_whole_credit_on_both_ends() {
    let m = Matcher::new([
        song("north/hero.flac", "Hero", "North"),
        song("duo/hero.flac", "Hero", "North & South"),
        song("lib/theme.flac", "Theme", "Composer"),
    ]);
    let h = linked(
        &[
            ("1", "Hero", "North & South", "Duets", 10.),
            ("2", "Hero", "North", "Solo", 50.),
            ("3", "Hero (Cover)", "Fan", "Covers by Fan", 7.),
        ],
        serde_json::json!([
            {"title": "Hero", "artist": "North & South", "as": {"title": "Theme", "artist": "Composer"}},
            {"title": "Hero (Cover)", "artist": "Fan", "as": {"title": "Hero", "artist": "North & South"}},
        ]),
    );
    let (out, _) = m.resolve(&h);
    assert_eq!(secs_of(&out, "lib/theme.flac"), 10.);
    assert_eq!(secs_of(&out, "north/hero.flac"), 50.);
    assert_eq!(secs_of(&out, "duo/hero.flac"), 7.);
}

#[test]
fn the_whole_credit_and_album_choose_between_recordings() {
    let m = Matcher::new([
        on_album("north/home.flac", "Home", "North", "Studio Days"),
        on_album("north/home-live.flac", "Home", "North", "Live at the Dock"),
        on_album("duo/home.flac", "Home", "North & South", "Duets"),
    ]);
    let h = history_on(&[
        ("1", "Home", "North & South", "Duets", 60.),
        ("2", "Home", "North", "Live at the Dock", 30.),
        ("3", "Home", "North", "Studio Days", 10.),
    ]);
    let (out, _) = m.resolve(&h);
    assert_eq!(secs_of(&out, "duo/home.flac"), 60.);
    assert_eq!(secs_of(&out, "north/home-live.flac"), 30.);
    assert_eq!(secs_of(&out, "north/home.flac"), 10.);
}

#[test]
fn japanese_voicing_marks_tell_kana_apart() {
    let m = Matcher::new([song("lib/ka.flac", "カ", "Kana Band")]);
    // ガ written as カ plus a voicing mark: a different song, so never カ's plays.
    let h = history_on(&[("1", "カ\u{3099}", "Kana Band", "Kana", 60.)]);
    let (out, _) = m.resolve(&h);
    assert_eq!(secs_of(&out, "lib/ka.flac"), 0.);
}

#[test]
fn songs_on_different_albums_together_prove_a_name_in_another_script() {
    // One song per album, a single among them: proof adds up across albums.
    let library = |artist: &str| {
        [
            on_album("lib/rain.flac", "Rain", artist, "Anticyclone"),
            on_album("lib/spring.flac", "Spring", artist, "Weather Station"),
            on_album("lib/train.flac", "Train", artist, "Train"),
        ]
    };
    let plays = |artist: &str| {
        history_on(&[
            ("1", "Rain", artist, "Anticyclone", 30.),
            ("2", "Spring", artist, "Weather Station", 20.),
            ("3", "Train", artist, "Train", 10.),
            ("4", "Boots", artist, "Weather Station", 5.),
        ])
    };
    let (out, resolved) = Matcher::new(library("Rin Aoba")).resolve(&plays("青葉 凛"));
    assert_eq!(resolved.matched_seconds, 60.);
    assert_eq!(artist_secs(&out, "Rin Aoba"), 65.);
    // The same shape in the same letters is two artists who happen to share titles and album names.
    let (out, resolved) = Matcher::new(library("North")).resolve(&plays("South"));
    assert_eq!(resolved.matched_seconds, 0.);
    assert_eq!(artist_secs(&out, "South"), 65.);
}
