use crate::{
    player_bar,
    route::Route,
    screens,
    theme::{look, *},
    ui::{self, area, child},
    view::{Overlay, View},
};
use egui::{Key, Modifiers, Rect, Ui, pos2, vec2};
pub fn shortcuts(ui: &Ui, v: &mut View<'_>) {
    let ctx = ui.ctx();
    let key = |m, k| ctx.input_mut(|i| i.consume_key(m, k));
    let no = Modifiers::NONE;
    if ui::menus::takes_keys(ctx) {
        return;
    }
    let typing = ctx.text_edit_focused();
    let back = ctx.input(|i| i.pointer.button_released(egui::PointerButton::Extra1))
        || (!typing && key(Modifiers::ALT, Key::ArrowLeft));
    let forward = ctx.input(|i| i.pointer.button_released(egui::PointerButton::Extra2))
        || (!typing && key(Modifiers::ALT, Key::ArrowRight));
    if back || forward {
        if back && v.views.history.has_back() {
            v.views.history.back();
            v.route_changed();
        } else if forward && v.views.history.has_forward() {
            v.views.history.forward();
            v.route_changed();
        }
        return;
    }
    if key(no, Key::Escape) {
        let search = egui::Id::unique("global search");
        if ctx.memory(|m| m.has_focus(search) || m.had_focus_last_frame(search)) {
            v.views.query.clear();
            ctx.memory_mut(|m| m.surrender_focus(search));
            if v.views.history.current == Route::Search && v.views.history.has_back() {
                v.views.history.back();
                v.route_changed();
            }
            return;
        }
        let filter = match v.views.history.current {
            Route::Songs => Some("Filter songs"),
            Route::Liked => Some("Filter liked songs"),
            _ => None,
        };
        if !v.views.filter.is_empty()
            && filter.is_some_and(|hint| {
                ctx.memory(|m| m.had_focus_last_frame(egui::Id::unique(("library filter", hint))))
            })
        {
            v.views.filter.clear();
        }
        if v.views.adding_playlist {
            v.views.adding_playlist = false;
        } else if v.views.queue {
            v.views.queue = false;
        } else if v.views.overlay.take().is_none() {
            ctx.memory_mut(|m| m.stop_text_input());
        }
        return;
    }
    if v.views.adding_playlist || v.views.close_failed {
        return;
    }
    if typing {
        return;
    }
    if key(Modifiers::CTRL, Key::F) || key(no, Key::Slash) {
        v.views.search_focus = v.views.overlay.is_none();
        return;
    }
    if key(no, Key::Space) {
        v.s.toggle(v.player);
    } else if key(Modifiers::CTRL, Key::ArrowLeft) {
        v.s.previous(v.player);
    } else if key(Modifiers::CTRL, Key::ArrowRight) {
        v.s.next(v.player);
    } else if key(no, Key::ArrowLeft) {
        v.s.seek(v.player, v.st.position - 5.);
    } else if key(no, Key::ArrowRight) {
        v.s.seek(v.player, v.st.position + 5.);
    } else if key(Modifiers::CTRL, Key::ArrowUp) {
        v.volume(v.s.volume() + 0.05);
    } else if key(Modifiers::CTRL, Key::ArrowDown) {
        v.volume(v.s.volume() - 0.05);
    } else if key(no, Key::M) {
        v.mute();
    } else if key(no, Key::S) {
        v.shuffle();
    } else if key(no, Key::R) {
        v.repeat();
    } else if key(no, Key::L) {
        v.like_current();
    } else if key(no, Key::Q) {
        v.views.queue = !v.views.queue;
    } else if key(no, Key::F) {
        v.record(Overlay::Record);
    } else if key(no, Key::Y) {
        v.record(Overlay::Lyrics);
    } else if key(no, Key::OpenBracket) {
        v.s.set_loop(v.player, Some((true, v.st.position)));
    } else if key(no, Key::CloseBracket) {
        v.s.set_loop(v.player, Some((false, v.st.position)));
    } else if key(no, Key::Backslash) {
        v.s.set_loop(v.player, None);
    }
}
pub fn show(ui: &mut Ui, v: &mut View<'_>) {
    // the person's own look.css: "Use my look.css" is on unless turned off, as in 1.x
    look::frame(ui.ctx(), v.s.setting("look").as_bool().unwrap_or(true));
    ui::set_tint(ui.ctx(), v.tint);
    screens::dupes::settle(v);
    let r = ui.max_rect();
    let main = Rect::from_min_max(r.min, pos2(r.right(), r.bottom() - BAR));
    if v.views.overlay.is_some() {
        screens::now_playing::show(&mut child(ui, main, "np"), v);
    } else {
        let side = if r.width() < 950. { 188. } else { SIDE };
        let side_rect = area(r.left(), r.top(), side, main.height());
        sidebar(&mut child(ui, side_rect, "sidebar"), v);
        let body = area(
            side_rect.right(),
            main.top(),
            main.width() - side,
            main.height(),
        );
        // the look's picture behind the pages (1.x's #main)
        look::paint_main(ui.painter(), body);
        screens::detail::backdrop(ui, v, body);
        topbar(
            &mut child(
                ui,
                area(body.left(), body.top(), body.width(), 58.),
                "topbar",
            ),
            v,
        );
        let content = area(
            body.left() + 29.,
            body.top() + 72.,
            body.width() - 58.,
            body.height() - 83.,
        );
        screens::show(
            &mut child(ui, content, ("screen", v.views.history.current.path())),
            v,
        );
    }
    player_bar::show(
        &mut child(ui, area(r.left(), main.bottom(), r.width(), BAR), "player"),
        v,
    );
    if v.views.queue {
        ui::queue_drawer::show(ui, v, main);
    }
    ui::edit_info::show(ui.ctx(), v);
    ui::credit::show(ui.ctx(), v);
    ui::menus::overlay(ui, v);
    v.views.toast.show(ui.ctx());
}
fn sidebar(ui: &mut Ui, v: &mut View<'_>) {
    let r = ui.max_rect();
    let p = ui.painter().clone();
    let t = v.tint;
    // rgb(4 4 5) with a faint light from the top, as the web version's sidebar
    p.rect_filled(r, 0, DEEP);
    ui::gradient(
        &p,
        r.with_max_y(r.top() + 220.),
        white(0.02),
        egui::Color32::TRANSPARENT,
        false,
    );
    p.vline(r.right() - 1., r.y_range(), (1., HAIR));
    // the logo box (1.x's .brand): the person's own picture from their look, if any, then String's logo unless the
    // look hides it: a glowing wire between two ringed pegs, then S T R I N G with a soft halo
    let brand = look::brand();
    look::paint_brand(
        &p,
        area(r.left(), r.top(), r.width(), brand.height),
        brand.pad,
    );
    let x = r.left() + brand.pad[3];
    let y = r.top() + brand.pad[0] + (brand.height - brand.pad[0] - brand.pad[2]) / 2.;
    if brand.wire {
        let (a, b) = (pos2(x + 4., y), pos2(x + 30., y));
        ui::glow(
            &p,
            Rect::from_min_max(a, b).expand2(vec2(0., 1.)),
            1.,
            6.,
            t.ink(0.25),
        );
        if v.st.playing {
            // The string plucks while music plays, as in the web version (±5.5 px, 0.11 s each way).
            let time = ui.input(|i| i.time);
            let bend = -5.5 * (std::f64::consts::PI * time / 0.11).cos() as f32;
            p.add(egui::epaint::QuadraticBezierShape::from_points_stroke(
                [a, pos2(x + 17., y + bend), b],
                false,
                egui::Color32::TRANSPARENT,
                egui::Stroke::new(1.6, t.ink(1.)),
            ));
            ui.ctx().request_repaint();
        } else {
            p.line_segment([a, b], (1.6, t.ink(1.)));
        }
        for peg in [a, b] {
            p.circle(peg, 2.4, BG, (1.4, t.ink(0.8)));
        }
    }
    if brand.name {
        ui::glowing(
            &p,
            area(x + 46., y - 10., 150., 20.),
            "STRING",
            "HUD",
            15.,
            5.7,
            TEXT,
            egui::Align2::LEFT_CENTER,
            (20., 0.4, t),
        );
    }
    // the page list starts below the logo box: 62 px in String's own look
    let top = r.top() + brand.height;
    let today =
        v.s.listening
            .days()
            .get(&chrono::Local::now().date_naive())
            .map_or(0., |d| d.s);
    let artists = v.s.models.catalog.artists.iter();
    let nav = [
        (Route::Home, "home", String::new()),
        (Route::Songs, "music", v.s.models.songs.len().to_string()),
        (
            Route::Albums,
            "grid",
            v.s.models.catalog.albums.len().to_string(),
        ),
        (
            Route::Artists,
            "user",
            artists.filter(|g| !g.key.is_empty()).count().to_string(),
        ),
        (Route::Liked, "heart", v.s.models.liked.len().to_string()),
        (Route::Stats, "chart", short_time(today)),
    ];
    // rows 38 px tall, 1 px apart; the one open lit by a tint fading to the right and a glowing edge
    for (idx, (route, icon, count)) in nav.into_iter().enumerate() {
        let row = area(
            r.left() + 10.,
            top + idx as f32 * 39.,
            r.width() - 21.,
            38.,
        );
        let res = ui::hit(ui, row, route.path(), route.title());
        let on = v.views.history.current == route;
        if on {
            ui::gradient(&p, row, t.ink(0.12), egui::Color32::TRANSPARENT, true);
            let edge = Rect::from_min_max(
                pos2(row.left(), row.top() + 9.),
                pos2(row.left() + 2., row.bottom() - 9.),
            );
            ui::glow(&p, edge, 1., 10., t.ink(0.5));
            p.rect_filled(edge, 0, t.ink(1.));
        } else if res.hovered() {
            p.rect_filled(row, 0, white(0.035));
        }
        let color = if on {
            t.ink(1.)
        } else if res.hovered() {
            TEXT
        } else {
            MUTED
        };
        ui::glyph::draw(
            &p,
            area(row.left() + 12., row.center().y - 8., 16., 16.),
            icon,
            color,
        );
        ui::spaced(
            &p,
            area(row.left() + 40., row.top(), 120., 38.),
            &route.title().to_uppercase(),
            "HUD",
            11.5,
            1.84,
            color,
            egui::Align2::LEFT_CENTER,
        );
        ui::spaced(
            &p,
            area(row.right() - 72., row.top(), 60., 38.),
            &count,
            "Mono",
            10.5,
            0.,
            FAINT,
            egui::Align2::RIGHT_CENTER,
        );
        if res.clicked() {
            v.go(route);
        }
    }
    ui::side_playlists::show(
        ui,
        v,
        Rect::from_min_max(
            pos2(r.left(), (top + 254.).min(r.bottom() - 40.)),
            pos2(r.right(), r.bottom() - 40.),
        ),
    );
    p.hline(r.x_range(), r.bottom() - 40., (1., HAIR));
    ui::label(
        &p,
        area(x, r.bottom() - 32., r.width() - 40., 18.),
        &format!(
            "{} · {}",
            screens::library::count(v.s.models.songs.len(), "song"),
            screens::library::duration(v.s.models.duration)
        ),
        "Mono",
        10.5,
        FAINT,
    );
}
/// Today's listening for the sidebar, as the web version writes it: "1H 30M", "45M", "<1M".
fn short_time(s: f64) -> String {
    let s = s.round() as u64;
    let (h, m) = (s / 3600, s % 3600 / 60);
    match (s, h, m) {
        (0, ..) => String::new(),
        (1..60, ..) => "<1M".into(),
        (_, 0, m) => format!("{m}M"),
        (_, h, 0) => format!("{h}H"),
        (_, h, m) => format!("{h}H {m}M"),
    }
}
fn topbar(ui: &mut Ui, v: &mut View<'_>) {
    let r = ui.max_rect();
    let y = r.top() + 30.;
    let x = r.left() + 22.;
    // as the web version's top bar: round back and forward buttons on a dark disc (faded while there's nowhere to
    // go), then a 380 px search field, nearly black
    for (i, icon, name, on) in [
        (0, "back", "Back", v.views.history.has_back()),
        (1, "fwd", "Forward", v.views.history.has_forward()),
    ] {
        let b = area(x + i as f32 * 40., y - 16., 32., 32.);
        ui.painter()
            .circle_filled(b.center(), 16., egui::Color32::from_black_alpha(115));
        let res = ui::round_btn(ui, b, icon, name, false, false, v.tint);
        if !on {
            ui.painter().circle_filled(
                b.center(),
                16.,
                egui::Color32::from_rgba_premultiplied(4, 4, 5, 170),
            );
        }
        if res.clicked() && on {
            if i == 0 {
                v.views.history.back()
            } else {
                v.views.history.forward()
            }
            v.route_changed();
        }
    }
    let rect = area(
        x + 86.,
        y - 19.,
        (r.width() - 86. - 22. - 120.).clamp(175., 380.),
        38.,
    );
    ui.painter().rect(
        rect,
        19,
        egui::Color32::from_rgb(2, 2, 3),
        (1., HAIR2),
        egui::StrokeKind::Inside,
    );
    ui::glyph::draw(
        ui.painter(),
        area(rect.left() + 11., y - 8., 16., 16.),
        "search",
        MUTED,
    );
    let edit = ui.put(
        rect.shrink2(vec2(12., 4.)).with_min_x(rect.left() + 35.),
        egui::TextEdit::singleline(&mut v.views.query)
            .id(egui::Id::unique("global search"))
            .hint_text("Search songs, albums, artists")
            .frame(egui::Frame::NONE),
    );
    if std::mem::take(&mut v.views.search_focus) {
        edit.request_focus();
        if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), edit.id) {
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::select_all_str(
                    &v.views.query,
                )));
            egui::TextEdit::store_state(ui.ctx(), edit.id, state);
        }
    }
    if edit.changed() && v.views.history.current != Route::Search {
        v.go(Route::Search);
    }
    if edit.lost_focus()
        && ui.input(|i| i.key_pressed(Key::Enter))
        && v.views.history.current == Route::Search
    {
        screens::search::enter(ui, v);
        v.views.selection = Default::default();
    }
    if let Some(status) = v.scan.status() {
        ui::label(
            ui.painter(),
            area(
                rect.right() + 14.,
                y - 10.,
                r.right() - rect.right() - 75.,
                20.,
            ),
            &status,
            "Mono",
            11.,
            MUTED,
        );
    }
    if ui::round_btn(
        ui,
        area(r.right() - 54., y - 16., 32., 32.),
        "gear",
        "Library settings",
        v.views.history.current == Route::Settings,
        false,
        v.tint,
    )
    .clicked()
    {
        v.go(Route::Settings);
    }
}
