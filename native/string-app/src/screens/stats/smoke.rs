use super::*;
use crate::{integration::spotify, smoke::Case};
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};
use string_core::stats::Track;
use string_extras::spotify::ImportError;

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
/// After the Spotify step: Top artists and Top songs expanded, so a list longer than ten draws its rows past ten.
pub const EXPANDED: Case = Case {
    name: "stats-top-expanded",
    prepare: |v| {
        select(v, "week", false);
        v.s.set_field(more_field(true), true.into());
        v.s.set_field(more_field(false), true.into());
        v.views.stats.reveal_top = true;
        v.views.stats.top_shown = [0; 2];
    },
    ready: |v| {
        let Some(s) = v.views.stats.snapshot.as_ref() else {
            return false;
        };
        let want = [s.artists.len(), s.tracks.len()].map(|n| n.min(TOP_EXPANDED));
        s.tracks.len() > TOP && v.views.stats.top_shown == want
    },
};
enum Run {
    Start,
    Sent(Instant, Expect),
    Done,
}
#[derive(Clone, Copy)]
enum Expect {
    /// The export the self-test was given: imported, or nothing newer in it.
    Given,
    /// The made-up one: a history of this many plays.
    Plays(u64),
}
static RUN: Mutex<Run> = Mutex::new(Run::Start);
fn spotify_ready(v: &mut View<'_>) -> bool {
    let export = std::env::var_os("STRING_SMOKE_SPOTIFY_EXPORT").map(PathBuf::from);
    ready_with(v, export, crate::smoke::out())
}
/// `export`: the export to import, if one is given; `out`: where a made-up one goes, and the numbers. A history that
/// doesn't come in as it should fails the case.
fn ready_with(v: &mut View<'_>, export: Option<PathBuf>, out: Option<&Path>) -> bool {
    let mut run = RUN.lock().unwrap();
    match *run {
        Run::Start if !v.s.spotify_loading() => {
            let send = match export {
                Some(path) => Some((path, Expect::Given)),
                None if v.s.spotify.is_none() && v.s.spotify_error.is_none() => {
                    let planted = plant(v, out);
                    if planted.is_none() {
                        crate::smoke::fail("the made-up Spotify export couldn't be written".into());
                    }
                    planted
                }
                None => None,
            };
            *run = match send {
                Some((path, expect)) => {
                    if !v.views.drops.send(vec![path], true, v.adds) {
                        crate::smoke::fail(
                            "the Spotify export couldn't be dropped on Stats".into(),
                        );
                    }
                    Run::Sent(Instant::now(), expect)
                }
                None => {
                    report(v, None, out);
                    Run::Done
                }
            };
            false
        }
        Run::Sent(at, expect) if !v.views.drops.pending() && !v.s.spotify_busy() => {
            report(v, Some(at.elapsed()), out);
            *run = Run::Done;
            if let Err(why) = check(v, expect) {
                crate::smoke::fail(why);
            }
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
fn check(v: &View<'_>, expect: Expect) -> Result<(), String> {
    let outcome = v.s.spotify_outcome.as_ref();
    let said = outcome.map_or("No Spotify history was found there.", |o| &o.message);
    match expect {
        // A profile that already holds this export has nothing newer to add: that's not a failure.
        Expect::Given
            if outcome
                .is_some_and(|o| !o.failed || o.message == ImportError::NothingNew.to_string()) =>
        {
            Ok(())
        }
        Expect::Plays(plays) if v.s.spotify_summary().map(|s| s.plays) == Some(plays) => Ok(()),
        _ => Err(format!("the Spotify history didn't come in: {said}")),
    }
}
/// A made-up export under `out`: fifty plays over the last few days, the first three of the library's first songs.
/// With the plays the history should then hold.
fn plant(v: &View<'_>, out: Option<&Path>) -> Option<(PathBuf, Expect)> {
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
    let dir = out?.join("spotify-export");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("my_spotify_data.zip");
    let files = spotify::sample_files(50, &like);
    // A play without a title isn't a song to the import.
    let plays = files
        .iter()
        .flat_map(|(_, rows)| rows)
        .filter(|row| {
            row["master_metadata_track_name"]
                .as_str()
                .is_some_and(|t| !t.is_empty())
        })
        .count();
    spotify::write_export(&path, &files).ok()?;
    Some((path, Expect::Plays(plays as u64)))
}
/// Numbers only: no song names, no paths.
fn report(v: &View<'_>, took: Option<Duration>, out: Option<&Path>) {
    let Some(out) = out else {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        storage::{self, Storage},
        view::Views,
    };
    use string_platform::{
        errors_log::ErrorLog,
        instance::{self, Claim, Names},
    };

    struct Quiet;
    impl string_audio::Host for Quiet {
        fn next_after(&mut self, _: &string_audio::Track) -> Option<string_audio::Track> {
            None
        }
        fn event(&mut self, _: string_audio::Event) {}
    }

    /// Runs `stats-spotify` on a new profile in `dir` as frames would, until it's ready (with the plays the history
    /// then holds) or has failed. `damage` runs on the profile folder once the history read at open is in.
    fn run_case(
        dir: &Path,
        export: Option<PathBuf>,
        damage: impl FnOnce(&Path),
    ) -> Result<Option<u64>, String> {
        let ctx = egui::Context::default();
        let profile = dir.join("profile");
        let (_store, mut s) = Storage::open(&storage::folder(&profile, true), &ctx).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while s.spotify_loading() {
            assert!(Instant::now() < deadline, "the history read never came in");
            s.poll_spotify();
            std::thread::sleep(Duration::from_millis(2));
        }
        damage(&profile);
        let out = dir.join("out");
        std::fs::create_dir_all(&out).unwrap();
        let args = crate::Args {
            root: profile,
            explicit: true,
            smoke: Some(out.clone()),
            quit: false,
            add: None,
            playlist: None,
        };
        let names = Names::custom(&format!("StringSpotifySmoke{:x}", rand::random::<u64>()));
        let Claim::First(instance) = instance::claim_quietly(&names).unwrap() else {
            panic!()
        };
        let adds = crate::add_work::Adds::new().unwrap();
        let mut platform =
            crate::platform::Platform::new(instance, &args, Arc::new(ErrorLog::new(dir)));
        let player = string_audio::Player::new(
            string_audio::Config {
                muted: true,
                ..Default::default()
            },
            Quiet,
        );
        let mut views = Views::default();
        let mut covers = crate::ui::covers::CoverCache::new(&ctx);
        let mut v = View {
            adds: &adds.input,
            platform: &mut platform,
            s: &mut s,
            player: &player,
            st: Default::default(),
            views: &mut views,
            covers: &mut covers,
            viz: &Default::default(),
            scan: &mut Default::default(),
            tint: crate::theme::Tint::Mono,
            mono: true,
            force_mute: true,
        };
        (SPOTIFY.prepare)(&mut v);
        loop {
            let ready = ready_with(&mut v, export.clone(), Some(&out));
            if let Some(why) = crate::smoke::take_failure() {
                return Err(why);
            }
            if ready {
                return Ok(v.s.spotify_summary().map(|s| s.plays));
            }
            assert!(Instant::now() < deadline, "the case never ended");
            // What the frames do meanwhile.
            let views = &mut *v.views;
            views.drops.poll(v.s, &mut views.toast);
            v.s.poll_spotify();
            v.views.stats.snapshot = Some(Arc::new(Snapshot::read(v.s, &v.views.stats, Week)));
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    fn numbers(dir: &Path) -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(dir.join("out/stats-spotify.json")).unwrap()).unwrap()
    }

    // One test, as the case keeps its progress in a static.
    #[test]
    fn the_spotify_case_passes_only_with_the_history_it_imported() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(run_case(dir.path(), None, |_| {}), Ok(Some(50)));
        assert_eq!(numbers(dir.path())["added"], 50);

        // The made-up export's import fails: a folder stands where spotify.json would be written.
        let dir = tempfile::tempdir().unwrap();
        let failed = run_case(dir.path(), None, |profile| {
            std::fs::create_dir(profile.join("spotify.json")).unwrap()
        });
        assert!(failed.is_err(), "{failed:?}");
        assert!(numbers(dir.path())["error"].is_string());

        // An export the self-test was given that can't be imported.
        let dir = tempfile::tempdir().unwrap();
        let broken = dir.path().join("given.zip");
        std::fs::write(&broken, b"not a zip").unwrap();
        let failed = run_case(dir.path(), Some(broken), |_| {});
        assert!(failed.is_err(), "{failed:?}");
        let error = numbers(dir.path())["error"].clone();
        assert!(
            error.as_str().is_some_and(|e| e.contains("given.zip")),
            "{error}"
        );
    }
}
