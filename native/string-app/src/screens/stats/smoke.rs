use super::*;
use crate::{integration::spotify, smoke::Case};
use serde_json::json;
use std::{path::PathBuf, sync::Mutex};
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

/// Stats with a Spotify history, imported the way a drop on Stats is: the export `STRING_SMOKE_SPOTIFY_EXPORT`
/// names, or a small made-up one when the profile has no history yet. Its numbers go to `stats-spotify.json`.
pub const SPOTIFY: Case = Case {
    name: "stats-spotify",
    prepare: |v| {
        crate::ui::menus::smoke::reset(v);
        select(v, "week", false);
        *RUN.lock().unwrap() = Run::Start;
    },
    ready: spotify_ready,
};
enum Run {
    Start,
    Sent(Instant),
    Done,
}
static RUN: Mutex<Run> = Mutex::new(Run::Start);
fn spotify_ready(v: &mut View<'_>) -> bool {
    let mut run = RUN.lock().unwrap();
    match *run {
        Run::Start if !v.s.spotify_loading() => {
            let export = std::env::var_os("STRING_SMOKE_SPOTIFY_EXPORT")
                .map(PathBuf::from)
                .or_else(|| {
                    (v.s.spotify.is_none() && v.s.spotify_error.is_none())
                        .then(|| plant(v))
                        .flatten()
                });
            let sent = export.is_some_and(|path| v.views.drops.send(vec![path], true, v.adds));
            *run = if sent {
                Run::Sent(Instant::now())
            } else {
                report(v, None);
                Run::Done
            };
            false
        }
        Run::Sent(at) if !v.views.drops.pending() && !v.s.spotify_busy() => {
            report(v, Some(at.elapsed()));
            *run = Run::Done;
            false
        }
        // Drawn with the history as it is now: imported, or the panel saying why not.
        Run::Done => v
            .views
            .stats
            .snapshot
            .as_ref()
            .is_some_and(|s| s.spotify_revision == v.s.spotify_revision),
        _ => false,
    }
}
/// A made-up export under `--out`: fifty plays over the last few days, the first three of the library's first songs.
fn plant(v: &View<'_>) -> Option<PathBuf> {
    let like: Vec<_> =
        v.s.models
            .songs
            .iter()
            .take(3)
            .map(|id| {
                let song = &v.s.models.by_id[id];
                (song.title.clone(), song.artist.clone())
            })
            .collect();
    let dir = crate::smoke::out()?.join("spotify-export");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("my_spotify_data.zip");
    spotify::write_export(&path, &spotify::sample_files(50, &like)).ok()?;
    Some(path)
}
/// Numbers only: no song names, no paths.
fn report(v: &View<'_>, took: Option<Duration>) {
    let Some(out) = crate::smoke::out() else {
        return;
    };
    let outcome = v.s.spotify_outcome.as_ref();
    let summary = outcome.and_then(|o| o.summary.as_ref());
    let seconds = summary.map_or(0., |s| s.seconds);
    let mut numbers = json!({
        "added": summary.map_or(0, |s| s.added),
        "seconds": seconds,
        "hours": seconds / 3600.,
        "matchedShare": v.s.spotify_summary().map_or(0., |s| s.matched_share),
        "importMs": took.map_or(0, |t| t.as_millis() as u64),
        "files": summary.map_or(0, |s| s.files),
    });
    if let Some(failed) = outcome.filter(|o| o.failed) {
        numbers["error"] = failed.message.clone().into();
    } else if took.is_some() && outcome.is_none() {
        numbers["error"] = "No Spotify history was found there.".into();
    }
    let _ = std::fs::write(
        out.join("stats-spotify.json"),
        serde_json::to_vec_pretty(&numbers).unwrap_or_default(),
    );
}
