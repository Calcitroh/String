use super::*;
use crate::smoke::Case;
use serde_json::json;
use string_core::stats::Track;

fn select(v: &mut View<'_>, kind: &str, table: bool) {
    v.go(Route::Stats);
    v.s.set_field("statsKind", kind.into());
    v.s.set_field("statsTable", table.into());
}
fn seed(v: &mut View<'_>) {
    let now = Local::now().naive_local();
    let today = now.date();
    for (i, id) in v.s.models.songs.iter().take(4).enumerate() {
        let song = &v.s.models.by_id[id];
        let track = Track(id.clone(), song.title.clone(), song.artist.clone());
        for back in 0..4 {
            let at = (today - chrono::Days::new(back))
                .and_hms_opt(8 + i as u32, 0, 0)
                .unwrap();
            for _ in 0..(i + 1) * (back as usize + 1) * 3 {
                v.s.listening.add(&track, 30., at);
            }
        }
    }
    v.s.listening_dirty = true;
    select(v, "day", false);
}
pub const CASES: &[Case] =
    &[
        Case {
            name: "20-stats-today",
            prepare: seed,
            ready: |v| {
                v.views
                    .stats
                    .snapshot
                    .as_ref()
                    .is_some_and(|s| s.kind == Day && s.buckets.len() == 24 && s.tracks.len() >= 2)
            },
        },
        Case {
            name: "21-stats-week",
            prepare: |v| select(v, "week", false),
            ready: |v| {
                v.views
                    .stats
                    .snapshot
                    .as_ref()
                    .is_some_and(|s| s.kind == Week && s.buckets.len() == 7)
            },
        },
        Case {
            name: "22-stats-table",
            prepare: |v| select(v, "week", true),
            ready: |v| {
                v.s.field("statsTable") == true
                    && v.views
                        .stats
                        .snapshot
                        .as_ref()
                        .is_some_and(|s| s.kind == Week)
            },
        },
        Case {
            name: "30-stats-phone",
            prepare: |v| {
                select(v, "week", false);
                if v.views.stats.phone_connected {
                    v.s.set_setting("phoneUser", "u1".into());
                    v.views.stats.phone_sync_requested = true;
                    return;
                }
                v.s.set_field(
                    "phone",
                    json!({"found":true,"plays":2,"at":Local::now().timestamp_millis()}),
                );
                v.s.set_setting("phoneUser", "fixture".into());
                v.views.stats.phone_users = vec![
                    ("fixture".into(), "Fixture listener".into()),
                    ("second".into(), "Second listener".into()),
                ];
                let t = Track(
                    "phone-only".into(),
                    "Old Song".into(),
                    "Someone Else".into(),
                );
                v.s.listening
                    .add_phone(&t, 416., Local::now().naive_local());
                v.s.listening_dirty = true;
                v.views.stats.snapshot = None;
                v.views.stats.reveal_phone = true;
            },
            ready: |v| {
                if v.views.stats.phone_connected {
                    if v.s.field("phone")["plays"] != 4 || v.views.stats.phone_users.len() != 2 {
                        return false;
                    }
                    if !v.views.stats.snapshot.as_ref().is_some_and(|s| {
                        s.note.contains("on your phone") && s.phone.users.len() == 2
                    }) {
                        v.views.stats.snapshot = None;
                        v.views.stats.reveal_phone = true;
                        return false;
                    }
                    if !v.views.stats.phone_visible {
                        v.views.stats.reveal_phone = true;
                    }
                }
                v.views.stats.phone_visible
                    && v.views
                        .stats
                        .snapshot
                        .as_ref()
                        .is_some_and(|s| s.note.contains("on your phone"))
            },
        },
    ];
