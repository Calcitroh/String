//! Regression cases for queue edits, preload, continuation, shuffle and restored history.

use serde_json::{Value, json};
use string_core::{
    queue::{Catalog, Group, NextAction, PlaybackOptions, Queue, Repeat, Song},
    state::State,
};

fn ids(s: &str) -> Vec<String> {
    s.split_whitespace().map(str::to_owned).collect()
}
fn catalog() -> Catalog {
    let songs = ids("s1 s2 s3 s4");
    let tracks = songs
        .iter()
        .enumerate()
        .map(|(i, id)| {
            (
                id.clone(),
                Song {
                    artist_key: if matches!(i, 0 | 3) { "a" } else { "b" }.into(),
                    ..Default::default()
                },
            )
        })
        .collect();
    let group = |key: &str, s| Group {
        key: key.into(),
        name: key.into(),
        ids: ids(s),
    };
    Catalog {
        songs,
        tracks,
        albums: vec![group("one", "s1"), group("two", "s2 s3")],
        artists: vec![group("a", "s1 s4"), group("b", "s2 s3")],
    }
}
fn ctx(kind: &str, key: &str) -> Value {
    json!({"kind":kind,"key":key})
}
fn opts() -> PlaybackOptions {
    PlaybackOptions::default()
}

#[test]
fn replacing_a_copy_preserves_live_queue_and_invalidates_preload() {
    let lib = catalog();
    for include_current in [false, true] {
        let mut state: State = serde_json::from_value(json!({"v":1,"queue":{
            "items":["s1","s1","s1","s3"],"orig":["s3","s1"],"man":[2],"at":1,"pos":17.5,
            "listStart":1,"past":[{"start":0,"ctx":{"key":"old"}}],"ctx":{"key":"new"}
        }}))
        .unwrap();
        let q = &mut state.queue;
        q.confirm_playing("s1");
        let plan = q.what_next(false, opts(), &lib);
        q.replace_track_id("s1", "s2", include_current);
        assert!(!q.commit_next(plan, opts(), &lib));
        assert_eq!(q.history(), ids("s1"));
        assert_eq!((q.at(), q.pos()), (Some(1), 17.5));
        assert_eq!(q.current(), Some(if include_current { "s2" } else { "s1" }));
        assert_eq!(q.original(), ids("s3 s2"));
        assert_eq!(q.items()[0].id, "s2");
        assert_eq!(q.items()[2].id, "s2");
        assert!(q.items()[2].manual);
        assert_eq!(q.context(Some(0))["key"], "old");
        assert_eq!(q.context(None)["key"], "new");
    }
}
fn upcoming(q: &Queue) -> Vec<String> {
    q.upcoming().iter().map(|x| x.id.clone()).collect()
}

#[test]
fn manual_entries_before_playback_and_replacing_a_list() {
    let lib = catalog();
    let mut q = Queue::default();
    assert_eq!(q.insert(&ids("s1 missing"), true, &lib), ids("s1"));
    q.insert(&ids("s2"), true, &lib);
    q.insert(&ids("s3"), false, &lib);
    assert_eq!(upcoming(&q), ids("s3 s1 s2"));
    q.play_ids(&ids("s1 s4"), 0, Value::Null, &lib);
    assert_eq!(upcoming(&q), ids("s3 s1 s2 s4"));
    q.clear_manual();
    assert_eq!(upcoming(&q), ids("s4"));
}
#[test]
fn clicked_index_counts_unplayable_and_hidden_rows() {
    let mut lib = catalog();
    let mut q = Queue::default();
    lib.tracks.get_mut("s1").unwrap().unplayable = true;
    for (list, i, want) in [
        ("s1 s2 s3 s4", 1, "s2"),
        ("s1 s2 s3 s4", 0, "s2"),
        ("s2 s1 s3 s4", 2, "s3"),
    ] {
        q.play_ids(&ids(list), i, Value::Null, &lib);
        assert_eq!(q.current(), Some(want));
    }
    lib.tracks.get_mut("s4").unwrap().hidden = true;
    q.play_ids(&lib.songs, 3, Value::Null, &lib);
    assert_eq!(q.current(), Some("s3"));
}
#[test]
fn speculative_album_promotes_on_start_and_restores_history() {
    let lib = catalog();
    let mut state = State::default();
    let q = &mut state.queue;
    q.play_ids(&ids("s1"), 0, ctx("album", "one"), &lib);
    let ticket = q.what_next(false, opts(), &lib);
    assert_eq!(ticket.id(), Some("s2"));
    assert_eq!(q.items().len(), 1);
    let bytes = serde_json::to_vec(&state).unwrap();
    let mut state: State = serde_json::from_slice(&bytes).unwrap();
    let q = &mut state.queue;
    assert_eq!(q.items().len(), 1);
    let ticket = q.what_next(false, opts(), &lib);
    assert!(q.commit_next(ticket, opts(), &lib));
    assert_eq!(q.current(), Some("s2"));
    assert_eq!(q.context(None)["key"], "two");
    let bytes = serde_json::to_vec(&state).unwrap();
    let mut state: State = serde_json::from_slice(&bytes).unwrap();
    let q = &mut state.queue;
    q.set_repeat(Repeat::All);
    for want in ["s3", "s2"] {
        q.next(false, opts(), &lib);
        assert_eq!(q.current(), Some(want));
    }
    assert_eq!(q.previous(0., &lib), Some("s1"));
    assert_eq!(q.context(None)["key"], "one");
    q.set_repeat(Repeat::Off);
    q.play_ids(&ids("s2 s3"), 1, ctx("album", "two"), &lib);
    q.next(true, opts(), &lib);
    assert_eq!(q.current(), Some("s1"));
}
#[test]
fn liked_full_or_partial_and_all_songs_wrap() {
    let mut lib = catalog();
    let mut q = Queue::default();
    for (list, i, kind, want) in [
        ("s1", 0, "liked", "s4"),
        ("s1 s2 s3 s4", 3, "liked", "s1"),
        ("s1 s2 s3 s4", 3, "songs", "s1"),
    ] {
        q.play_ids(&ids(list), i, ctx(kind, ""), &lib);
        q.next(true, opts(), &lib);
        assert_eq!(q.current(), Some(want));
    }
    lib.songs = ids("s1 missing s2");
    lib.tracks.get_mut("s1").unwrap().unplayable = true;
    q.play_ids(&ids("s2"), 0, ctx("liked", ""), &lib);
    assert_eq!(q.next(true, opts(), &lib), NextAction::Restart);
    lib.tracks.get_mut("s1").unwrap().unplayable = false;
    q.play_ids(&ids("s1"), 0, ctx("liked", ""), &lib);
    lib.tracks.get_mut("s1").unwrap().unplayable = true;
    q.next(false, opts(), &lib);
    assert_eq!(q.current(), Some("s2"));
}
#[test]
fn unplayable_current_album_retains_display_order_and_artist_context_advances() {
    let mut lib = catalog();
    let mut q = Queue::default();
    lib.albums.push(Group {
        key: "three".into(),
        name: "Three".into(),
        ids: ids("s4"),
    });
    q.play_ids(&ids("s2"), 0, ctx("album", "two"), &lib);
    lib.tracks.get_mut("s2").unwrap().unplayable = true;
    q.next(false, opts(), &lib);
    assert_eq!(q.current(), Some("s4"));
    q.play_ids(&ids("s4"), 0, ctx("artist", "a"), &lib);
    q.next(false, opts(), &lib);
    assert_eq!(q.current(), Some("s3"));
    assert_eq!(q.context(None)["kind"], "artist");
}
#[test]
fn shuffle_honors_manual_positions_and_explicit_removals() {
    let lib = catalog();
    let mut q = Queue::default();
    q.play_ids(&lib.songs, 1, Value::Null, &lib);
    q.remove(2);
    q.insert(&ids("s3"), false, &lib);
    q.move_before(2, 4);
    q.set_shuffle(true, &lib);
    let up = upcoming(&q);
    assert!(up.contains(&"s1".into()));
    assert_eq!(up[1], "s3");
    assert_eq!(up.iter().filter(|s| *s == "s3").count(), 1);
    q.set_shuffle(false, &lib);
    assert_eq!(upcoming(&q), ids("s4 s3"));
    q.clear_manual();
    q.play_ids(&lib.songs, 3, ctx("songs", ""), &lib);
    q.set_shuffle(true, &lib);
    assert_eq!(q.upcoming().len(), 3);
    assert!(!upcoming(&q).contains(&"s4".into()));
}
#[test]
fn shuffled_wrap_excludes_newly_hidden_song() {
    let mut lib = catalog();
    let mut q = Queue::default();
    q.set_shuffle(true, &lib);
    q.play_ids(&lib.songs, 0, ctx("songs", ""), &lib);
    q.jump(3, &lib);
    let id = q.current().unwrap().to_owned();
    lib.tracks.get_mut(&id).unwrap().hidden = true;
    q.next(false, opts(), &lib);
    assert_ne!(q.current(), Some(id.as_str()));
    assert!(!upcoming(&q).contains(&id));
}
#[test]
fn mode_and_queue_edits_invalidate_preload() {
    let lib = catalog();
    let mut q = Queue::default();
    q.play_ids(&ids("s1"), 0, ctx("album", "one"), &lib);
    let disabled = PlaybackOptions {
        autoplay: false,
        ..opts()
    };
    let looping = PlaybackOptions {
        looping: true,
        ..opts()
    };
    for opt in [disabled, looping] {
        let p = q.what_next(false, opts(), &lib);
        assert!(!q.commit_next(p, opt, &lib));
        assert_eq!(q.next(true, opt, &lib), NextAction::Stop);
    }
    let p = q.what_next(false, opts(), &lib);
    q.cycle_repeat();
    assert!(!q.commit_next(p, opts(), &lib));
    assert_eq!(q.what_next(false, opts(), &lib).id(), Some("s1"));
    q.cycle_repeat();
    assert_eq!(q.next(false, opts(), &lib), NextAction::Restart);
    q.cycle_repeat();
    let p = q.what_next(false, opts(), &lib);
    q.insert(&ids("s3"), false, &lib);
    assert!(!q.commit_next(p, opts(), &lib));
    q.next(true, opts(), &lib);
    assert_eq!(q.current(), Some("s3"));
    assert_eq!(q.previous(4., &lib), Some("s3"));
    assert_eq!(q.previous(3.5, &lib), Some("s1"));
}
#[test]
fn old_queue_restores_omitted_ends_without_guessing_ambiguous_order() {
    let order: Vec<_> = (0..1500).map(|i| format!("track-{i}")).collect();
    let doc = json!({"v":1,"queue":{"at":100,"items":order[150..1050],"orig":null,"man":[1],"ctx":{"kind":"songs"}}});
    let mut state: State = serde_json::from_value(doc.clone()).unwrap();
    assert!(state.queue.needs_legacy_order());
    state.queue.restore_legacy_order(order.clone());
    assert_eq!(
        state.queue.items().len(),
        900,
        "non-contiguous saved order cannot be expanded"
    );
    let mut doc = doc;
    doc["queue"]["man"] = json!([]);
    let mut state: State = serde_json::from_value(doc).unwrap();
    state.queue.restore_legacy_order(order);
    assert_eq!(state.queue.items().len(), 1500);
    assert_eq!(state.queue.at(), Some(250));
}
#[test]
fn older_list_edits_shift_boundaries_and_history_is_capped() {
    let lib = catalog();
    let mut q = Queue::default();
    q.play_ids(&vec!["s1".into(); 250], 249, ctx("album", "one"), &lib);
    q.next(false, opts(), &lib);
    assert_eq!(q.at(), Some(200));
    q.previous(0., &lib);
    q.insert(&ids("s4"), false, &lib);
    assert_eq!(q.context(Some(200))["key"], "one");
    assert_eq!(q.context(Some(201))["key"], "two");
    q.clear_manual();
    assert_eq!(q.context(Some(200))["key"], "two");
}

#[test]
fn shuffled_preload_is_repeatable_and_recent_requires_confirmed_output() {
    let lib = catalog();
    let mut s = State::default();
    s.queue.set_shuffle(true, &lib);
    s.play_ids(&ids("s1"), 0, ctx("album", "one"), &lib, 10);
    let plan = s.queue.what_next(false, opts(), &lib);
    let candidate = plan.id().unwrap().to_owned();
    for _ in 0..10 {
        assert_eq!(
            s.queue.what_next(false, opts(), &lib).id(),
            Some(candidate.as_str())
        );
    }
    assert_eq!(s.fields["recent"][0]["key"], "one");
    s.queue.confirm_playing(&candidate);
    assert!(s.queue.history().is_empty());
    s.queue.confirm_playing("s1");
    assert!(s.commit_next(plan, opts(), &lib, 20));
    assert_eq!(s.fields["recent"][0]["key"], "two");
    assert_eq!(s.queue.history(), ids("s1"));
    s.queue.confirm_playing(&candidate);
    s.queue.confirm_playing(&candidate);
    assert_eq!(s.queue.history(), [candidate, "s1".into()]);
    let mut loaded: State = serde_json::from_slice(&serde_json::to_vec(&s).unwrap()).unwrap();
    assert!(loaded.queue.history().is_empty());
    let id = loaded.queue.current().unwrap().to_owned();
    loaded.queue.confirm_playing(&id);
    assert_eq!(loaded.queue.history(), [id]);
}

#[test]
fn compact_equal_order_manual_indices_and_legacy_defaults() {
    let doc = json!({"v":1,"queue":{"items":["a","b","a"],"man":[0],"orig":["a","b","a"],"at":8},"set":{"fade":3},"sort":{"dir":-1}});
    let s: State = serde_json::from_value(doc).unwrap();
    assert_eq!(s.queue.at(), Some(2));
    assert_eq!(s.fields["set"]["autoplay"], true);
    assert_eq!(s.fields["sort"]["key"], "title");
    let wire: Value = serde_json::from_slice(&serde_json::to_vec(&s).unwrap()).unwrap();
    assert!(wire["queue"]["orig"].is_null());
    assert_eq!(wire["queue"]["items"], json!([-1, 1, 0]));
    assert_eq!(wire["queue"]["paths"], json!([[0, "a", 0], [0, "b", 0]]));
}
