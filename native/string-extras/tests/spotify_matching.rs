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
        "Song (feat. X - Reprise)",
        "Song (feat. X - Karaoke)",
        "Song (feat. X – Dub)",
        "Song (feat. X—Piano Take)",
        "Song (feat. X/Unplugged)",
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

#[test]
fn a_collaboration_takes_its_own_credit_or_its_lead_alone_never_another_duet() {
    let m = Matcher::new([
        on_album("duo/home.flac", "Home", "North & South", "Duets"),
        on_album("north/home.flac", "Home", "North", "Solo"),
    ]);
    let (out, _) = m.resolve(&history_on(&[("1", "Home", "North & West", "Duets", 60.)]));
    assert_eq!(secs_of(&out, "duo/home.flac"), 0.);
    assert_eq!(secs_of(&out, "north/home.flac"), 60.);
}

#[test]
fn a_play_two_different_duets_could_be_needs_the_album_to_choose() {
    let m = Matcher::new([
        on_album("ab/finale.flac", "Finale", "Ada & Ben", "Ben Sessions"),
        on_album("ac/finale.flac", "Finale", "Ada & Cora", "Cora Sessions"),
    ]);
    let (out, resolved) = m.resolve(&history_on(&[("1", "Finale", "Ada", "Solo Sessions", 60.)]));
    assert_eq!(resolved.matched_seconds, 0.);
    assert_eq!(secs_of(&out, "spotify:track:1"), 60.);
    let (out, _) = m.resolve(&history_on(&[("1", "Finale", "Ada", "Cora Sessions", 60.)]));
    assert_eq!(secs_of(&out, "ac/finale.flac"), 60.);
}

#[test]
fn names_that_differ_only_by_a_separator_stay_apart() {
    let m = Matcher::new([
        song("rs/home.flac", "Home", "River Stone").into(),
        song("rivers/tide.flac", "Tide", "The Rivers"),
    ]);
    let h = linked(
        &[
            ("1", "Home", "River Stone", "Solo", 60.),
            ("2", "Tide", "River & Stone", "Duo", 10.),
            ("3", "Unknown", "River Stone", "Solo", 30.),
        ],
        serde_json::json!([artist_link("River & Stone", "The Rivers")]),
    );
    let (out, _) = m.resolve(&h);
    assert_eq!(secs_of(&out, "rs/home.flac"), 60.);
    assert_eq!(secs_of(&out, "rivers/tide.flac"), 10.);
    assert_eq!(artist_secs(&out, "River Stone"), 90.);
    assert_eq!(artist_secs(&out, "The Rivers"), 10.);
}

#[test]
fn a_song_link_names_the_whole_title_and_uses_the_plays_album() {
    let m = Matcher::new([
        song("mio/yui.flac", "Home (feat. Yui)", "Mio").into(),
        on_album("north/home.flac", "Home", "North", "Studio Days"),
        on_album("north/home-live.flac", "Home", "North", "Live"),
    ]);
    let h = linked(
        &[
            ("1", "Home (feat. Ena)", "Mio", "Duets", 10.),
            ("2", "Home (feat. Yui)", "Mio", "Duets", 20.),
        ],
        serde_json::json!([
            {"title": "Home (feat. Ena)", "artist": "Mio", "as": {"title": "Home", "artist": "North"}},
        ]),
    );
    let (out, _) = m.resolve(&h);
    assert_eq!(secs_of(&out, "mio/yui.flac"), 20.);
    assert_eq!(
        secs_of(&out, "north/home.flac") + secs_of(&out, "north/home-live.flac"),
        10.
    );
    let live = linked(
        &[("1", "Home (feat. Ena)", "Mio", "Live", 10.)],
        serde_json::json!([
            {"title": "Home (feat. Ena)", "artist": "Mio", "as": {"title": "Home", "artist": "North"}},
        ]),
    );
    let (out, _) = m.resolve(&live);
    assert_eq!(secs_of(&out, "north/home-live.flac"), 10.);
}

#[test]
fn albums_that_differ_only_by_a_separator_stay_apart() {
    let m = Matcher::new([
        on_album("a/plus.flac", "Overture", "North", "Act I + II"),
        on_album("a/slash.flac", "Overture", "North", "Act I / II"),
    ]);
    let (out, _) = m.resolve(&history_on(&[(
        "1",
        "Overture",
        "North",
        "Act I / II",
        60.,
    )]));
    assert_eq!(secs_of(&out, "a/slash.flac"), 60.);
}

#[test]
fn an_artist_link_to_a_name_the_library_lacks_leaves_normal_matching_alone() {
    let m = Matcher::new([song("ada/home.flac", "Home", "Ada")]);
    let h = linked(
        &[("1", "Home", "Ada", "Solo", 60.)],
        serde_json::json!([artist_link("Ada", "Missing Name")]),
    );
    let (out, _) = m.resolve(&h);
    assert_eq!(secs_of(&out, "ada/home.flac"), 60.);
}

#[test]
fn a_link_with_keys_it_doesnt_know_is_passed_over() {
    let m = Matcher::new([song("north/theme.flac", "Theme", "North")]);
    let h = linked(
        &[
            ("1", "Hero", "Fan", "Covers", 60.),
            ("2", "Ode", "Fan", "Covers", 30.),
        ],
        serde_json::json!([{"name": "Hero", "artist": "Fan", "as": {"name": "Theme", "artist": "North"}}]),
    );
    let (out, resolved) = m.resolve(&h);
    assert_eq!(resolved.matched_seconds, 0.);
    assert_eq!(artist_secs(&out, "Fan"), 90.);
}

#[test]
fn an_artist_link_keeps_plays_under_its_artist_when_the_library_lacks_the_song() {
    let m = Matcher::new([
        song("jp/quay.flac", "Quay", "Nova JP"),
        song("nova/home.flac", "Home", "Nova"),
    ]);
    let h = linked(
        &[("1", "Home", "Nova", "Singles", 60.)],
        serde_json::json!([artist_link("Nova", "Nova JP")]),
    );
    let (out, resolved) = m.resolve(&h);
    assert_eq!(resolved.matched_seconds, 0.);
    assert_eq!(secs_of(&out, "nova/home.flac"), 0.);
    assert_eq!(artist_secs(&out, "Nova JP"), 60.);
}

#[test]
fn marks_that_join_names_never_turn_into_words() {
    let m = Matcher::new([
        song("slash/home.flac", "Home", "Slash"),
        song("plus/ode.flac", "Ode", "North Plus"),
    ]);
    let h = linked(
        &[
            ("1", "Unknown", "/", "Marks", 10.),
            ("2", "Ode", "North+", "Marks", 20.),
            ("3", "Ode", "North Plus", "Words", 30.),
        ],
        serde_json::json!([artist_link("North+", "Elsewhere")]),
    );
    let (out, _) = m.resolve(&h);
    assert_eq!(artist_secs(&out, "Slash"), 0.);
    assert_eq!(artist_secs(&out, "/"), 10.);
    assert_eq!(secs_of(&out, "plus/ode.flac"), 30.);
}

#[test]
fn a_credit_written_without_spaces_is_the_same_credit() {
    let m = Matcher::new([
        on_album("rs/home.flac", "Home", "River Stone", "Solo"),
        on_album("duo/home.flac", "Home", "River & Stone", "Duets"),
        song("ab/home.flac", "Home", "A B").into(),
    ]);
    let h = history_on(&[
        ("1", "Home", "River&Stone", "Duets", 60.),
        ("2", "Home", "A/B", "Marks", 30.),
    ]);
    let (out, _) = m.resolve(&h);
    assert_eq!(secs_of(&out, "duo/home.flac"), 60.);
    assert_eq!(secs_of(&out, "rs/home.flac"), 0.);
    assert_eq!(secs_of(&out, "ab/home.flac"), 0.);
}

#[test]
fn the_written_title_tells_a_sharp_from_a_natural() {
    for flip in [false, true] {
        let mut library = vec![
            on_album("p/c.flac", "Prelude in C Minor", "Pianist", "Preludes"),
            on_album(
                "p/c-sharp.flac",
                "Prelude in C# Minor",
                "Pianist",
                "Preludes",
            ),
        ];
        if flip {
            library.reverse();
        }
        let m = Matcher::new(library);
        let h = linked(
            &[
                ("1", "Prelude in C# Minor", "Pianist", "Preludes", 60.),
                ("2", "Prelude (Cover)", "Fan", "Covers", 10.),
            ],
            serde_json::json!([
                {"title": "Prelude (Cover)", "artist": "Fan", "as": {"title": "Prelude in C# Minor", "artist": "Pianist"}},
            ]),
        );
        let (out, _) = m.resolve(&h);
        assert_eq!(secs_of(&out, "p/c-sharp.flac"), 70., "{flip}");
        assert_eq!(secs_of(&out, "p/c.flac"), 0., "{flip}");
    }
}

#[test]
fn the_written_title_picks_the_featured_recording() {
    let m = Matcher::new([
        on_album("mira/ben.flac", "Home (feat. Ben)", "Mira", "Duets"),
        on_album("mira/cora.flac", "Home (feat. Cora)", "Mira", "Duets"),
    ]);
    let h = linked(
        &[
            ("1", "Home (feat. Cora)", "Mira", "Duets", 60.),
            ("2", "Home (feat. Cora)", "ミラ", "Duets", 30.),
        ],
        serde_json::json!([artist_link("ミラ", "Mira")]),
    );
    let (out, _) = m.resolve(&h);
    assert_eq!(secs_of(&out, "mira/cora.flac"), 90.);
    assert_eq!(secs_of(&out, "mira/ben.flac"), 0.);
}

#[test]
fn a_band_with_a_mark_in_its_name_never_lends_its_plays_to_a_member() {
    let m = Matcher::new([
        song("gh/pier.flac", "Pier", "Glass/House"),
        song("glass/signal.flac", "Signal", "Glass"),
        song("nw/signal.flac", "Signal", "North/West"),
        song("north/home.flac", "Home", "North"),
    ]);
    let h = history_on(&[
        ("1", "Signal", "Glass/House", "Other", 60.),
        ("2", "Home", "North/West", "Other", 30.),
        ("3", "Home", "North+", "Other", 20.),
    ]);
    let (out, resolved) = m.resolve(&h);
    assert_eq!(resolved.matched_seconds, 0.);
    assert_eq!(secs_of(&out, "glass/signal.flac"), 0.);
    assert_eq!(secs_of(&out, "north/home.flac"), 0.);
}

#[test]
fn a_credit_beginning_with_a_mark_still_matches_itself() {
    let m = Matcher::new([
        song("lights/home.flac", "Home", "&Lights"),
        song("line/home.flac", "Home", "//Line"),
    ]);
    let h = history_on(&[
        ("1", "Home", "&Lights", "Album", 60.),
        ("2", "Home", "//Line", "Album", 30.),
    ]);
    let (out, _) = m.resolve(&h);
    assert_eq!(secs_of(&out, "lights/home.flac"), 60.);
    assert_eq!(secs_of(&out, "line/home.flac"), 30.);
}

#[test]
fn a_hyphenated_band_is_not_a_collaboration_with_the_same_words() {
    for flip in [false, true] {
        let mut library = vec![
            on_album("band/signal.flac", "Signal", "North-X-West", "Stage"),
            on_album("duo/signal.flac", "Signal", "North x West", "Stage"),
        ];
        if flip {
            library.reverse();
        }
        let m = Matcher::new(library);
        let h = history_on(&[
            ("1", "Signal", "North-X-West", "Stage", 60.),
            ("2", "Signal", "North x West", "Stage", 30.),
        ]);
        let (out, _) = m.resolve(&h);
        assert_eq!(secs_of(&out, "band/signal.flac"), 60., "{flip}");
        assert_eq!(secs_of(&out, "duo/signal.flac"), 30., "{flip}");
    }
}

#[test]
fn the_album_outranks_a_title_that_differs_by_a_note() {
    let m = Matcher::new([
        on_album(
            "live/home.flac",
            "Home - 2011 Remaster",
            "North",
            "Live at the Pier",
        ),
        on_album("studio/home.flac", "Home", "North", "Studio Days"),
        on_album(
            "mio/concert.flac",
            "Home (feat. Ena)",
            "Mio",
            "Live at Harbor",
        ),
        on_album("mio/studio.flac", "Home", "Mio", "Studio Days"),
    ]);
    let h = history_on(&[
        ("1", "Home", "North", "Live at the Pier", 60.),
        ("2", "Home", "Mio", "Live at Harbor", 30.),
    ]);
    let (out, _) = m.resolve(&h);
    assert_eq!(secs_of(&out, "live/home.flac"), 60.);
    assert_eq!(secs_of(&out, "mio/concert.flac"), 30.);
}

#[test]
fn a_remaster_note_hides_neither_a_sharp_nor_a_guest() {
    let m = Matcher::new([
        on_album("p/c.flac", "Etude in C", "Pianist", "Etudes"),
        on_album("p/c-sharp.flac", "Etude in C#", "Pianist", "Etudes"),
        on_album("mira/ben.flac", "Home (feat. Ben)", "Mira", "Duets"),
        on_album("mira/cora.flac", "Home (feat. Cora)", "Mira", "Duets"),
    ]);
    let h = history_on(&[
        ("1", "Etude in C# - 2011 Remaster", "Pianist", "Etudes", 60.),
        (
            "2",
            "Home (feat. Cora) - 2011 Remaster",
            "Mira",
            "Duets",
            30.,
        ),
        ("3", "Etude in C\u{266f}", "Pianist", "Etudes", 5.),
    ]);
    let (out, _) = m.resolve(&h);
    assert_eq!(secs_of(&out, "p/c-sharp.flac"), 65.);
    assert_eq!(secs_of(&out, "mira/cora.flac"), 30.);
}

#[test]
fn a_collaboration_the_album_chose_still_picks_the_named_guest() {
    let m = Matcher::new([
        on_album("rin/ena.flac", "Home (feat. Ena)", "Mio & Rin", "Duets"),
        on_album("rin/yui.flac", "Home (feat. Yui)", "Mio & Rin", "Duets"),
        on_album("sol/home.flac", "Home", "Mio & Sol", "Other Duets"),
    ]);
    let (out, _) = m.resolve(&history_on(&[(
        "1",
        "Home (feat. Yui)",
        "Mio",
        "Duets",
        60.,
    )]));
    assert_eq!(secs_of(&out, "rin/yui.flac"), 60.);
}

#[test]
fn a_play_naming_another_guest_never_takes_a_guests_version() {
    let m = Matcher::new([on_album("mio/ena.flac", "Home (feat. Ena)", "Mio", "Duets")]);
    let h = linked(
        &[
            ("1", "Home (feat. Yui)", "Mio", "Duets", 60.),
            ("2", "Home (feat. Yui)", "ミオ", "Duets", 30.),
            ("3", "Home", "Mio", "Duets", 10.),
        ],
        serde_json::json!([artist_link("ミオ", "Mio")]),
    );
    let (out, _) = m.resolve(&h);
    assert_eq!(secs_of(&out, "mio/ena.flac"), 10.);
}

#[test]
fn a_guest_whose_name_is_a_common_word_still_folds_away() {
    let m = Matcher::new([song("lib/home.flac", "Home", "North")]);
    let (out, _) = m.resolve(&history_on(&[(
        "1",
        "Home (feat. Take Two)",
        "North",
        "Album",
        60.,
    )]));
    assert_eq!(secs_of(&out, "lib/home.flac"), 60.);
}

#[test]
fn leaving_out_another_guests_copy_never_leaves_a_stranger_as_the_match() {
    for flip in [false, true] {
        let mut library = vec![
            song("ben/home.flac", "Home (feat. Ben)", "Mira & Ben"),
            song("sol/home.flac", "Home", "Mira & Sol"),
        ];
        if flip {
            library.reverse();
        }
        let m = Matcher::new(library);
        let (out, resolved) = m.resolve(&history_on(&[(
            "1",
            "Home (feat. Cora)",
            "Mira",
            "Singles",
            60.,
        )]));
        assert_eq!(resolved.matched_seconds, 0., "{flip}");
        assert_eq!(secs_of(&out, "spotify:track:1"), 60., "{flip}");
    }
}

#[test]
fn the_same_guests_in_another_order_are_the_same_guests() {
    let m = Matcher::new([
        on_album(
            "rin/home.flac",
            "Home (feat. Ena & Yui)",
            "Mio & Rin",
            "Duets",
        ),
        on_album("sol/home.flac", "Home", "Mio & Sol", "Other Duets"),
    ]);
    let (out, _) = m.resolve(&history_on(&[(
        "1",
        "Home (feat. Yui & Ena)",
        "Mio",
        "Duets",
        60.,
    )]));
    assert_eq!(secs_of(&out, "rin/home.flac"), 60.);
}

#[test]
fn one_artist_alone_never_takes_a_song_by_a_name_with_a_mark_in_it() {
    let m = Matcher::new([
        song("nw/other.flac", "Other", "North West"),
        song("slash/signal.flac", "Signal", "North/West"),
        song("gh/home.flac", "Home", "Glass/House"),
        song("glass-house/pier.flac", "Pier", "Glass House"),
        song("plus/ode.flac", "Ode", "North+"),
        song("nova/quay.flac", "Quay", "&Nova"),
    ]);
    let h = history_on(&[
        ("1", "Signal", "North West", "Album", 60.),
        ("2", "Signal", "North&West", "Album", 10.),
        ("3", "Home", "Glass House", "Album", 30.),
        ("4", "Ode", "North", "Album", 20.),
        ("5", "Quay", "Nova", "Album", 5.),
    ]);
    let (_, resolved) = m.resolve(&h);
    assert_eq!(resolved.matched_seconds, 0.);
}

#[test]
fn a_named_remaster_year_picks_that_remaster() {
    let m = Matcher::new([
        on_album("r/2009.flac", "Home - 2009 Remaster", "North", "Home Again"),
        on_album("r/2018.flac", "Home - 2018 Remaster", "North", "Home Again"),
    ]);
    let h = linked(
        &[
            ("1", "Home - 2018 Remaster", "North", "Home Again", 60.),
            ("2", "Home - 2018 Remaster", "ノース", "Home Again", 30.),
        ],
        serde_json::json!([artist_link("ノース", "North")]),
    );
    let (out, _) = m.resolve(&h);
    assert_eq!(secs_of(&out, "r/2018.flac"), 90.);
}

#[test]
fn a_named_guest_outweighs_the_album() {
    let m = Matcher::new([
        on_album("mira/solo.flac", "Home", "Mira", "Best Mixes 2024"),
        on_album("mira/cora.flac", "Home (feat. Cora)", "Mira", "Duets"),
    ]);
    let h = linked(
        &[
            ("1", "Home (feat. Cora)", "Mira", "Best Mixes 2024", 60.),
            ("2", "Home (feat. Cora)", "ミラ", "Best Mixes 2024", 30.),
        ],
        serde_json::json!([artist_link("ミラ", "Mira")]),
    );
    let (out, _) = m.resolve(&h);
    assert_eq!(secs_of(&out, "mira/cora.flac"), 90.);
}

#[test]
fn a_long_dash_in_an_album_is_a_hyphen() {
    let m = Matcher::new([
        on_album("live/south.flac", "Home", "Mio", "Live - South"),
        on_album("live/north.flac", "Home", "Mio", "Live - North"),
    ]);
    let h = linked(
        &[
            ("1", "Home", "Mio", "Live \u{2013} North", 60.),
            ("3", "Home", "Mio", "Live \u{2011} North", 15.),
            ("4", "Home", "Mio", "Live \u{2212} North", 5.),
            ("2", "Home (Encore)", "Fan", "Live \u{2014} North", 30.),
        ],
        serde_json::json!([
            {"title": "Home (Encore)", "artist": "Fan", "as": {"title": "Home", "artist": "Mio"}},
        ]),
    );
    let (out, _) = m.resolve(&h);
    assert_eq!(secs_of(&out, "live/north.flac"), 110.);
}

#[test]
fn a_take_named_after_a_guest_is_another_recording() {
    let m = Matcher::new([song("lib/home.flac", "Home", "North")]);
    let h = history_on(&[
        ("1", "Home (feat. Ben, Take 2)", "North", "Album", 60.),
        (
            "2",
            "Home (feat. Ben, alternate take)",
            "North",
            "Album",
            30.,
        ),
    ]);
    let (out, resolved) = m.resolve(&h);
    assert_eq!(resolved.matched_seconds, 0.);
    assert_eq!(secs_of(&out, "lib/home.flac"), 0.);
}
