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
        "Song (feat. X – Live)",
        "Song (with X, Remix)",
        "Song (feat. X / Acoustic)",
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

fn artist_link(from: &str, to: &str) -> serde_json::Value {
    serde_json::json!({"artist": from, "as": {"artist": to}})
}

#[test]
fn a_name_in_another_script_joins_only_through_an_artist_link() {
    let m = Matcher::new([
        on_album("lib/1.flac", "Opening", "Rin Aoba", "Night Ferry OST"),
        on_album("lib/2.flac", "Harbor", "Rin Aoba", "Night Ferry OST"),
        on_album("lib/3.flac", "Lantern", "Rin Aoba", "Lantern"),
    ]);
    let plays = [
        ("1", "Opening", "青葉 凛", "Night Ferry OST", 10.),
        ("2", "Harbor", "青葉 凛", "Night Ferry OST", 20.),
        ("3", "Lantern", "青葉 凛", "Lantern", 30.),
        ("4", "Tide", "青葉 凛", "Tide", 40.),
    ];
    // Same titles on the same album prove nothing by themselves: a label's credit looks just the same.
    let (out, resolved) = m.resolve(&history_on(&plays));
    assert_eq!(resolved.matched_seconds, 0.);
    assert_eq!(artist_secs(&out, "青葉 凛"), 100.);
    let linked_history = linked(
        &plays,
        serde_json::json!([artist_link("青葉 凛", "Rin Aoba")]),
    );
    let (out, resolved) = m.resolve(&linked_history);
    assert_eq!(resolved.matched_seconds, 60.);
    assert_eq!(secs_of(&out, "lib/3.flac"), 30.);
    // Not in the library: counted under the library's spelling.
    assert_eq!(secs_of(&out, "spotify:track:4"), 40.);
    assert_eq!(artist_secs(&out, "Rin Aoba"), 100.);
    assert_eq!(artist_secs(&out, "青葉 凛"), 0.);
}

#[test]
fn shared_titles_and_albums_never_join_two_artists_by_themselves() {
    // A label credited with a composer's soundtrack, and two artists who share titles on look-alike albums.
    let m = Matcher::new([
        on_album("north/quay.flac", "Quay", "North", "Harbor Lights"),
        on_album("north/jetty.flac", "Jetty", "North", "Harbor Lights"),
        on_album("north/intro.flac", "Intro", "North", "Best Of"),
        on_album("north/outro.flac", "Outro", "North", "Best Of"),
        on_album("north/finale.flac", "Finale", "North", "North at Sea"),
    ]);
    let h = history_on(&[
        ("1", "Quay", "灯台音楽", "Harbor Lights", 10.),
        ("2", "Jetty", "灯台音楽", "Harbor Lights", 10.),
        ("3", "Finale", "灯台音楽", "Another Score", 60.),
        ("4", "Intro", "South", "Best Of", 10.),
        ("5", "Outro", "South", "Best Of", 10.),
    ]);
    let (out, resolved) = m.resolve(&h);
    assert_eq!(resolved.matched_seconds, 0.);
    assert_eq!(artist_secs(&out, "North"), 0.);
}

#[test]
fn an_artist_link_names_the_whole_credit_on_both_ends() {
    let m = Matcher::new([
        song("duo/home.flac", "Home", "Mio & Ena"),
        song("mio/alone.flac", "Alone", "Mio"),
        song("mio/home.flac", "Home", "Mio"),
    ]);
    let h = linked(
        &[
            ("1", "Home", "ミオ & エナ", "Duets", 10.),
            ("2", "Alone", "ミオ & エナ", "Duets", 20.),
            ("3", "Home", "ミオ", "Solo", 30.),
            ("4", "Ode", "ミオ & ユイ", "Other Duets", 40.),
        ],
        serde_json::json!([artist_link("ミオ & エナ", "Mio & Ena")]),
    );
    let (out, resolved) = m.resolve(&h);
    assert_eq!(secs_of(&out, "duo/home.flac"), 10.);
    assert_eq!(secs_of(&out, "mio/alone.flac"), 0.);
    assert_eq!(secs_of(&out, "mio/home.flac"), 0.);
    assert_eq!(resolved.matched_seconds, 10.);
    assert_eq!(artist_secs(&out, "Mio & Ena"), 30.);
    assert_eq!(artist_secs(&out, "ミオ"), 30.);
    assert_eq!(artist_secs(&out, "ミオ & ユイ"), 40.);
}

#[test]
fn a_link_to_a_credit_the_library_lacks_changes_nothing() {
    let m = Matcher::new([song("duo/theme.flac", "Theme", "North & South")]);
    let h = linked(
        &[
            ("1", "Hero", "Fan", "Covers", 10.),
            ("2", "Theme", "ノース", "Scores", 20.),
        ],
        serde_json::json!([
            {"title": "Hero", "artist": "Fan", "as": {"title": "Theme", "artist": "North"}},
            artist_link("ノース", "North"),
        ]),
    );
    let (out, resolved) = m.resolve(&h);
    assert_eq!(resolved.matched_seconds, 0.);
    assert_eq!(secs_of(&out, "duo/theme.flac"), 0.);
}

#[test]
fn a_collaboration_never_lands_on_another_collaborations_song() {
    let duet = Matcher::new([song("duo/home.flac", "Home", "North & South")]);
    let solo = Matcher::new([song("north/home.flac", "Home", "North")]);
    let h = |credit: &str| history_on(&[("1", "Home", credit, "Album", 10.)]);
    assert_eq!(duet.resolve(&h("North & West")).1.matched_seconds, 0.);
    assert_eq!(duet.resolve(&h("North & South")).1.matched_seconds, 10.);
    assert_eq!(duet.resolve(&h("North")).1.matched_seconds, 10.);
    assert_eq!(solo.resolve(&h("North, West")).1.matched_seconds, 10.);
}

#[test]
fn plays_without_an_artist_name_never_join() {
    let m = Matcher::new([
        song("lib/home.flac", "Home", "Rin Aoba"),
        song("lib/bang.flac", "Bang", "!!!"),
    ]);
    let h = history_on(&[
        ("1", "Home", "", "Other", 30.),
        ("2", "Moon", "🌙", "Moon Songs", 70.),
    ]);
    let (out, resolved) = m.resolve(&h);
    assert_eq!(resolved.matched_seconds, 0.);
    assert_eq!(artist_secs(&out, "!!!"), 0.);
    assert_eq!(artist_secs(&out, "🌙"), 70.);
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
        on_album("north/home-live.flac", "Home", "North", "Live"),
        on_album("duo/home.flac", "Home", "North & South", "Duets"),
    ]);
    let h = history_on(&[
        ("1", "Home", "North & South", "Duets", 60.),
        ("2", "Home", "North", "Live", 30.),
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
