//! The sidebar's playlists: the + button, the helpers' buttons left of it, each playlist with its cover, and songs
//! dropped onto one.
use crate::{
    integration::helpers::Started,
    model::{CoverSource, Ids, Models},
    route::Route,
    theme::*,
    ui::{
        self, area,
        covers::Size,
        menus::{self, caps},
    },
    view::View,
};
use egui::{Align2, Color32, Painter, Rect, Sense, StrokeKind, Ui, Vec2, pos2, vec2};
use serde_json::json;
use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};

/// Smoke runs only: draw this playlist as if these songs were held over it.
static DEMO: Mutex<Option<(String, Ids)>> = Mutex::new(None);
pub fn demo_drop(target: Option<(String, Ids)>) {
    *DEMO.lock().unwrap() = target;
}

/// Up to four different covers among these songs; songs without a picture don't count.
pub fn cover_picks(m: &Models, ids: &[String]) -> Vec<Arc<CoverSource>> {
    let mut seen = HashSet::new();
    let mut out = vec![];
    for song in ids.iter().filter_map(|id| m.by_id.get(id)) {
        let c = &song.cover;
        if c.file.is_empty() && c.embedded.is_empty() && c.fallback.is_empty() {
            continue;
        }
        let key = if song.album_key.is_empty() {
            &song.id
        } else {
            &song.album_key
        };
        if seen.insert(key) {
            out.push(c.clone());
            if out.len() == 4 {
                break;
            }
        }
    }
    out
}
/// A playlist's cover: four covers in a square, one when it has fewer, a drawn one when it has none.
pub fn mosaic(p: &Painter, r: Rect, v: &mut View<'_>, ids: &[String], seed: &str, size: Size) {
    let picks = cover_picks(&v.s.models, ids);
    let tint = v.mono.then_some(v.tint);
    if picks.len() < 4 {
        let source = picks.into_iter().next().unwrap_or_else(|| {
            Arc::new(CoverSource {
                seed: seed.into(),
                ..Default::default()
            })
        });
        ui::sleeve(p, r, v.covers.get(&source, size, tint));
        return;
    }
    p.rect_filled(r, 1, PANEL2);
    let half = r.size() / 2.;
    for (i, source) in picks.iter().enumerate() {
        let cell = Rect::from_min_size(
            r.min + vec2((i % 2) as f32 * half.x, (i / 2) as f32 * half.y),
            half,
        );
        if let Some(art) = v.covers.get(source, Size::Small, tint) {
            ui::image(p, cell, &art.cover);
        }
    }
    p.rect_stroke(r, 1, (1., HAIR), StrokeKind::Inside);
}
fn fade_right(p: &Painter, r: Rect, color: Color32) {
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(r.left_top(), color);
    mesh.colored_vertex(r.right_top(), Color32::TRANSPARENT);
    mesh.colored_vertex(r.right_bottom(), Color32::TRANSPARENT);
    mesh.colored_vertex(r.left_bottom(), color);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    p.add(mesh);
}
/// The helper programs' small round buttons, left of `+`: as many as fit before the heading, the rest in a menu under
/// "more". A helper String started that still runs shows lit.
fn helper_buttons(ui: &mut Ui, v: &mut View<'_>, r: Rect, cy: f32, hi: Tint) {
    v.platform.helpers_focus(ui.input(|i| i.focused));
    let h = &v.platform.helpers;
    let helpers: Vec<(String, String, String, String, bool)> = h
        .list()
        .iter()
        .map(|x| {
            let open = h.is_open(&x.id);
            (
                x.id.clone(),
                x.name.clone(),
                x.title.clone(),
                x.glyph.clone(),
                open,
            )
        })
        .collect();
    v.platform.helpers.drawn.clear();
    if helpers.is_empty() {
        return;
    }
    const STEP: f32 = 28.;
    let plus = r.right() - 48.;
    let slots = (((plus - (r.left() + 100.)) / STEP).floor() as usize).max(1);
    let (shown, more) = if helpers.len() > slots {
        (slots - 1, true)
    } else {
        (helpers.len(), false)
    };
    let count = shown + usize::from(more);
    let mut clicked = None;
    for (i, (id, _, title, glyph, open)) in helpers.iter().take(shown).enumerate() {
        let at = area(plus - STEP * (count - i) as f32, cy - 13., 26., 26.);
        // a faint ring at rest, so it reads as a button of its own and not as the heading's
        ui.painter()
            .circle_stroke(at.center(), at.width() / 2. - 0.5, (1., HAIR2));
        let res = ui
            .push_id(("helper", id), |ui| {
                ui::round_btn(ui, at, glyph, title, *open, false, hi)
            })
            .inner;
        v.platform.helpers.drawn.push(id.clone());
        if res.clicked() {
            clicked = Some(id.clone());
        }
    }
    if more {
        let at = area(plus - STEP, cy - 13., 26., 26.);
        ui.painter()
            .circle_stroke(at.center(), at.width() / 2. - 0.5, (1., HAIR2));
        let res = ui::round_btn(ui, at, "more", "More helpers", false, false, hi);
        egui::Popup::menu(&res).show(|ui| {
            for (id, name, title, _, open) in &helpers[shown..] {
                let label = if *open {
                    format!("{name} (open)")
                } else {
                    name.clone()
                };
                if ui.button(label).on_hover_text(title).clicked() {
                    clicked = Some(id.clone());
                }
            }
        });
        let rest = helpers[shown..].iter().map(|x| x.0.clone());
        v.platform.helpers.drawn.extend(rest);
    }
    if let Some(id) = clicked {
        start_helper(v, &id);
    }
}
fn start_helper(v: &mut View<'_>, id: &str) {
    let name = v
        .platform
        .helpers
        .list()
        .iter()
        .find(|h| h.id == id)
        .map_or_else(|| id.to_owned(), |h| h.name.clone());
    match v.platform.helpers.start(id) {
        Ok(Started::Started) => v.views.toast.set(format!("Starting {name}…")),
        Ok(Started::AlreadyOpen(true)) => {}
        Ok(Started::AlreadyOpen(false)) => v.views.toast.set(format!("{name} is already open.")),
        Ok(Started::SelfTest) => v.views.toast.set("Helpers don’t start in a self-test."),
        Err(e) => {
            v.platform.log.write("add-ons", format!("{id}: {e}"));
            v.views.toast.set(format!("{name} couldn’t start: {e}"));
        }
    }
}
/// `r` runs from the "Playlists" heading down to the sidebar's foot.
pub fn show(ui: &mut Ui, v: &mut View<'_>, r: Rect) {
    let hi = v.tint;
    let p = ui.painter().clone();
    let cy = r.top() + 14.;
    caps(
        &p,
        pos2(r.left() + 22., cy),
        Align2::LEFT_CENTER,
        "Playlists",
        "HUD",
        10.5,
        0.2,
        FAINT,
    );
    helper_buttons(ui, v, r, cy, hi);
    if ui::icon_btn(
        ui,
        area(r.right() - 48., cy - 13., 26., 26.),
        "plus",
        "New playlist",
        false,
        hi,
    )
    .clicked()
    {
        menus::new_playlist(v, vec![], true);
    }
    let body = Rect::from_min_max(
        pos2(r.left() + 10., r.top() + 34.),
        pos2(r.right() - 10., r.bottom()),
    );
    let mut list = ui::child(ui, body, "playlists");
    list.spacing_mut().item_spacing = Vec2::ZERO;
    if v.s.models.playlists.is_empty() {
        let mut note = ui::child(&mut list, body.shrink2(vec2(12., 6.)), "note");
        note.add(
            egui::Label::new(mono("Make one with +, then drag songs onto it.", 10.5).color(FAINT))
                .wrap(),
        );
        return;
    }
    // How many entries each playlist really holds, songs no longer in the library included.
    let counts: Vec<(String, String, usize)> =
        v.s.models
            .playlists
            .iter()
            .map(|pl| {
                let n =
                    v.s.field("playlists")
                        .as_array()
                        .into_iter()
                        .flatten()
                        .find(|p| p["id"] == pl.id.as_str())
                        .and_then(|p| p["items"].as_array())
                        .map_or(pl.ids.len(), Vec::len);
                (pl.id.clone(), pl.name.clone(), n)
            })
            .collect();
    let from = v.s.state.queue.context(None);
    let playing = (v.s.current().is_some() && from["kind"] == "pl")
        .then(|| from["key"].as_str().unwrap_or("").to_owned());
    let demo = DEMO.lock().unwrap().clone();
    let time = ui.input(|i| i.time);
    let mut go = None;
    let mut menu = None;
    let mut dropped = None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(&mut list, |ui| {
            let w = ui.available_width();
            for (i, (id, name, count)) in counts.iter().enumerate() {
                let (row, res) = ui.allocate_exact_size(vec2(w, 46.), Sense::click());
                ui.add_space(1.);
                if !ui.is_rect_visible(row) {
                    continue;
                }
                res.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::Button, true, name.as_str())
                });
                let p = ui.painter().clone();
                let on = matches!(&v.views.history.current, Route::Playlist(k) if k == id);
                let held = demo.as_ref().filter(|d| &d.0 == id).map(|d| d.1.clone());
                let drop = res.dnd_hover_payload::<Ids>().is_some() || held.is_some();
                if drop {
                    p.rect(row, 0, hi.ink(0.14), (1., hi.ink(0.6)), StrokeKind::Inside);
                } else if on {
                    fade_right(&p, row, hi.ink(0.12));
                    p.vline(row.left() + 1., row.y_range().shrink(9.), (2., hi.ink(1.)));
                } else if res.hovered() {
                    p.rect_filled(row, 0, Color32::from_white_alpha(9));
                }
                let ids = v.s.models.playlists[i].ids.clone();
                mosaic(
                    &p,
                    area(row.left() + 12., row.center().y - 16., 32., 32.),
                    v,
                    &ids,
                    &format!("pl:{id}"),
                    Size::Small,
                );
                let x = row.left() + 54.;
                let live = playing.as_deref() == Some(id.as_str());
                let end = row.right() - if live { 34. } else { 8. };
                ui::label(
                    &p,
                    area(x, row.top() + 7., end - x, 18.),
                    name,
                    "UI",
                    13.,
                    if drop || res.hovered() {
                        TEXT
                    } else if on {
                        hi.ink(1.)
                    } else {
                        MUTED
                    },
                );
                ui::label(
                    &p,
                    area(x, row.top() + 25., end - x, 14.),
                    &menus::plural(*count, "song"),
                    "Mono",
                    10.5,
                    FAINT,
                );
                if live {
                    menus::eq_bars(
                        &p,
                        area(row.right() - 32., row.center().y - 10., 22., 20.),
                        v.st.playing,
                        time,
                        hi,
                    );
                }
                if let Some(ids) = held {
                    menus::ghost(
                        ui.ctx(),
                        pos2(row.right() - 60., row.center().y - 6.),
                        menus::drag_text(&v.s.models, &ids),
                        hi,
                    );
                }
                if res.clicked() {
                    go = Some(id.clone());
                }
                if res.secondary_clicked() {
                    menu = Some((
                        id.clone(),
                        res.interact_pointer_pos().unwrap_or(row.center()),
                    ));
                }
                if let Some(ids) = res.dnd_release_payload::<Ids>() {
                    dropped = Some((id.clone(), (*ids).clone()));
                }
            }
            ui.add_space(14.);
        });
    if let Some(id) = go {
        v.go(Route::Playlist(id));
    }
    if let Some((id, at)) = menu {
        menus::ctx_menu(json!({"kind":"pl","key":id}), at, None);
    }
    if let Some((id, ids)) = dropped {
        drop_songs(v, &id, &ids);
    }
}

pub(crate) fn drop_songs(v: &mut View<'_>, playlist: &str, ids: &[String]) {
    menus::add_to_playlist(v, playlist, ids);
}
