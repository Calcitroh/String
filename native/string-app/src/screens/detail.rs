use super::{
    cards::{self, Art},
    library::{self, clip_label},
};
use crate::{
    model::Ids,
    route::Route,
    theme::*,
    ui::{self, area, covers::Size},
    view::View,
};
use egui::{Color32, Id, Mesh, Rect, Ui, Vec2, pos2, vec2};

pub fn filter(ui: &mut Ui, v: &mut View<'_>, field: Rect, hint: &str) {
    let clear = !v.views.filter.is_empty();
    ui.painter()
        .rect(field, 16, BG, (1., HAIR2), egui::StrokeKind::Inside);
    ui::glyph::draw(
        ui.painter(),
        area(field.left() + 10., field.center().y - 7., 14., 14.),
        "search",
        FAINT,
    );
    let id = Id::unique(("library filter", hint));
    let r = field
        .shrink2(vec2(5., 4.))
        .with_min_x(field.left() + 31.)
        .with_max_x(field.right() - if clear { 28. } else { 7. });
    let resp = ui.place(
        r,
        egui::TextEdit::singleline(&mut v.views.filter)
            .id(id)
            .font(font("UI", 12.5))
            .hint_text(hint)
            .frame(egui::Frame::NONE),
    );
    if resp.changed() {
        v.views.selection = Default::default();
    }
    if clear
        && ui::icon_btn(
            ui,
            area(field.right() - 27., field.center().y - 12., 24., 24.),
            "x",
            "Clear filter",
            false,
            v.tint,
        )
        .clicked()
    {
        v.views.filter.clear();
        v.views.selection = Default::default();
        resp.request_focus();
    }
}
pub struct Hero<'a> {
    pub label: &'a str,
    pub title: &'a str,
    pub meta: &'a str,
    pub artist: Option<(&'a str, &'a str)>,
    pub art: &'a Art,
    pub round: bool,
}
pub(crate) fn backdrop(ui: &Ui, v: &mut View<'_>, body: Rect) {
    let route = &v.views.history.current;
    if !matches!(route, Route::Album(_) | Route::Artist(_)) {
        return;
    }
    let index = library::Index::get(ui, v);
    let Some(card) = index.page(route) else {
        return;
    };
    let Art::Cover(c) = &card.art else {
        return;
    };
    if c.file.is_empty() && c.embedded.is_empty() && c.fallback.is_empty() {
        return;
    }
    let Some(img) = v
        .covers
        .get(c, Size::Large, v.mono.then_some(v.tint))
        .and_then(|a| a.ambient)
    else {
        return;
    };
    let scroll = ui
        .data(|d| d.get_temp::<f32>(Id::unique(("hero offset", route.path()))))
        .unwrap_or_default();
    let height = if body.width() - 58. < 680. {
        270.
    } else {
        336.
    };
    let r = area(body.left(), body.top() - scroll, body.width(), height);
    let p = ui.painter().with_clip_rect(body.intersect(ui.clip_rect()));
    ui::image(&p, r, &img);
    let uv = Rect::from_min_max(pos2(0., 0.), pos2(1., 1.));
    p.image(img.id(), r, uv, Color32::from_rgb_additive(240, 240, 240));
    let mut mesh = Mesh::default();
    for (pt, color) in [
        (r.left_top(), Color32::TRANSPARENT),
        (r.right_top(), Color32::TRANSPARENT),
        (r.left_bottom(), BG),
        (r.right_bottom(), BG),
    ] {
        mesh.colored_vertex(pt, color);
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 3, 2);
    p.add(mesh);
}
pub fn hero(ui: &mut Ui, v: &mut View<'_>, h: Hero<'_>) {
    let side = if ui.available_width() < 680. {
        150.
    } else {
        216.
    };
    let (_, r) = ui.allocate_space(vec2(ui.available_width(), side + 46.));
    let p = ui.painter().clone();
    let offset = (ui.clip_rect().top() - r.top()).max(0.);
    ui.data_mut(|d| {
        d.insert_temp(
            Id::unique(("hero offset", v.views.history.current.path())),
            offset,
        )
    });
    let cover = Rect::from_min_size(r.min + vec2(0., 20.), Vec2::splat(side));
    cards::art(&p, cover, h.art, h.round, Size::Medium, v);
    let x = cover.right() + 28.;
    let width = (r.right() - x).max(1.);
    let size = if side < 200. { 42. } else { 64. };
    let mut job =
        egui::text::LayoutJob::simple(h.title.into(), font("Bold", size), Color32::WHITE, width);
    job.wrap.max_rows = 2;
    for section in &mut job.sections {
        section.format.extra_letter_spacing = -size * 0.02;
    }
    let title = p.layout_job(job);
    let base = cover.bottom() - 5.;
    let y = base - 25. - title.size().y;
    p.text(
        pos2(x, y + 4.),
        egui::Align2::LEFT_BOTTOM,
        h.label.to_uppercase(),
        font("Strong", 11.),
        TEXT,
    );
    p.galley(pos2(x, y + 5.), title, Color32::WHITE);
    let mut mx = x;
    if let Some((name, key)) = h.artist {
        let g = p.layout_no_wrap(name.into(), font("Mono", 12.5), TEXT);
        let w = g.size().x.min(width * 0.6);
        let link = area(x, base - 20., w, 21.);
        clip_label(ui, link, name, "Mono", 12.5, TEXT);
        if ui::hit(ui, link, "album artist", name)
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
        {
            v.go(Route::Artist(key.into()));
        }
        mx += w;
    }
    clip_label(
        ui,
        area(mx, base - 20., r.right() - mx, 21.),
        h.meta,
        "Mono",
        12.5,
        SECONDARY,
    );
}

pub fn actions(ui: &mut Ui, v: &mut View<'_>, ids: &Ids, more: bool, hint: Option<&str>) -> Ids {
    let ctx = v.context();
    let (_, r) = ui.allocate_space(vec2(ui.available_width(), 56.));
    if let Some(hint) = hint {
        let w = if r.width() < 670. { 150. } else { 220. };
        filter(ui, v, area(r.right() - w, r.center().y - 16., w, 32.), hint);
    }
    let ids = library::filtered(ui, v, ids);
    let filtered = !v.views.filter.trim().is_empty();
    let playing = !filtered && library::is_now(v, &ctx) && v.st.playing;
    if ui::play_big(
        ui,
        Rect::from_min_size(r.min, Vec2::splat(56.)),
        playing,
        v.tint,
    )
    .clicked()
    {
        library::play(v, &ids, ctx.clone(), false, filtered);
    }
    if ui::icon_btn(
        ui,
        area(r.left() + 70., r.center().y - 15., 30., 30.),
        "shuffle",
        "Shuffle",
        false,
        v.tint,
    )
    .clicked()
    {
        library::play(v, &ids, ctx.clone(), true, filtered);
    }
    if more {
        let resp = ui::icon_btn(
            ui,
            area(r.left() + 115., r.center().y - 15., 30., 30.),
            "more",
            "More",
            false,
            v.tint,
        );
        egui::Popup::menu(&resp).show(|ui| {
            for (name, end) in [("Play next", false), ("Add to queue", true)] {
                if ui.button(name).clicked() {
                    v.s.insert(v.player, &ids, end);
                    ui.close();
                }
            }
            if !v.s.models.playlists.is_empty() {
                let mut add = None;
                ui.menu_button("Add to playlist", |ui| {
                    for pl in &v.s.models.playlists {
                        if ui.button(&pl.name).clicked() {
                            add = Some(pl.id.clone());
                            ui.close();
                        }
                    }
                });
                if let Some(k) = add {
                    v.s.add_to_playlist(&k, ids.clone());
                }
            }
            if ui.button("Toggle liked").clicked() {
                v.s.toggle_like(&ids);
                ui.close();
            }
        });
    }
    ui.add_space(12.);
    ids
}
pub fn group_ids(ui: &Ui, v: &View<'_>) -> Ids {
    let key = Id::unique((v.views.history.current.path(), v.s.generation));
    library::cached(ui, "library detail tracks", key, || {
        v.s.models.context_ids(&v.context())
    })
    .as_ref()
    .clone()
}
