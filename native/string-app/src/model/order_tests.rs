use super::*;
use serde_json::json;
use string_core::queue::{PlaybackOptions, Queue};

fn track(file: &str, artist: &str, album: &str, year: u32, added: i64) -> Track {
    let mut t = Track {
        path: format!(r"F:\{file}.mp3"),
        added: Some(added),
        ..Default::default()
    };
    t.tags.artist = artist.into();
    t.tags.album = album.into();
    t.tags.year = year;
    t
}

fn model(tracks: Vec<Track>, state: &State) -> Models {
    let mut lib = Library::default();
    lib.tracks = tracks;
    Models::build(&lib, state)
}

fn titles<'a>(m: &'a Models, ids: &[String]) -> Vec<&'a str> {
    ids.iter().map(|id| m.by_id[id].title.as_str()).collect()
}

fn continuation<'a>(m: &'a Models, ctx: Value, last: &str) -> &'a str {
    let mut q = Queue::default();
    assert!(q.play_ids(&[last.to_owned()], 0, ctx, &m.catalog));
    let plan = q.what_next(false, PlaybackOptions::default(), &m.catalog);
    assert!(plan.starts_list());
    &m.by_id[plan.id().unwrap()].title
}

#[test]
fn album_plays_filename_numbers_and_prefers_track_tags() {
    let mut tagged = track("01 - Fourth", "", "Record", 0, 0);
    tagged.tags.track = 4;
    let mut tracks = vec![tagged];
    for file in ["02 - Apple", "03 - Mango", "01 - Zebra"] {
        tracks.push(track(file, "", "Record", 0, 0));
    }
    let m = model(tracks, &State::default());
    assert_eq!(
        titles(&m, &m.catalog.albums[0].ids),
        ["Zebra", "Apple", "Mango", "Fourth"]
    );
    for (name, expected) in [
        (" 098)Song", 98),
        ("3 Song", 3),
        ("01_Song", 1),
        ("12\t-Name", 12),
        ("12\tName", 0),
        ("0001 - Name", 0),
        ("20th Century", 0),
    ] {
        assert_eq!(
            make(&track(name, "", "", 0, 0), &State::default(), 0).number,
            expected,
            "{name}"
        );
    }
}

#[test]
fn id_only_filename_keeps_full_name() {
    assert_eq!(filename_title("[Ab12Cd34Ef5].mp3"), "[Ab12Cd34Ef5].mp3");
    assert_eq!(filename_title("[123456].mp3"), "[123456].mp3");
}

#[test]
fn root_album_shortcut_resolves() {
    let t = track("Song", "", "Record", 0, 0);
    let id = t.id();
    let m = model(vec![t], &State::default());
    let shortcut = json!({"kind":"album","key":"record\u{1}f:"});
    assert_eq!(m.by_id[&id].album_key, shortcut["key"]);
    assert_eq!(m.context_ids(&shortcut).as_ref(), [id]);
}

#[test]
fn earlier_root_shortcut_still_opens_and_continues() {
    let first = track("First", "A", "Middle", 1, 0);
    let mut m = model(
        vec![
            first.clone(),
            track("Next", "A", "Zulu", 2, 0),
            track("Other", "B", "Alpha", 1, 0),
        ],
        &State::default(),
    );
    let ctx = json!({"kind":"album","key":"middle\u{1}f:\\"});
    assert_eq!(m.context_ids(&ctx).as_ref(), [first.id()]);
    assert_eq!(continuation(&m, ctx.clone(), &first.id()), "Next");
    let mut exact = track("Credit", "B", "Middle", 0, 0);
    exact.tags.album_artist = r"F:\".into();
    m = model(vec![first, exact.clone()], &State::default());
    assert_eq!(m.context_ids(&ctx).as_ref(), [exact.id()]);
}

#[test]
fn album_continuation_uses_each_page_sort() {
    let tracks = [
        track("First", "A", "Alpha", 2001, 3),
        track("Second", "A", "Zulu", 2002, 1),
        track("Third", "B", "Middle", 2003, 2),
    ];
    let first = tracks[0].id();
    let mut state = State::default();
    let mut m = model(tracks.to_vec(), &state);
    for (mode, order, after_first) in [
        ("artist", ["Alpha", "Zulu", "Middle"], "Second"),
        ("name", ["Alpha", "Middle", "Zulu"], "Third"),
        ("year", ["Middle", "Zulu", "Alpha"], "Third"),
        ("recent", ["Alpha", "Middle", "Zulu"], "Third"),
    ] {
        state.fields.insert("albumSort".into(), json!(mode));
        m.sort_albums(&state);
        let ctx = json!({"kind":"album","key":m.by_id[&first].album_key});
        assert_eq!(continuation(&m, ctx, &first), after_first, "{mode}");
        assert_eq!(
            m.catalog
                .albums
                .iter()
                .map(|g| g.name.as_str())
                .collect::<Vec<_>>(),
            order,
            "{mode}"
        );
    }
}

#[test]
fn artist_continuation_puts_unknown_last() {
    let tracks = [
        track("One", "Tango", "", 0, 0),
        track("Two", "", "", 0, 0),
        track("Three", "Zulu", "", 0, 0),
    ];
    let m = model(tracks.to_vec(), &State::default());
    assert_eq!(
        continuation(&m, json!({"kind":"artist","key":"tango"}), &tracks[0].id()),
        "Three"
    );
    assert_eq!(
        continuation(&m, json!({"kind":"artist","key":"zulu"}), &tracks[2].id()),
        "Two"
    );
}

#[test]
fn album_sort_uses_first_credited_artist_and_library_order_for_recent_ties() {
    let mut tracks = vec![
        track("Z", "Zed", "Mix", 2000, 1),
        track("A", "Ace", "Mix", 2000, 1),
        track("B", "Bee", "Solo", 2001, 1),
    ];
    tracks[2].tags.album_artist = "Middle".into();
    let mut state = State::default();
    let m = model(tracks.clone(), &state);
    assert_eq!(m.catalog.albums[0].name, "Solo");
    state.fields.insert("albumSort".into(), json!("recent"));
    assert_eq!(model(tracks, &state).catalog.albums[0].name, "Mix");
}

#[test]
fn artist_tracks_compare_track_before_title_and_ignore_disc() {
    let mut a = track("One", "Solo", "Record", 0, 0);
    a.tags.track = 1;
    a.tags.disc = 2;
    let mut b = track("Two", "Solo", "Record", 0, 0);
    b.tags.track = 2;
    b.tags.disc = 1;
    let m = model(vec![a, b], &State::default());
    assert_eq!(titles(&m, &m.catalog.artists[0].ids), ["One", "Two"]);
    assert_eq!(titles(&m, &m.catalog.albums[0].ids), ["Two", "One"]);
}

#[test]
fn songs_continuation_uses_page_ties_and_direction() {
    let mut a = track("Apple", "Solo", "Side", 0, 1);
    a.tags.disc = 2;
    a.tags.track = 1;
    let mut b = track("Banana", "Solo", "Side", 0, 2);
    b.tags.disc = 1;
    b.tags.track = 2;
    let c = track("Cherry", "Solo", "", 0, 3);
    let mut state = State::default();
    let mut m = model(vec![b, c.clone(), a], &state);
    for (key, dir, order) in [
        ("artist", 1, ["Cherry", "Apple", "Banana"]),
        ("album", 1, ["Banana", "Apple", "Cherry"]),
        ("title", -1, ["Cherry", "Banana", "Apple"]),
        ("added", 1, ["Cherry", "Banana", "Apple"]),
        ("duration", 1, ["Apple", "Banana", "Cherry"]),
    ] {
        state
            .fields
            .insert("sort".into(), json!({"key":key,"dir":dir}));
        m.sort_songs(&state);
        assert_eq!(titles(&m, &m.songs), order, "{key}");
        let expected = if order[0] == "Cherry" {
            order[1]
        } else {
            order[0]
        };
        assert_eq!(continuation(&m, json!({"kind":"songs"}), &c.id()), expected);
    }
}

#[test]
fn titles_and_search_use_numeric_base_collation() {
    let tracks = ["Take 10", "Écho", "echo", "Take 2", "1979", "99 Problems"]
        .map(|file| track(file, "", "", 0, 0));
    let m = model(tracks.to_vec(), &State::default());
    assert_eq!(
        titles(&m, &m.songs),
        ["99 Problems", "1979", "Écho", "echo", "Take 2", "Take 10"]
    );
    assert_eq!(titles(&m, &m.search("take")), ["Take 2", "Take 10"]);
}

#[test]
fn titles_follow_the_local_intl_alphabet() {
    let names = [
        "Zebra", "Ørb", "Osa", "Łane", "Lion", "Alfa", "Æther", "...And", "(Intro)",
    ];
    let script = "const a=JSON.parse(process.argv[1]);console.log(JSON.stringify(a.sort(new Intl.Collator(undefined,{numeric:true,sensitivity:'base'}).compare)))";
    let reference = std::process::Command::new("node")
        .args(["-e", script, &json!(names).to_string()])
        .output()
        .unwrap();
    assert!(reference.status.success());
    let expected: Vec<String> = serde_json::from_slice(&reference.stdout).unwrap();
    let m = model(
        names.map(|s| track(s, "", "", 0, 0)).to_vec(),
        &State::default(),
    );
    assert_eq!(titles(&m, &m.songs), expected);
}

#[test]
#[ignore = "release timing on generated song metadata"]
fn measure_twelve_thousand_songs() {
    use std::time::Instant;
    let mut state = State::default();
    let tracks = (0..12_000)
        .map(|i| {
            let n = i * 7919 % 12_000;
            let mut t = track(
                &format!("Táké {n}"),
                &format!("Artist {}", n % 80),
                &format!("Album {}", n % 600),
                2000 + n % 25,
                n as i64,
            );
            t.tags.duration = 60. + f64::from(n * 37 % 7200) / 10.;
            t
        })
        .collect();
    let start = Instant::now();
    let mut m = model(tracks, &state);
    println!("12,000-song model build: {:?}", start.elapsed());
    assert_eq!(m.songs.len(), 12_000);
    for key in ["artist", "album", "title", "duration", "added"] {
        state
            .fields
            .insert("sort".into(), json!({"key":key,"dir":1}));
        let start = Instant::now();
        m.sort_songs(&state);
        println!("12,000-song {key} sort: {:?}", start.elapsed());
    }
}
