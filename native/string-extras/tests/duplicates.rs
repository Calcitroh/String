use serde_json::{Value, json};
use string_core::{
    library::{Library, Track},
    state::State,
    stats::{Listening, Track as Heard},
    tags::Tags,
};
use string_extras::duplicates::*;

fn song(id: &str, title: &str, artist: &str, duration: f64, format: &str, size: u64) -> Track {
    Track {
        path: id.into(),
        size,
        tags: Tags {
            title: title.into(),
            artist: artist.into(),
            duration,
            format: format.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}
fn library(tracks: Vec<Track>) -> Library {
    let mut l = Library::default();
    l.tracks = tracks;
    l
}

#[test]
fn neither_an_unknown_nor_chained_length_certifies_two_recordings() {
    for lengths in [[60., 0., 360.], [60., 62., 64.]] {
        let lib = library(
            lengths
                .into_iter()
                .enumerate()
                .map(|(i, d)| song(&i.to_string(), "Overture", "Band", d, "", 0))
                .collect(),
        );
        let g = &duplicates(&lib, &State::default())[0];
        let set = |id| g.sets[g.ids.iter().position(|s| s == id).unwrap()];
        assert!(!g.sure);
        assert_ne!(set("0"), set("2"));
    }
}
#[test]
fn quality_bitrate_sets_and_lengths() {
    let lib = library(vec![
        song("1", "Battle Theme", "Band", 180., "mp3", 4_000_000),
        song("2", "Battle Theme", "Band", 181., "flac", 30_000_000),
        song("3", "battle theme!", "band", 180.5, "mp3", 5_000_000),
        song("4", "Battle Theme", "Band", 400., "mp3", 9_000_000),
        song("5", "Battle Theme", "Other", 180., "mp3", 4_000_000),
        song("6", "Lonely", "X", 100., "mp3", 1),
    ]);
    let groups = duplicates(&lib, &State::default());
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].ids, ["2", "3", "1", "5"]);
    assert_eq!(groups[0].sets, [0, 0, 0, 1]);
    assert!(!groups[0].sure);
}
#[test]
fn decorated_names_sorting_and_apart_pairs() {
    let rows = [
        ("a1", "Glass Orbit", "Band", 200.),
        ("a2", "Glass Orbit (Official Video)", "Band", 201.),
        ("b1", "Paper Lantern (From \"Some Show\")", "Singer", 165.),
        ("b2", "Paper Lantern", "Other", 165.),
        ("c1", "Static Bloom", "Band", 269.),
        ("c2", "Static Bloom - Long Ver.", "Band", 269.),
        ("d1", "Intro", "A", 60.),
        ("d2", "Intro", "B", 95.),
        ("e1", "Song feat. Someone", "Band", 180.),
        ("e2", "Song", "Band", 180.),
    ];
    let lib = library(
        rows.into_iter()
            .map(|(id, t, a, d)| song(id, t, a, d, "m4a", 3_000_000))
            .collect(),
    );
    let mut s = State::default();
    s.fields["liked"]["a2"] = 1.into();
    let gs = duplicates(&lib, &s);
    assert_eq!(gs.len(), 4);
    assert!(gs[0].sure);
    assert_eq!(gs[0].ids, ["a2", "a1"]);
    for id in ["b1", "c1", "e1"] {
        let g = gs.iter().find(|g| g.ids.contains(&id.to_owned())).unwrap();
        assert_eq!(g.ids.len(), 2);
        assert!(!g.sure);
    }
    mark_not_same(
        &mut s,
        gs.iter().find(|g| g.ids.contains(&"b1".into())).unwrap(),
        123,
    );
    assert_eq!(s.fields["notDupes"]["b1\u{1}b2"], 123);
    let gs = duplicates(&lib, &s);
    assert_eq!(gs.len(), 3);
    assert!(gs.iter().all(|g| !g.ids.contains(&"b1".into())));
    forget_not_same(&mut s);
    assert_eq!(duplicates(&lib, &s).len(), 4);
}
#[test]
fn largest_sure_set_first_unicode_and_skip_common_titles() {
    let lib = library(vec![
        song(
            "a",
            "Static Bloom - Long Ver.",
            "Band",
            269.,
            "m4a",
            4_000_000,
        ),
        song("b", "Ｓtátic Bloom", "Band", 269., "m4a", 4_000_000),
        song("c", "Static Bloom【MV】", "Band", 269., "m4a", 4_000_000),
    ]);
    let g = &duplicates(&lib, &State::default())[0];
    assert_eq!(g.ids[0], "b");
    assert_eq!(g.suggested_drops(0).collect::<Vec<_>>(), ["c"]);
    let lib = library(
        (0..201)
            .map(|i| song(&i.to_string(), "Intro", "A", 60., "mp3", 1))
            .collect(),
    );
    assert!(duplicates(&lib, &State::default()).is_empty());
}
#[test]
fn preference_uses_visible_folders_likes_playlists_and_plays() {
    let lib = library(vec![
        song("x/a", "Song", "A", 10., "mp3", 100),
        song("y/b", "Song", "A", 10., "mp3", 101),
        song("x/c", "Solo", "B", 20., "mp3", 300),
        song("x/d", "Single", "C", 30., "mp3", 500),
    ]);
    let mut s = State::default();
    assert_eq!(duplicates(&lib, &s)[0].ids[0], "x/a");
    s.fields["liked"]["y/b"] = 1.into();
    s.fields["playlists"] = json!([{"items":[{"id":"y/b"}]}]);
    s.fields["plays"]["y/b"] = json!([1, 55]);
    assert_eq!(duplicates(&lib, &s)[0].ids[0], "y/b");
    s.fields["hidden"]["y/b"] = 77.into();
    assert!(duplicates(&lib, &s).is_empty());
    unhide_all(&mut s);
    assert_eq!(duplicates(&lib, &s)[0].ids[0], "y/b");
}
#[test]
fn confirmed_actions_merge_only_completed_copies_and_preserve_the_current_place() {
    for mode in [Disposition::Hide, Disposition::Recycled] {
        let mut s:State=serde_json::from_value(json!({"v":1,"liked":{"d":8,"k":7},"plays":{"d":[4,190],"k":[2,100]},
            "edits":{"d":{"title":"old"}},"notDupes":{"other":12},"loops":{"d":{"a":2,"b":8}},
            "playlists":[{"items":[{"id":"d","note":8},{"id":"x"},{"id":"k"}]},{"items":[{"id":"k"},{"id":"k"}]}],
            "queue":{"items":["d","d","x"],"orig":["x","d"],"at":0,"pos":13,"man":[1]}})).unwrap();
        let copy =
            CopyToMerge::new(&song("K", "", "", 0., "", 0), &song("D", "", "", 0., "", 0)).unwrap();
        assert_eq!(copy.path, "D");
        let when = chrono::DateTime::from_timestamp(0, 0).unwrap().naive_utc();
        let mut m = Listening::default();
        for (id, n) in [("k", 5.), ("d", 11.)] {
            m.add_phone(&Heard(id.into(), id.into(), "A".into()), n, when);
        }
        let switch = merge_copy(&mut s, &mut m, &copy, mode, 77);
        assert_eq!(switch, (mode == Disposition::Recycled).then_some("k"));
        assert_eq!(s.fields["liked"], json!({"k":7}));
        assert_eq!(s.fields["plays"], json!({"k":[6,190]}));
        assert_eq!(s.fields["loops"], json!({"k":{"a":2,"b":8}}));
        assert_eq!(
            s.fields["playlists"][0],
            json!({"items":[{"id":"k","note":8},{"id":"x"}],"updated":77})
        );
        assert_eq!(
            s.fields["playlists"][1]["items"].as_array().unwrap().len(),
            2
        );
        assert_eq!(s.fields["edits"]["d"]["title"], "old");
        assert_eq!(s.fields["notDupes"]["other"], 12);
        assert_eq!(s.queue.items()[1].id, "k");
        assert!(s.queue.items()[1].manual);
        assert_eq!(s.queue.pos(), 13.);
        assert_eq!(s.hidden("d"), mode == Disposition::Hide);
        let range = m.range(when.date(), when.date());
        assert_eq!((range.total, range.phone), (16., 16.));
        assert_eq!(range.tracks.len(), 1);
        assert_eq!(range.tracks[0].0.0, "k");
        assert_ne!(serde_json::to_value(s).unwrap()["queue"], Value::Null);
    }
}
