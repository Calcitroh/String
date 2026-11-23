use super::*;
use crate::{smoke::Case, ui::covers::Size};
use serde_json::json;

fn cue(v: &mut View<'_>, title: &str, at: f64) {
    let ids = v.s.models.songs.clone();
    if let Some(i) = ids
        .iter()
        .position(|id| v.s.models.by_id[id].title == title)
    {
        v.s.state.queue.play_ids(
            &ids,
            i as isize,
            json!({"kind":"songs","name":"All songs"}),
            &v.s.models.catalog,
        );
        v.s.load_current(v.player, true, at);
        v.views.overlay = Some(Overlay::Lyrics);
    }
}
fn ready(v: &mut View<'_>, source: Source, synced: bool) -> bool {
    let art = v.s.current().is_some_and(|s| {
        v.covers
            .get(&s.cover, Size::Large, v.mono.then_some(v.tint))
            .is_some()
    });
    art && v
        .views
        .lyrics
        .as_ref()
        .and_then(|s| s.page.as_ref())
        .is_some_and(|p| {
            Some(p.id.as_str()) == v.s.state.queue.current()
                && p.lookup.is_none()
                && p.found
                    .as_ref()
                    .is_some_and(|r| r.source == source && r.synced == synced)
        })
}
pub const CASES: &[Case] = &[
    Case {
        name: "15-lyrics-synced",
        prepare: |v| {
            v.s.set_setting("lyricsOnline", serde_json::Value::Null);
            v.s.set_setting("translate", false.into());
            cue(v, "Low Tide", 5.4);
        },
        ready: |v| {
            ready(v, Source::File, true) && v.st.playing && (5.4..8.).contains(&v.st.position)
        },
    },
    Case {
        name: "15b-lyrics-plain",
        prepare: |v| cue(v, "Northern Static", 0.),
        ready: |v| ready(v, Source::File, false),
    },
    Case {
        name: "16-lyrics-ask",
        prepare: |v| cue(v, "Glass Orbit", 2.4),
        ready: |v| ready(v, Source::None, false) && v.s.setting("lyricsOnline").is_null(),
    },
    Case {
        name: "17-lyrics-online",
        prepare: |v| v.s.set_setting("lyricsOnline", true.into()),
        ready: |v| ready(v, Source::Lrclib, true),
    },
    Case {
        name: "18-lyrics-translated",
        prepare: |v| {
            v.s.set_setting("translate", true.into());
            cue(v, "Low Tide", 5.4);
        },
        ready: |v| {
            ready(v, Source::File, true)
                && v.views.lyrics.as_ref().is_some_and(|s| {
                    s.translator.status.starts_with("Translated into English")
                        && s.translator.rows.len() == 5
                        && s.translator
                            .rows
                            .iter()
                            .flatten()
                            .filter(|r| r.starts_with("[EN]"))
                            .count()
                            == 5
                })
        },
    },
];
