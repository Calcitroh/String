use super::{
    cards::{self, Art},
    library::{self, clip_label},
    motion::Motion,
};
#[path = "../ui/hero_backdrop.rs"]
mod hero_backdrop;
use crate::{
    model::Ids,
    route::Route,
    theme::*,
    ui::{self, area, covers::Size},
    view::View,
};
use egui::{Color32, Id, Rect, Ui, Vec2, pos2, vec2};

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
#[derive(Clone, Copy)]
struct Backdrop {
    body: Rect,
    layer: egui::LayerId,
    shape: egui::layers::ShapeIdx,
    shadow: egui::layers::ShapeIdx,
    pass: u64,
}
pub(crate) fn backdrop(ui: &Ui, v: &mut View<'_>, body: Rect) {
    if matches!(v.views.history.current, Route::Album(_) | Route::Artist(_)) {
        // Reserve the paint order below the top bar. The scroll area supplies this frame's
        // actual header position (including a restored scroll offset) when it lays out the hero.
        let slot = Backdrop {
            body: body.intersect(ui.clip_rect()),
            layer: ui.layer_id(),
            shape: ui.painter().add(egui::Shape::Noop),
            shadow: ui.painter().add(egui::Shape::Noop),
            pass: ui.ctx().cumulative_pass_nr(),
        };
        ui.data_mut(|d| d.insert_temp(Id::unique("hero underlay"), slot));
    }
}
fn paint_backdrop(ui: &Ui, v: &View<'_>, hero: Rect, sleeve: Rect, h: &Hero<'_>, motion: Motion) {
    let Some(slot) = ui.data(|d| d.get_temp::<Backdrop>(Id::unique("hero underlay"))) else {
        return;
    };
    if slot.pass != ui.ctx().cumulative_pass_nr() {
        return;
    }
    let r = Rect::from_min_max(
        pos2(slot.body.left(), hero.top() - 72.),
        pos2(slot.body.right(), hero.bottom()),
    );
    if !slot.body.intersects(r) {
        return;
    }
    let mut underlay = ui
        .ctx()
        .layer_painter(slot.layer)
        .with_clip_rect(slot.body.intersect(r));
    underlay.set_opacity(ui.opacity());
    let radius = if h.round { sleeve.width() / 2. } else { 0. };
    underlay.set(
        slot.shadow,
        ui::glow_shape(
            sleeve.translate(vec2(0., 30.)),
            radius,
            70.,
            Color32::from_black_alpha(179),
        ),
    );
    let Art::Cover(c) = h.art else {
        return;
    };
    if c.file.is_empty() && c.embedded.is_empty() && c.fallback.is_empty() {
        return;
    }
    if let Some(img) = hero_backdrop::texture(ui.ctx(), c, r.size(), v.mono) {
        let glow = motion.reveal(ui.ctx(), img.id(), 0.6);
        underlay.set(slot.shape, hero_backdrop::mesh(r, &img, glow));
    }
}
pub fn hero(ui: &mut Ui, v: &mut View<'_>, h: Hero<'_>) {
    let side = if ui.available_width() < 680. {
        150.
    } else {
        216.
    };
    let (_, r) = ui.allocate_space(vec2(ui.available_width(), side + 46.));
    let p = ui.painter().clone();
    let motion = Motion::for_page(ui.ctx(), &v.views.history.current);
    let cover = Rect::from_min_size(r.min + vec2(0., 20.), Vec2::splat(side));
    paint_backdrop(ui, v, r, cover, &h, motion);
    cards::art(
        &p,
        cover,
        h.art,
        cards::Sleeve {
            round: h.round,
            size: Size::Medium,
            zoom: 1.,
        },
        motion,
        v,
    );
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
        if resp.clicked() {
            crate::ui::menus::ctx_menu(ctx, resp.rect.left_bottom(), Some(ids.clone()));
        }
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
