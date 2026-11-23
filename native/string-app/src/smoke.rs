//! Each case prepares a view, waits for its dependencies, then checks the rendered frame.
use crate::{
    route::Route,
    screens::library_smoke as lib,
    ui::{covers::Size, menus::smoke as lists},
    view::{Overlay, View},
};
use crossbeam_channel::{Receiver, bounded};
use egui::Context;
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};
use windows::Win32::{
    Foundation::FILETIME,
    System::Threading::{GetCurrentProcess, GetProcessTimes},
};
pub struct Case {
    pub name: &'static str,
    pub prepare: fn(&mut View<'_>),
    pub ready: fn(&mut View<'_>) -> bool,
}
fn library(v: &mut View<'_>) {
    v.go(Route::Home);
    v.go(Route::Songs);
    v.views.history.back();
    v.views.history.forward();
    v.route_changed();
}
fn library_ready(v: &mut View<'_>) -> bool {
    if !v.scan.running()
        && v.scan.completed > 0
        && v.s.models.songs.is_empty()
        && v.views.history.current != Route::Home
    {
        v.go(Route::Home);
        return false;
    }
    (v.views.history.current == Route::Songs
        || (v.views.history.current == Route::Home && v.s.models.songs.is_empty()))
        && !v.scan.running()
        && v.scan.completed > 0
        && v.s.models.songs.iter().take(14).all(|id| {
            v.covers
                .get(
                    &v.s.models.by_id[id].cover,
                    Size::Small,
                    v.mono.then_some(v.tint),
                )
                .is_some()
        })
}
fn record(v: &mut View<'_>) {
    lists::reset(v);
    let ids = v.s.models.songs.clone();
    v.s.play_ids(
        v.player,
        &ids,
        0,
        serde_json::json!({"kind":"songs","key":"","name":"All songs"}),
    );
    v.views.overlay = Some(Overlay::Record);
}
fn record_ready(v: &mut View<'_>) -> bool {
    v.st.playing
        && v.st.position > 0.15
        && v.s.current().is_some_and(|s| {
            v.covers
                .get(&s.cover, Size::Large, v.mono.then_some(v.tint))
                .is_some()
        })
}
pub const CASES: &[Case] = &[
    Case {
        name: "01-songs",
        prepare: library,
        ready: library_ready,
    },
    crate::integration::smoke::HOST,
    crate::platform::live_checks::TRANSPORT,
    crate::integration::helpers::smoke::SIDEBAR,
    lib::HOME,
    lib::ALBUMS,
    lib::ARTISTS,
    lib::ALBUM,
    lib::ARTIST,
    lib::LIKED,
    lib::SEARCH,
    lib::SEARCH_GROUPS,
    lib::HOME_PLAYING,
    crate::ui::credit::smoke::SHOWN,
    crate::ui::credit::smoke::CLOSED,
    Case {
        name: "40-edit-info",
        prepare: crate::ui::edit_info::smoke::prepare,
        ready: crate::ui::edit_info::smoke::ready,
    },
    Case {
        name: "41-settings",
        prepare: crate::screens::settings::smoke::open,
        ready: crate::screens::settings::smoke::ready,
    },
    Case {
        name: "42-settings-updates",
        prepare: crate::screens::settings::smoke::updates,
        ready: crate::screens::settings::smoke::updates_ready,
    },
    Case {
        name: "43-duplicates",
        prepare: crate::screens::dupes::smoke::open,
        ready: crate::screens::dupes::smoke::settled,
    },
    Case {
        name: "44-duplicates-ask",
        prepare: crate::screens::dupes::smoke::ask,
        ready: crate::screens::dupes::smoke::asking,
    },
    Case {
        name: "45-duplicates-hidden",
        prepare: crate::screens::dupes::smoke::hide,
        ready: crate::screens::dupes::smoke::settled,
    },
    Case {
        name: "46-duplicates-back",
        prepare: crate::screens::dupes::smoke::back,
        ready: crate::screens::dupes::smoke::settled,
    },
    Case {
        name: "47-look",
        prepare: crate::screens::settings::smoke::look,
        ready: crate::screens::settings::smoke::look_ready,
    },
    Case {
        name: "queue-drawer",
        prepare: lists::queue,
        ready: lists::queue_ready,
    },
    Case {
        name: "queue-recent",
        prepare: lists::recent,
        ready: lists::recent_ready,
    },
    Case {
        name: "menu-track",
        prepare: lists::menu,
        ready: lists::menu_ready,
    },
    Case {
        name: "playlist-page",
        prepare: lists::playlist,
        ready: lists::playlist_ready,
    },
    Case {
        name: "playlist-drop",
        prepare: lists::dropping,
        ready: lists::playlist_ready,
    },
    Case {
        name: "menu-playlist",
        prepare: lists::list_menu,
        ready: lists::menu_ready,
    },
    Case {
        name: "dialog-delete-playlist",
        prepare: lists::dialog,
        ready: lists::dialog_ready,
    },
    Case {
        name: "playlist-filter-menu",
        prepare: lists::filtered,
        ready: lists::menu_ready,
    },
    Case {
        name: "menu-add-to-playlist",
        prepare: lists::picker,
        ready: lists::menu_ready,
    },
    Case {
        name: "dropzone",
        prepare: lists::dropzone,
        ready: lists::ready,
    },
    Case {
        name: "02-now-playing",
        prepare: record,
        ready: record_ready,
    },
];
fn cpu() -> Result<u64, String> {
    let [mut c, mut e, mut k, mut u] = [FILETIME::default(); 4];
    unsafe { GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u) }
        .map_err(|e| e.to_string())?;
    let ticks = |f: FILETIME| (u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime);
    Ok(ticks(k) + ticks(u))
}
enum Step {
    Resume(f64),
    Prepare,
    Ready,
    Readback(Receiver<Result<(), String>>),
    LibraryInput,
    Warm,
    Measure(Instant, u64, u64),
    Advance(String),
    Pause,
}
pub struct Smoke {
    cases: Vec<&'static Case>,
    out: PathBuf,
    index: usize,
    step: Step,
    since: Instant,
    frames: u64,
    probe: crate::screens::library_input::Probe,
    checks: Vec<String>,
    perf: Value,
    ended: bool,
    skipped: Vec<String>,
}
impl Smoke {
    pub fn new(out: PathBuf) -> Self {
        let mut cases: Vec<_> = CASES.iter().collect();
        if std::env::var_os("STRING_SMOKE_LRCLIB_PORT").is_some()
            && std::env::var_os("STRING_SMOKE_OLLAMA_PORT").is_some()
        {
            cases.splice(
                1..1,
                crate::screens::lyrics::smoke::CASES
                    .iter()
                    .chain(crate::screens::stats::smoke::CASES),
            );
        }
        Self {
            cases,
            out,
            index: 0,
            step: Step::Prepare,
            since: Instant::now(),
            frames: 0,
            probe: Default::default(),
            checks: vec![],
            perf: Value::Null,
            ended: false,
            skipped: vec![],
        }
    }
    pub fn expect_resume(&mut self, pos: f64) {
        self.step(Step::Resume(pos));
    }
    fn step(&mut self, step: Step) {
        self.step = step;
        self.since = Instant::now();
    }
    pub fn tick(&mut self, ctx: &Context, v: &mut View<'_>) -> Result<bool, String> {
        if self.ended {
            return Ok(true);
        }
        if self.frames == 0 {
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(1384., 841.)));
        }
        self.frames += 1;
        if self.since.elapsed() > Duration::from_secs(14) {
            if matches!(self.step, Step::LibraryInput) {
                return Err(format!(
                    "Timed out in library interaction step {}",
                    self.probe.step()
                ));
            }
            return Err(format!(
                "Timed out waiting for smoke case {}. Playback requires a decodable song in the test profile.",
                self.index + 1
            ));
        }
        match &self.step {
            Step::Resume(pos) => {
                if v.st.playing {
                    return Err("Restored playback started without input".into());
                }
                if v.st.key.is_some() && (v.st.position - pos).abs() < 0.10 {
                    self.checks
                        .push("restored saved track paused at its position".into());
                    self.step(Step::Prepare);
                }
            }
            Step::Prepare => {
                if lib::skip(self.cases[self.index].name, v) {
                    self.skipped.push(format!(
                        "{} requires a tagged album.",
                        self.cases[self.index].name
                    ));
                    self.index += 1;
                    return Ok(false);
                }
                (self.cases[self.index].prepare)(v);
                self.step(Step::Ready);
            }
            Step::Ready => {
                let c = self.cases[self.index];
                if (c.ready)(v) && self.since.elapsed() > Duration::from_millis(150) {
                    let name = if v.s.models.songs.is_empty() {
                        "02-welcome"
                    } else {
                        c.name
                    };
                    let path = self.out.join(format!("{name}.png"));
                    let (tx, rx) = bounded(1);
                    let ctx2 = ctx.clone();
                    ctx.request_screenshot(move |im| {
                        let result = image::save_buffer(
                            &path,
                            bytemuck::cast_slice(&im.pixels),
                            im.width() as u32,
                            im.height() as u32,
                            image::ColorType::Rgba8,
                        )
                        .map_err(|e| e.to_string());
                        let _ = tx.send(result);
                        ctx2.request_repaint();
                    });
                    self.step(Step::Readback(rx));
                }
            }
            Step::Readback(rx) => {
                if let Ok(result) = rx.try_recv() {
                    result?;
                    self.checks.push(
                        if v.s.models.songs.is_empty() {
                            "02-welcome"
                        } else {
                            self.cases[self.index].name
                        }
                        .into(),
                    );
                    self.index += 1;
                    if v.s.models.songs.is_empty() {
                        self.skipped
                            .push("Playback and performance require a library song.".into());
                        self.ended = true;
                        return Ok(true);
                    }
                    if self.cases[self.index - 1].name == lib::HOME_PLAYING.name {
                        self.step(Step::LibraryInput);
                    } else if self.index == self.cases.len() {
                        self.step(Step::Warm);
                    } else {
                        self.step(Step::Prepare);
                    }
                }
            }
            Step::LibraryInput => {
                if self.probe.tick(ctx, v)? {
                    self.checks.push(
                        "persistent filter input, song sort, album sort and search Enter".into(),
                    );
                    if !v.s.models.catalog.albums.is_empty() {
                        self.checks.push("hover card play, pause and resume".into());
                    }
                    self.step(Step::Prepare);
                }
            }
            Step::Warm => {
                if self.since.elapsed() > Duration::from_millis(600) {
                    self.step(Step::Measure(Instant::now(), cpu()?, self.frames));
                }
            }
            Step::Measure(start, ticks, frames) => {
                let secs = start.elapsed().as_secs_f64();
                if secs >= 2. {
                    if !v.st.playing {
                        return Err("Playback stopped during measurement".into());
                    }
                    self.perf = json!({"wall_seconds":secs,"ui_frames":self.frames-*frames,"ui_frames_per_second":(self.frames-*frames)as f64/secs,"cpu_percent_one_core":(cpu()?-*ticks)as f64*0.00001/secs,"muted":v.force_mute,"off_screen":true});
                    if let (Some(next), Some(dur)) =
                        (v.s.state.queue.upcoming().first(), v.st.duration)
                    {
                        let id = next.id.clone();
                        v.platform
                            .smoke_seek(Duration::from_secs_f64((dur - 0.85).max(0.)));
                        self.step(Step::Advance(id));
                    } else {
                        v.platform.smoke_play_pause();
                        self.step(Step::Pause);
                    }
                }
            }
            Step::Advance(expected) => {
                if v.st.playing
                    && v.st.position > 0.1
                    && v.s.state.queue.current() == Some(expected.as_str())
                    && v.s.state.queue.history().first() == Some(expected)
                {
                    self.checks
                        .push("native seek, automatic queue advance and confirmed history".into());
                    v.platform.smoke_play_pause();
                    self.step(Step::Pause);
                }
            }
            Step::Pause => {
                if !v.st.playing && v.st.position > 0. {
                    self.checks
                        .push("taskbar command paused with saved position".into());
                    self.ended = true;
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
    pub fn report(&self, error: Option<&str>) -> std::io::Result<()> {
        fs::write(
            self.out.join("results.json"),
            serde_json::to_vec_pretty(
                &json!({"passed":error.is_none(),"error":error,"checks":self.checks,"skipped":self.skipped,"performance":self.perf}),
            )?,
        )
    }
}
