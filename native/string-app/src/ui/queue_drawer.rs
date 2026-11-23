//! The queue drawer (Q): what plays next and what played lately, and the queue button that opens it.
use crate::{
    model::Ids,
    route::Route,
    theme::*,
    ui::{
        self, area,
        covers::Size,
        glyph,
        menus::{self, Origin, caps},
    },
    view::View,
};
use egui::{Align2, Area, Color32, Id, Order, Pos2, Rect, Sense, StrokeKind, Ui, Vec2, pos2, vec2};
use serde_json::json;
use std::sync::Mutex;

struct State {
    recent: bool,
    /// The queue place being dragged.
    drag: Option<usize>,
}
static STATE: Mutex<State> = Mutex::new(State {
    recent: false,
    drag: None,
});
/// Show the Recent tab (true) or the Queue tab.
pub fn set_tab(recent: bool) {
    STATE.lock().unwrap().recent = recent;
}

/// The player bar's queue button. Songs dragged onto it join the queue.
pub fn button(ui: &mut Ui, v: &mut View<'_>, r: Rect) {
    let res = ui::round_btn(ui, r, "queue", "Queue", v.views.queue, false, v.tint);
    if res.clicked() {
        v.views.queue = !v.views.queue;
    }
    if res.dnd_hover_payload::<Ids>().is_some() {
        let p = ui.painter();
        for i in 1..=5 {
            p.rect_stroke(
                r.expand(i as f32 * 1.6),
                3,
                (1.6, v.tint.ink(0.07 * (1. - i as f32 / 6.))),
                StrokeKind::Middle,
            );
        }
        p.rect(
            r,
            3,
            v.tint.ink(0.18),
            (1., v.tint.ink(0.7)),
            StrokeKind::Inside,
        );
        glyph::draw(
            p,
            Rect::from_center_size(r.center(), Vec2::splat(17.)),
            "queue",
            v.tint.ink(1.),
        );
    }
    if let Some(ids) = res.dnd_release_payload::<Ids>() {
        menus::queue_tracks(v, &ids[..]);
    }
}

/// This run's songs that really started, then the songs played most lately; hidden ones left out.
pub fn recent_ids(v: &View<'_>) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    let ok = |id: &str| v.s.models.by_id.contains_key(id) && !v.s.state.hidden(id);
    for id in v.s.state.queue.history() {
        if ok(id) && !out.contains(id) {
            out.push(id.clone());
        }
    }
    let mut played: Vec<(i64, &String)> =
        v.s.field("plays")
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(id, _)| ok(id))
            .map(|(id, p)| (p[1].as_f64().unwrap_or(0.) as i64, id))
            .collect();
    played.sort_by_key(|p| std::cmp::Reverse(p.0));
    for (_, id) in played {
        if out.len() >= 60 {
            break;
        }
        if !out.contains(id) {
            out.push(id.clone());
        }
    }
    out
}

enum Row {
    Section(&'static str, Option<(Route, String, String)>),
    /// A queue place (or none on the Recent tab), the song and whether it is the one playing.
    Song(Option<usize>, String, bool),
    Empty(&'static str),
}
enum Act {
    Jump(usize),
    Remove(usize),
    Move(usize, usize),
    PlayRecent(usize),
    Menu(String, Pos2),
    Go(Route, String),
}
fn rows(v: &View<'_>, recent: bool) -> Vec<Row> {
    let mut out = vec![];
    let now = v.s.state.queue.current();
    if recent {
        let ids = recent_ids(v);
        if ids.is_empty() {
            out.push(Row::Empty("Songs you play show up here"));
        } else {
            out.push(Row::Section("Recently played", None));
        }
        for id in ids {
            let cur = now == Some(id.as_str());
            out.push(Row::Song(None, id, cur));
        }
        return out;
    }
    let q = &v.s.state.queue;
    let known = |id: &str| v.s.models.catalog.tracks.get(id).is_some_and(|s| !s.hidden);
    let at = q.at();
    if let (Some(i), Some(id)) = (at, now)
        && v.s.models.by_id.contains_key(id)
    {
        out.push(Row::Section("Now playing", None));
        out.push(Row::Song(Some(i), id.to_owned(), true));
    }
    let first = at.map_or(0, |i| i + 1);
    let upcoming = || {
        q.upcoming()
            .iter()
            .enumerate()
            .filter(|(_, x)| known(&x.id))
            .map(move |(k, x)| (first + k, x))
    };
    let mut any = false;
    for (i, x) in upcoming().filter(|(_, x)| x.manual) {
        if !any {
            out.push(Row::Section("Next in queue", None));
            any = true;
        }
        out.push(Row::Song(Some(i), x.id.clone(), false));
    }
    let mut rest = upcoming().filter(|(_, x)| !x.manual).take(120).peekable();
    if let Some((i, x)) = rest.peek() {
        let from = q.context(Some(*i));
        let name = menus::ctx_name(&v.s.models, from);
        out.push(Row::Section(
            "Next from",
            Some((
                menus::ctx_route(from).unwrap_or_default(),
                if name.is_empty() {
                    "the queue".into()
                } else {
                    name
                },
                if menus::ctx_route(from).is_some() {
                    x.id.clone()
                } else {
                    String::new()
                },
            )),
        ));
        any = true;
    }
    for (i, x) in rest {
        out.push(Row::Song(Some(i), x.id.clone(), false));
    }
    if out.is_empty() && !any {
        out.push(Row::Empty(
            "The queue is empty. Use “Add to queue” on any song.",
        ));
    }
    out
}
fn seg(ui: &mut Ui, at: Pos2, recent: bool, hi: Tint) -> Option<bool> {
    let mut picked = None;
    let p = ui.painter().clone();
    let mut x = at.x + 2.;
    let mut cells = vec![];
    for (name, value) in [("Queue", false), ("Recent", true)] {
        let w = p
            .layout_no_wrap(name.to_uppercase(), font("HUD", 10.5), TEXT)
            .size()
            .x
            + name.len() as f32 * 1.26
            + 20.;
        cells.push((name, value, area(x, at.y + 2., w, 24.)));
        x += w;
    }
    p.rect(
        Rect::from_min_max(at, pos2(x + 2., at.y + 28.)),
        0,
        Color32::from_black_alpha(90),
        (1., HAIR2),
        StrokeKind::Inside,
    );
    for (name, value, r) in cells {
        let res = ui::hit(ui, r, ("queue tab", name), name);
        let on = value == recent;
        if on {
            p.rect(r, 0, hi.ink(0.14), (1., hi.ink(0.35)), StrokeKind::Inside);
        }
        caps(
            &p,
            r.center(),
            Align2::CENTER_CENTER,
            name,
            "HUD",
            10.5,
            0.12,
            if on {
                hi.ink(1.)
            } else if res.hovered() {
                TEXT
            } else {
                MUTED
            },
        );
        if res.clicked() && !on {
            picked = Some(value);
        }
    }
    picked
}
pub fn show(ui: &mut Ui, v: &mut View<'_>, main: Rect) {
    let ctx = ui.ctx().clone();
    let w = 380_f32.min(main.width() * 0.92);
    let rect = area(main.right() - w, main.top(), w, main.height());
    let (mut recent, mut drag) = {
        let s = STATE.lock().unwrap();
        (s.recent, s.drag)
    };
    let list = rows(v, recent);
    let hi = v.tint;
    let at = v.s.state.queue.at();
    let time = ctx.input(|i| i.time);
    let mut act = None;
    let mut clear = false;
    let mut close = false;
    let mut target = None;
    Area::new(Id::unique("queue drawer"))
        .order(Order::Foreground)
        .fixed_pos(rect.min)
        .show(&ctx, |ui| {
            let (r, _) = ui.allocate_exact_size(rect.size(), Sense::click());
            let p = ui.painter().clone();
            for i in 0..12 {
                let x = r.left() - 60. + i as f32 * 5.;
                p.rect_filled(
                    area(x, r.top(), 5., r.height()),
                    0,
                    Color32::from_black_alpha((i * i) as u8 / 2 + 2),
                );
            }
            p.rect_filled(r, 0, Color32::from_rgba_unmultiplied(5, 5, 7, 250));
            p.vline(r.left(), r.y_range(), (1., HAIR2));
            if let Some(tab) = seg(ui, r.min + vec2(20., 18.), recent, hi) {
                recent = tab;
                drag = None;
            }
            let cy = r.top() + 32.;
            let x = area(r.right() - 48., cy - 16., 32., 32.);
            if ui::icon_btn(ui, x, "x", "Close", false, hi).clicked() {
                close = true;
            }
            let manual = v.s.state.queue.upcoming().iter().any(|x| x.manual);
            if manual && !recent {
                let b = area(x.left() - 72., cy - 14., 62., 28.);
                let res = ui::hit(ui, b, "clear queue", "Clear");
                if res.hovered() {
                    p.rect(
                        b,
                        0,
                        Color32::from_white_alpha(10),
                        (1., HAIR2),
                        StrokeKind::Inside,
                    );
                }
                caps(
                    &p,
                    b.center(),
                    Align2::CENTER_CENTER,
                    "Clear",
                    "HUD",
                    10.5,
                    0.12,
                    if res.hovered() { TEXT } else { MUTED },
                );
                clear = res.clicked();
            }
            let body = Rect::from_min_max(
                pos2(r.left() + 10., r.top() + 58.),
                pos2(r.right() - 10., r.bottom()),
            );
            let mut inner = ui::child(ui, body, "queue body");
            inner.spacing_mut().item_spacing = Vec2::ZERO;
            egui::ScrollArea::vertical()
                .id_salt(recent)
                .auto_shrink([false, false])
                .show(&mut inner, |ui| {
                    let w = ui.available_width();
                    for (n, row) in list.iter().enumerate() {
                        match row {
                            Row::Empty(text) => {
                                ui.add_space(20.);
                                let (space, _) =
                                    ui.allocate_exact_size(vec2(w, 0.), Sense::hover());
                                let mut note = ui::child(
                                    ui,
                                    area(space.left() + 10., space.top(), w - 20., 400.),
                                    "empty",
                                );
                                let h = menus::empty_note(&mut note, text);
                                ui.add_space(h);
                            }
                            Row::Section(label, link) => {
                                let (r, _) = ui.allocate_exact_size(vec2(w, 35.), Sense::hover());
                                let at = pos2(r.left() + 10., r.top() + 21.);
                                let p = ui.painter().clone();
                                let done = caps(
                                    &p,
                                    at,
                                    Align2::LEFT_CENTER,
                                    label,
                                    "HUD",
                                    10.5,
                                    0.2,
                                    FAINT,
                                );
                                let Some((route, name, focus)) = link else {
                                    continue;
                                };
                                let at = pos2(done.right() + 7., at.y);
                                let clip = p.with_clip_rect(r.intersect(p.clip_rect()));
                                let nr = caps(
                                    &clip,
                                    at,
                                    Align2::LEFT_CENTER,
                                    name,
                                    "HUD",
                                    10.5,
                                    0.2,
                                    MUTED,
                                )
                                .intersect(r);
                                if focus.is_empty() {
                                    continue;
                                }
                                let res = ui::hit(ui, nr, "queue source", name);
                                if res.hovered() {
                                    caps(
                                        &clip,
                                        at,
                                        Align2::LEFT_CENTER,
                                        name,
                                        "HUD",
                                        10.5,
                                        0.2,
                                        TEXT,
                                    );
                                    clip.hline(nr.x_range(), nr.bottom() + 2., (1., hi.ink(0.5)));
                                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                }
                                if res.clicked() {
                                    act = Some(Act::Go(route.clone(), focus.clone()));
                                }
                            }
                            Row::Song(place, id, cur) => {
                                let Some(song) = v.s.models.by_id.get(id) else {
                                    continue;
                                };
                                let movable = place.is_some() && !cur;
                                let (r, res) = ui.allocate_exact_size(
                                    vec2(w, 50.),
                                    if movable {
                                        Sense::click_and_drag()
                                    } else {
                                        Sense::click()
                                    },
                                );
                                if !ui.is_rect_visible(r) {
                                    continue;
                                }
                                let over = ui.rect_contains_pointer(r);
                                let dragged = drag.is_some() && drag == *place;
                                let mut p = ui.painter().clone();
                                if dragged {
                                    p.set_opacity(0.35);
                                } else if over && drag.is_none() {
                                    p.rect_filled(r, 0, Color32::from_white_alpha(11));
                                }
                                ui::sleeve(
                                    &p,
                                    area(r.left() + 10., r.center().y - 18., 36., 36.),
                                    v.covers.get(&song.cover, Size::Small, v.mono.then_some(hi)),
                                );
                                let x = r.left() + 57.;
                                let end = r.right() - 46.;
                                ui::label(
                                    &p,
                                    area(x, r.top() + 8., end - x, 18.),
                                    &song.title,
                                    "Strong",
                                    13.5,
                                    if *cur { hi.ink(1.) } else { TEXT },
                                );
                                ui::label(
                                    &p,
                                    area(x, r.top() + 26., end - x, 16.),
                                    song.artist_name(),
                                    "UI",
                                    12.,
                                    FAINT,
                                );
                                let side = area(r.right() - 38., r.center().y - 14., 28., 28.);
                                if *cur {
                                    if v.st.playing {
                                        menus::eq_bars(&p, side, true, time, hi);
                                    }
                                } else if let Some(i) = *place
                                    && over
                                    && drag.is_none()
                                {
                                    let x = ui::hit(ui, side, ("remove", n), "Remove");
                                    if x.hovered() {
                                        p.rect(
                                            side,
                                            0,
                                            Color32::from_white_alpha(13),
                                            (1., HAIR),
                                            StrokeKind::Inside,
                                        );
                                    }
                                    glyph::draw(
                                        &p,
                                        Rect::from_center_size(side.center(), Vec2::splat(15.)),
                                        "x",
                                        if x.hovered() { TEXT } else { MUTED },
                                    );
                                    if x.on_hover_text("Remove").clicked() {
                                        act = Some(Act::Remove(i));
                                    }
                                }
                                if let Some(i) = *place {
                                    if res.drag_started() && movable {
                                        drag = Some(i);
                                    }
                                    if drag.is_some() && over && at.is_some_and(|a| i > a) {
                                        p.hline(r.x_range(), r.top() + 1., (2., hi.ink(1.)));
                                        target = Some(i);
                                    }
                                    if res.double_clicked() {
                                        act = Some(Act::Jump(i));
                                    }
                                } else if res.double_clicked() {
                                    act = Some(Act::PlayRecent(n - 1));
                                }
                                if res.secondary_clicked() {
                                    act = Some(Act::Menu(
                                        id.clone(),
                                        res.interact_pointer_pos().unwrap_or(r.center()),
                                    ));
                                }
                            }
                        }
                    }
                    ui.add_space(20.);
                });
        });
    if let Some(from) = drag {
        let (released, down, pos) = ctx.input(|i| {
            (
                i.pointer.any_released(),
                i.pointer.any_down(),
                i.pointer.latest_pos(),
            )
        });
        if let (Some(pos), Some(song)) = (
            pos,
            v.s.state
                .queue
                .items()
                .get(from)
                .and_then(|x| v.s.models.by_id.get(&x.id)),
        ) {
            menus::ghost(&ctx, pos, song.title.clone(), hi);
        }
        if released {
            if let Some(to) = target
                && to != from
            {
                act = Some(Act::Move(from, to));
            }
            drag = None;
        } else if !down {
            drag = None;
        }
        ctx.request_repaint();
    }
    {
        let mut s = STATE.lock().unwrap();
        s.recent = recent;
        s.drag = drag;
    }
    if close {
        v.views.queue = false;
    }
    if clear {
        v.s.state.queue.clear_manual();
        v.s.queue_changed(v.player);
    }
    match act {
        Some(Act::Jump(i)) => v.jump(i),
        Some(Act::Remove(i)) => {
            v.s.state.queue.remove(i);
            v.s.queue_changed(v.player);
        }
        Some(Act::Move(from, to)) => {
            v.s.state.queue.move_before(from, to);
            v.s.queue_changed(v.player);
        }
        Some(Act::PlayRecent(i)) => {
            let ids = recent_ids(v);
            v.s.play_ids(
                v.player,
                &ids,
                i as isize,
                json!({"kind":"recent","key":"lately","name":"Recently played"}),
            );
        }
        Some(Act::Menu(id, at)) => menus::track_menu(vec![id], at, Origin::none()),
        Some(Act::Go(route, id)) => {
            v.go(route);
            v.views.reveal_id = Some(id);
        }
        None => {}
    }
}
