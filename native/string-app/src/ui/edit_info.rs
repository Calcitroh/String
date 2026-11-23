//! Edit info: String's own names and cover for a song. The file isn't touched.
//!
//! A song menu calls [`open`] or [`swap`]; the shell calls [`show`] once a frame.
use super::{
    covers::Size,
    panel::{self, Btn, Kind},
};
use crate::{
    model::CoverSource,
    services::{self, Event, Task},
    theme::*,
    view::View,
};
use egui::{Align, Context, Key, Label, Layout, Sense, vec2};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex},
};
use string_core::{library::Track, state::State as Saved};
use string_extras::edit::{Info, display_names, edit_info, saved_info, swap_names};

const PICTURES: &str = "Pictures|*.jpg;*.jpeg;*.png;*.webp;*.gif;*.bmp";
struct Form {
    id: String,
    info: Info,
    /// What each empty box falls back to: the song's own tags.
    own: Info,
    folder: PathBuf,
    note: String,
    fresh: bool,
}
#[derive(Default)]
struct State {
    form: Option<Form>,
    picking: Option<Task<Result<Vec<PathBuf>, String>>>,
}
static STATE: LazyLock<Mutex<State>> = LazyLock::new(Default::default);
fn track<'a>(v: &'a View<'_>, id: &str) -> Option<&'a Track> {
    v.s.library.tracks.iter().find(|t| t.id() == id)
}
pub fn is_open() -> bool {
    STATE.lock().unwrap().form.is_some()
}
pub fn close() {
    STATE.lock().unwrap().form = None;
}
/// Open the dialog for one song.
pub fn open(v: &View<'_>, id: &str) {
    let Some(t) = track(v, id) else { return };
    let mut own = display_names(&Saved::default(), t);
    if own.artist.is_empty() {
        own.artist = "Unknown artist".into();
    }
    if own.album.is_empty() {
        own.album = "No album".into();
    }
    let note = services::get()
        .edit_note(id)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "These edits are saved in String.".into());
    STATE.lock().unwrap().form = Some(Form {
        id: id.into(),
        info: saved_info(&v.s.state, id),
        own,
        folder: Path::new(&t.path)
            .parent()
            .map(Path::to_owned)
            .unwrap_or_default(),
        note,
        fresh: true,
    });
}
pub(crate) fn save_info(s: &mut crate::playback::Session, id: &str, info: &Info) {
    edit_info(&mut s.state, id, info);
    s.rebuild();
}
fn changed(v: &mut View<'_>, id: &str) {
    v.s.state_dirty = true;
    v.s.queue_changed(v.player);
    services::get().event(Event::Edited(id));
}
/// A song named the wrong way round ("Title - Artist" read as "Artist - Title"): one click, an edit of the
/// person's own.
pub fn swap(v: &mut View<'_>, id: &str) {
    let Some(t) = track(v, id).cloned() else {
        return;
    };
    swap_names(&mut v.s.state, &t);
    let now = display_names(&v.s.state, &t);
    v.views
        .toast
        .set(format!("Now “{}” by {}", now.title, now.artist));
    v.s.rebuild();
    changed(v, id);
}
/// Draw the dialog while it is open, and pass on what worker threads had to say.
pub fn show(ctx: &Context, v: &mut View<'_>) {
    for message in services::take_notices() {
        v.views.toast.set(message);
    }
    let mut st = STATE.lock().unwrap();
    let st = &mut *st;
    if let Some(answer) = st.picking.as_ref().and_then(Task::poll) {
        st.picking = None;
        match answer {
            Ok(list) => {
                if let (Some(form), Some(p)) = (&mut st.form, list.first()) {
                    form.info.art = p.to_string_lossy().into_owned();
                }
            }
            Err(e) => v.views.toast.set(e),
        }
    }
    let Some(form) = &mut st.form else { return };
    let Some(song) = v.s.models.by_id.get(&form.id) else {
        st.form = None;
        return;
    };
    let tint = v.tint;
    let cover = Arc::new(CoverSource {
        file: form.info.art.clone(),
        ..(*song.cover).clone()
    });
    let art = v.covers.get(&cover, Size::Small, v.mono.then_some(tint));
    let pickers = services::get().pickers();
    let (mut save, mut close) = (false, false);
    // The key press that opened the dialog must not also save it.
    let fresh = form.fresh;
    let (_, dismissed) = panel::dialog(ctx, "edit info", "Edit info", tint, |ui| {
        let w = ui.available_width();
        let own = &form.own;
        for (label, text, hint) in [
            ("Title", &mut form.info.title, &own.title),
            ("Artist", &mut form.info.artist, &own.artist),
            ("Album", &mut form.info.album, &own.album),
        ] {
            ui.add_space(4.);
            panel::lbl(ui, label);
            ui.add_space(1.);
            let resp = panel::field(ui, text, hint, "UI", w);
            if label == "Title" && std::mem::take(&mut form.fresh) {
                resp.request_focus();
            }
        }
        ui.add_space(8.);
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(64., 64.), Sense::hover());
            super::sleeve(ui.painter(), r, art);
            ui.add_space(6.);
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    if pickers
                        && Btn::new("Choose a cover", tint)
                            .icon("image")
                            .sm()
                            .enabled(st.picking.is_none())
                            .show(ui)
                            .clicked()
                    {
                        let start = form.folder.clone();
                        st.picking = Some(Task::run(ui.ctx(), move |s| {
                            s.pick_files("Choose a cover picture", PICTURES, Some(&start), false)
                                .map_err(|e| e.to_string())
                        }));
                    }
                    if Btn::new("Use the song's own", tint)
                        .kind(Kind::Ghost)
                        .sm()
                        .show(ui)
                        .clicked()
                    {
                        form.info.art.clear();
                    }
                });
                if pickers {
                    let name = Path::new(&form.info.art)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    ui.label(mono(&name, 11.).color(FAINT));
                } else {
                    panel::field(
                        ui,
                        &mut form.info.art,
                        "A cover picture's path",
                        "Mono",
                        w - 78.,
                    );
                }
            });
        });
        ui.add_space(8.);
        ui.add(
            Label::new(
                mono(
                    &format!("{} Leave a box empty to use the song's own tag.", form.note),
                    11.,
                )
                .color(FAINT),
            )
            .wrap(),
        );
        ui.add_space(12.);
        ui.horizontal(|ui| {
            if Btn::new("Reset all", tint)
                .kind(Kind::Ghost)
                .show(ui)
                .clicked()
            {
                form.info = Info::default();
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                save = Btn::new("Save", tint)
                    .kind(Kind::Primary)
                    .show(ui)
                    .clicked()
                    || (!fresh && ui.input(|i| i.key_pressed(Key::Enter)));
                close = Btn::new("Cancel", tint)
                    .kind(Kind::Ghost)
                    .show(ui)
                    .clicked();
            });
        });
    });
    if save {
        let form = st.form.take().unwrap();
        let mut info = form.info;
        info.art = info.art.trim().trim_matches('"').to_owned();
        save_info(v.s, &form.id, &info);
        changed(v, &form.id);
        v.views.toast.set("Saved");
    } else if close || dismissed {
        st.form = None;
    }
}
/// Smoke case: the dialog over the Songs page.
pub mod smoke {
    use super::*;
    pub fn prepare(v: &mut View<'_>) {
        if let Some(id) = v.s.models.songs.first().cloned() {
            open(v, &id);
        }
    }
    pub fn ready(v: &mut View<'_>) -> bool {
        is_open() || v.s.models.songs.is_empty()
    }
}
