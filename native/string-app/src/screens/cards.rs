use super::library::{clip_label, is_now, page_context, play, section};
use crate::{
    model::{CoverSource, Ids, Song},
    route::Route,
    theme::*,
    ui::{self, area, covers::Size},
    view::View,
};
use egui::{Color32, Mesh, Painter, Rect, Shape, StrokeKind, Ui, Vec2, pos2, vec2};
use serde_json::Value;
use std::sync::Arc;

#[derive(Clone)]
pub enum Art {
    Cover(Arc<CoverSource>),
    Mosaic(Vec<Arc<CoverSource>>),
    Heart,
    Shuffle,
}
pub enum Target {
    Page(Route),
    Track(String),
    Shuffle,
}
pub struct Card {
    pub target: Target,
    pub title: String,
    pub subtitle: String,
    pub art: Art,
}
impl Card {
    pub fn page(route: Route, title: &str, subtitle: String, art: Art) -> Self {
        Self {
            target: Target::Page(route),
            title: title.into(),
            subtitle,
            art,
        }
    }
    pub fn track(s: &Song) -> Self {
        Self {
            target: Target::Track(s.id.clone()),
            title: s.title.clone(),
            subtitle: s.artist_name().into(),
            art: Art::Cover(s.cover.clone()),
        }
    }
    pub fn is(&self, r: &Route) -> bool {
        matches!(&self.target,Target::Page(route) if route==r)
    }
}

fn circle_image(p: &Painter, r: Rect, texture: egui::TextureId) {
    let mut mesh = Mesh::with_texture(texture);
    let uv_center = pos2(0.5, 0.5);
    mesh.vertices.push(egui::epaint::Vertex {
        pos: r.center(),
        uv: uv_center,
        color: Color32::WHITE,
    });
    for i in 0..=96 {
        let dir = Vec2::angled(i as f32 * std::f32::consts::TAU / 96.);
        mesh.vertices.push(egui::epaint::Vertex {
            pos: r.center() + dir * r.width() * 0.5,
            uv: uv_center + dir * 0.5,
            color: Color32::WHITE,
        });
        if i > 0 {
            mesh.add_triangle(0, i, i + 1);
        }
    }
    p.add(mesh);
}
pub fn heart(p: &Painter, r: Rect, t: Tint) {
    // A triangle fan preserves the notch at the top of the heart.
    let mut mesh = Mesh::default();
    mesh.colored_vertex(r.center(), t.ink(1.));
    for i in 0..=64 {
        let a = i as f32 * std::f32::consts::TAU / 64.;
        let x = 16. * a.sin().powi(3);
        let y = 13. * a.cos() - 5. * (2. * a).cos() - 2. * (3. * a).cos() - (4. * a).cos();
        mesh.colored_vertex(r.center() + vec2(x, -y - 2.) * (r.width() / 32.), t.ink(1.));
        if i > 0 {
            mesh.add_triangle(0, i, i + 1);
        }
    }
    p.add(mesh);
}
pub fn art(p: &Painter, r: Rect, art: &Art, round: bool, size: Size, v: &mut View<'_>) {
    match art {
        Art::Cover(c) => {
            let img = v.covers.get(c, size, v.mono.then_some(v.tint));
            if round {
                p.circle_filled(r.center(), r.width() / 2., PANEL);
                if let Some(img) = img {
                    circle_image(p, r, img.cover.id());
                    p.circle_filled(
                        r.center(),
                        r.width() / 2.,
                        Color32::from_black_alpha(if v.mono { 55 } else { 25 }),
                    );
                }
                p.circle_stroke(r.center(), r.width() / 2., (1., HAIR));
            } else {
                ui::sleeve(p, r, img);
                p.rect_filled(
                    r,
                    0,
                    Color32::from_black_alpha(if v.mono { 55 } else { 25 }),
                );
            }
        }
        Art::Mosaic(cs) => {
            for (i, c) in cs.iter().enumerate() {
                let side = r.width() / 2.;
                let tile = area(
                    r.left() + (i % 2) as f32 * side,
                    r.top() + (i / 2) as f32 * side,
                    side,
                    side,
                );
                ui::sleeve(
                    p,
                    tile,
                    v.covers.get(c, Size::Small, v.mono.then_some(v.tint)),
                );
            }
        }
        Art::Heart => {
            p.rect_filled(r, 0, BG2);
            for i in (1..=44).rev() {
                let f = i as f32 / 44.;
                p.circle_filled(
                    r.center() - vec2(0., r.height() * 0.045),
                    r.width() * 0.50 * f,
                    v.tint.light(0.007 + 0.016 * (1. - f)),
                );
            }
            heart(
                p,
                Rect::from_center_size(r.center(), r.size() * 0.30),
                v.tint,
            );
        }
        Art::Shuffle => {
            p.rect_filled(r, 0, PANEL);
            for i in 0..15 {
                let y = r.top() + r.height() * i as f32 / 15.;
                let points = (0..=20)
                    .map(|j| {
                        let f = j as f32 / 20.;
                        pos2(
                            r.left() + r.width() * f,
                            y + 3. * (f * 5. + i as f32 * 0.2).sin(),
                        )
                    })
                    .collect();
                p.with_clip_rect(r.intersect(p.clip_rect()))
                    .add(Shape::line(points, (0.75, MUTED)));
            }
            ui::glyph::draw(
                p,
                Rect::from_center_size(r.center(), Vec2::splat(25.)),
                "shuffle",
                TEXT,
            );
        }
    }
}
type RowList<'a> = Option<(&'a Ids, &'a Value)>;

fn draw(ui: &mut Ui, v: &mut View<'_>, r: Rect, card: &Card, tile: bool, list: RowList<'_>) {
    if !ui.is_rect_visible(r) {
        return;
    }
    let mut u = ui::child(ui, r.with_min_y(r.top() - 3.), ui.next_auto_id());
    ui.skip_ahead_auto_ids(1);
    let ctx = match &card.target {
        Target::Page(route) => Some(page_context(route, &card.title)),
        _ => None,
    };
    let now = match &card.target {
        Target::Track(id) => v.s.state.queue.current() == Some(id.as_str()),
        _ => ctx.as_ref().is_some_and(|c| is_now(v, c)),
    };
    let response =
        ui::hit(&mut u, r, "open", &card.title).on_hover_cursor(egui::CursorIcon::PointingHand);
    let hover = u.rect_contains_pointer(r) || response.has_focus();
    // a card lifts 3 px under the pointer, as in the web version
    let lift = if tile {
        0.
    } else {
        3. * u.ctx().animate_bool_with_time(response.id, hover, 0.3)
    };
    let r = r.translate(vec2(0., -lift));
    let p = u.painter().clone();
    // the web version's colours: cards white at 1.8 % (5 % under the pointer), home tiles at 3 % with a hairline,
    // lit by the tint under the pointer
    let (fill, line) = match (tile, hover) {
        (true, true) => (v.tint.ink(0.07), v.tint.ink(0.25)),
        (true, false) => (white(0.03), HAIR),
        (false, true) => (white(0.05), HAIR2),
        (false, false) => (white(0.018), Color32::TRANSPARENT),
    };
    p.rect(
        r,
        0,
        fill,
        (1., if now { v.tint.ink(0.35) } else { line }),
        StrokeKind::Inside,
    );
    let side = if tile {
        r.height() - 2.
    } else {
        r.width() - 26.
    };
    let cover = Rect::from_min_size(
        r.min + Vec2::splat(if tile { 1. } else { 13. }),
        Vec2::splat(side),
    );
    art(
        &p,
        cover,
        &card.art,
        !tile && matches!(card.target, Target::Page(Route::Artist(_))),
        if tile { Size::Small } else { Size::Medium },
        v,
    );
    if !tile
        && let Art::Cover(c) = &card.art
        && c.file.is_empty()
        && c.embedded.is_empty()
        && c.fallback.is_empty()
    {
        let initials: String = card
            .title
            .split(|c: char| !c.is_alphanumeric())
            .filter_map(|w| w.chars().next())
            .take(2)
            .flat_map(char::to_uppercase)
            .collect();
        let round = matches!(&card.target, Target::Page(Route::Artist(_)));
        let x = cover.left() + side * 0.07;
        let y = cover.bottom() - side * 0.05;
        let ink_rect = if round {
            cover
                .with_min_x(cover.left() + side * 0.17)
                .with_max_y(cover.bottom() - side * 0.09)
        } else {
            cover
        };
        p.with_clip_rect(ink_rect.intersect(p.clip_rect())).text(
            pos2(x, y),
            egui::Align2::LEFT_BOTTOM,
            initials,
            font("HUD", side * 0.15),
            v.tint.ink(0.75),
        );
    }
    let button = if tile {
        clip_label(
            &u,
            area(
                cover.right() + 14.,
                r.top(),
                (r.width() - 125.).max(1.),
                r.height(),
            ),
            &card.title,
            "Strong",
            13.,
            if now { v.tint.ink(1.) } else { TEXT },
        );
        Rect::from_center_size(pos2(r.right() - 31., r.center().y), Vec2::splat(38.))
    } else {
        clip_label(
            &u,
            area(cover.left(), cover.bottom() + 10., side, 20.),
            &card.title,
            "Strong",
            13.5,
            if now { v.tint.ink(1.) } else { TEXT },
        );
        clip_label(
            &u,
            area(cover.left(), cover.bottom() + 33., side, 16.),
            &card.subtitle,
            "Mono",
            11.5,
            FAINT,
        );
        Rect::from_min_size(cover.right_bottom() - Vec2::splat(52.), Vec2::splat(44.))
    };
    let mut fab = false;
    if hover || now {
        p.circle_filled(
            button.center(),
            button.width() / 2.,
            Color32::from_black_alpha(198),
        );
        fab = ui::play_big(&mut u, button, now && v.st.playing, v.tint).clicked();
    }
    if !fab && !response.clicked() {
        return;
    }
    match &card.target {
        Target::Page(route) if !fab => v.go(route.clone()),
        Target::Page(_) => {
            let ctx = ctx.unwrap();
            play(v, &v.s.models.context_ids(&ctx), ctx, false, false);
        }
        Target::Track(id) => {
            if now {
                v.s.toggle(v.player);
            } else if let Some((ids, ctx)) = list
                && let Some(i) = ids.iter().position(|s| s == id)
            {
                v.s.play_ids(v.player, ids, i as isize, ctx.clone());
            }
        }
        Target::Shuffle => play(
            v,
            &v.s.models.songs.clone(),
            page_context(&Route::Songs, "All songs"),
            true,
            false,
        ),
    }
}

pub fn grid<'a>(
    ui: &mut Ui,
    v: &mut View<'_>,
    cards: impl IntoIterator<Item = &'a Card>,
    tile: bool,
) {
    let (min, gap) = if tile { (250., 10.) } else { (168., 18.) };
    let width = ui.available_width();
    let cols = (((width + gap) / (min + gap)).floor() as usize).max(1);
    let w = (width - gap * (cols - 1) as f32) / cols as f32;
    let h = if tile { 62. } else { w + 48. };
    let mut row = Rect::NOTHING;
    for (i, card) in cards.into_iter().enumerate() {
        if i % cols == 0 {
            if i > 0 {
                ui.add_space((gap - ui.spacing().item_spacing.y).max(0.));
            }
            row = ui.allocate_space(vec2(width, h)).1;
        }
        let rect = area(row.left() + (i % cols) as f32 * (w + gap), row.top(), w, h);
        draw(ui, v, rect, card, tile, None);
    }
}
pub fn row<'a>(
    ui: &mut Ui,
    v: &mut View<'_>,
    title: &str,
    cards: impl IntoIterator<Item = &'a Card>,
    link: Option<Route>,
    list: RowList<'_>,
) {
    let mut cards = cards.into_iter().peekable();
    if cards.peek().is_none() {
        return;
    }
    section(ui, title, link, v);
    egui::ScrollArea::horizontal()
        .id_salt(title)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            // room for a card to lift into
            ui.add_space(4.);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 18.;
                for card in cards {
                    let (_, r) = ui.allocate_space(vec2(168., 216.));
                    draw(ui, v, r, card, false, list);
                }
            });
        });
    ui.add_space(6.);
}
