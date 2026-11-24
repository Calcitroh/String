pub mod covers;
pub mod credit;
pub mod edit_info;
pub mod glyph;
pub mod logo_disc;
pub mod menus;
pub mod panel;
pub mod queue_drawer;
pub mod side_playlists;
pub mod songs;
use crate::theme::*;
use egui::{
    Align2, Color32, Context, Id, Painter, Pos2, Rect, Response, Role, Sense, StrokeKind, Ui, Vec2,
    WidgetInfo, pos2, vec2,
};
use std::time::{Duration, Instant};
pub fn area(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect::from_min_size(pos2(x, y), vec2(w.max(0.), h.max(0.)))
}
pub fn child(ui: &mut Ui, r: Rect, id: impl egui::AsIdSalt) -> Ui {
    let mut u = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(id)
            .max_rect(r)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    u.set_clip_rect(ui.clip_rect().intersect(r));
    u
}
pub fn label(p: &Painter, r: Rect, text: &str, face: &str, size: f32, color: Color32) {
    spaced(p, r, text, face, size, 0., color, Align2::LEFT_CENTER);
}
/// One line of text placed in `r`, cut with "…" wherever it runs out of room (as CSS's `text-overflow: ellipsis`,
/// mid-word too), with `spacing` px between letters (CSS `letter-spacing`). Returns where the text went.
#[allow(clippy::too_many_arguments)]
pub fn spaced(
    p: &Painter,
    r: Rect,
    text: &str,
    face: &str,
    size: f32,
    spacing: f32,
    color: Color32,
    align: Align2,
) -> Rect {
    match place(p, r, text, face, size, spacing, color, align) {
        Some((at, g)) => {
            paint_placed(p, r, spacing, at, g, color);
            at
        }
        None => Rect::from_min_size(align.pos_in_rect(&r), Vec2::ZERO),
    }
}
/// `spaced`, with the web version's `text-shadow: 0 0 <blur> <tint at alpha>` behind the letters.
#[allow(clippy::too_many_arguments)]
pub fn glowing(
    p: &Painter,
    r: Rect,
    text: &str,
    face: &str,
    size: f32,
    spacing: f32,
    color: Color32,
    align: Align2,
    (blur, alpha, t): (f32, f32, Tint),
) -> Rect {
    match place(p, r, text, face, size, spacing, color, align) {
        Some((at, g)) => {
            p.add(text_glow(g.mesh_bounds.translate(at.min.to_vec2()), blur, alpha, t));
            paint_placed(p, r, spacing, at, g, color);
            at
        }
        None => Rect::from_min_size(align.pos_in_rect(&r), Vec2::ZERO),
    }
}
fn paint_placed(
    p: &Painter,
    r: Rect,
    spacing: f32,
    at: Rect,
    g: std::sync::Arc<egui::Galley>,
    color: Color32,
) {
    p.with_clip_rect(p.clip_rect().intersect(r.expand2(vec2(spacing, 3.))))
        .galley(at.min, g, color);
}
#[allow(clippy::too_many_arguments)]
fn place(
    p: &Painter,
    r: Rect,
    text: &str,
    face: &str,
    size: f32,
    spacing: f32,
    color: Color32,
    align: Align2,
) -> Option<(Rect, std::sync::Arc<egui::Galley>)> {
    if text.is_empty() || r.width() <= 0. {
        return None;
    }
    let mut job = egui::text::LayoutJob::single_section(
        text.into(),
        egui::TextFormat {
            font_id: font(face, size),
            extra_letter_spacing: spacing,
            color,
            ..Default::default()
        },
    );
    job.wrap = egui::text::TextWrapping {
        max_width: r.width(),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let g = p.layout_job(job);
    let at = align.anchor_size(align.pos_in_rect(&r), g.size());
    Some((at, g))
}
/// The tint this frame is drawn in (set once a frame by the shell), for drawing code that has no `View`.
pub fn tint(ctx: &Context) -> Tint {
    ctx.data(|d| d.get_temp(Id::unique("string tint")))
        .unwrap_or_default()
}
pub fn set_tint(ctx: &Context, t: Tint) {
    ctx.data_mut(|d| d.insert_temp(Id::unique("string tint"), t));
}
/// A soft glow around `r` (CSS `box-shadow: 0 0 <blur> <color>`), drawn by egui's own feathered shadow, so it costs a
/// handful of triangles.
pub fn glow(p: &Painter, r: Rect, radius: f32, blur: f32, color: Color32) {
    p.add(glow_shape(r, radius, blur, color));
}
pub fn glow_shape(r: Rect, radius: f32, blur: f32, color: Color32) -> egui::Shape {
    egui::epaint::Shadow {
        offset: [0, 0],
        blur: blur.clamp(0., 255.) as u8,
        spread: 0,
        color,
    }
    .as_shape(r, radius.clamp(0., 255.) as u8)
    .into()
}
/// The haze of a CSS `text-shadow: 0 0 <blur> <tint at alpha>` around letters whose ink fills `ink` (a galley's
/// `mesh_bounds`, placed). Three very faint soft pills, each wider and softer than the last, so it fades out with no
/// edge to read as a highlight: about 10 levels brighter on the letters and 2 or 3 at the blur's distance, as the web
/// version's page titles measure.
pub fn text_glow(ink: Rect, blur: f32, alpha: f32, t: Tint) -> egui::Shape {
    let k = alpha / 0.35;
    egui::Shape::Vec(
        [(0.0, 0.65, 0.018), (0.3, 1.2, 0.014), (0.8, 2.0, 0.012)]
            .iter()
            .map(|&(grow, soft, a)| {
                let r = ink.expand(blur * grow);
                glow_shape(r, r.height() / 2., blur * soft, t.ink(a * k))
            })
            .collect(),
    )
}
/// A rectangle fading from `a` to `b`: left to right when `across`, else top to bottom (a CSS linear-gradient).
pub fn gradient(p: &Painter, r: Rect, a: Color32, b: Color32, across: bool) {
    let mut m = egui::Mesh::default();
    let (c1, c3) = if across { (b, a) } else { (a, b) };
    m.colored_vertex(r.left_top(), a);
    m.colored_vertex(r.right_top(), c1);
    m.colored_vertex(r.right_bottom(), b);
    m.colored_vertex(r.left_bottom(), c3);
    m.add_triangle(0, 1, 2);
    m.add_triangle(0, 2, 3);
    p.add(m);
}
/// A disc filled from `inner` at `focus` out to `outer` at its edge (a CSS radial-gradient), as one triangle fan.
pub fn radial(p: &Painter, center: Pos2, radius: f32, focus: Pos2, inner: Color32, outer: Color32) {
    let mut m = egui::Mesh::default();
    m.colored_vertex(focus, inner);
    for i in 0..=48 {
        let at = center + Vec2::angled(i as f32 * std::f32::consts::TAU / 48.) * radius;
        m.colored_vertex(at, outer);
        if i > 0 {
            m.add_triangle(0, i, i + 1);
        }
    }
    p.add(m);
}
/// A ring from `outer` colour at radius `r1` fading to `inner` at `r0` (CSS `box-shadow: inset 0 0 …` on a circle).
pub fn ring(p: &Painter, center: Pos2, r1: f32, r0: f32, outer: Color32, inner: Color32) {
    let mut m = egui::Mesh::default();
    for i in 0..=48 {
        let d = Vec2::angled(i as f32 * std::f32::consts::TAU / 48.);
        m.colored_vertex(center + d * r1, outer);
        m.colored_vertex(center + d * r0, inner);
        if i > 0 {
            let k = 2 * i;
            m.add_triangle(k - 2, k - 1, k);
            m.add_triangle(k - 1, k, k + 1);
        }
    }
    p.add(m);
}
pub fn hit(ui: &mut Ui, r: Rect, id: impl egui::AsIdSalt, label: &str) -> Response {
    let resp = ui.interact(r, ui.scope_id().with(id), Sense::click());
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, label));
    resp
}
/// The web version's square icon button: lit while on (the tint inside a faint ring of it), a faint box on hover.
pub fn icon_btn(ui: &mut Ui, r: Rect, name: &str, title: &str, on: bool, t: Tint) -> Response {
    styled_btn(ui, r, name, title, on, false, false, t)
}
/// The round icon button of the player bar and the top bar. `mode` (shuffle, repeat) adds the small glowing dot
/// under it while on.
pub fn round_btn(
    ui: &mut Ui,
    r: Rect,
    name: &str,
    title: &str,
    on: bool,
    mode: bool,
    t: Tint,
) -> Response {
    styled_btn(ui, r, name, title, on, true, mode, t)
}
#[allow(clippy::too_many_arguments)]
fn styled_btn(
    ui: &mut Ui,
    r: Rect,
    name: &str,
    title: &str,
    on: bool,
    round: bool,
    mode: bool,
    t: Tint,
) -> Response {
    let resp = hit(ui, r, title, title);
    let hot = resp.hovered() || resp.has_focus();
    let p = ui.painter();
    let (fill, line) = if on {
        (t.ink(0.08), t.ink(0.25))
    } else if hot {
        (white(0.05), HAIR)
    } else {
        (Color32::TRANSPARENT, Color32::TRANSPARENT)
    };
    if fill != Color32::TRANSPARENT {
        if round {
            p.circle(r.center(), r.width() / 2. - 0.5, fill, (1., line));
        } else {
            p.rect(r, 0, fill, (1., line), StrokeKind::Inside);
        }
    }
    glyph::draw(
        p,
        Rect::from_center_size(r.center(), Vec2::splat(if round { 18. } else { 16. })),
        name,
        if on {
            t.ink(1.)
        } else if hot {
            TEXT
        } else {
            MUTED
        },
    );
    if on && mode {
        let dot = r.center_bottom() - vec2(0., 3.);
        glow(p, Rect::from_center_size(dot, Vec2::splat(4.)), 2., 6., t.ink(0.5));
        p.circle_filled(dot, 2., t.ink(1.));
    }
    resp.on_hover_text(title)
}
/// The like button: an outline heart, or once liked a filled, softly glowing one with no box around it.
pub fn like_btn(ui: &mut Ui, r: Rect, title: &str, liked: bool, t: Tint) -> Response {
    if !liked {
        return styled_btn(ui, r, "heart", title, false, true, false, t);
    }
    let resp = hit(ui, r, title, title);
    let p = ui.painter();
    let heart = Rect::from_center_size(r.center() + vec2(0., 0.5), Vec2::splat(15.));
    crate::screens::cards::lit_heart(p, heart, t);
    resp.on_hover_text(title)
}
/// The big round play button: a lit rim, a soft halo and a glowing glyph (the web version's `.play-big`).
pub fn play_big(ui: &mut Ui, r: Rect, playing: bool, t: Tint) -> Response {
    let name = if playing { "Pause" } else { "Play" };
    let res = hit(ui, r, "transport", name);
    let hot = res.hovered();
    let p = ui.painter();
    let c = r.center();
    let radius = r.width() * 0.5;
    glow(
        p,
        r,
        radius,
        if hot { 34. } else { 24. },
        t.ink(if hot { 0.2 } else { 0.11 }),
    );
    radial(
        p,
        c,
        radius - 0.5,
        pos2(c.x, r.top() + r.height() * 0.35),
        t.ink(if hot { 0.3 } else { 0.2 }),
        Color32::from_black_alpha(if hot { 140 } else { 153 }),
    );
    ring(
        p,
        c,
        radius - 0.5,
        radius * 0.55,
        t.ink(if hot { 0.2 } else { 0.12 }),
        Color32::TRANSPARENT,
    );
    p.circle_stroke(c, radius - 0.5, (1., t.ink(0.7)));
    let icon = Rect::from_center_size(c, Vec2::splat(radius));
    glow(p, icon.shrink(radius * 0.2), radius * 0.2, 6., t.ink(0.3));
    glyph::draw(
        p,
        icon,
        if playing { "pausef" } else { "playf" },
        t.ink(1.),
    );
    res.on_hover_text(name)
}
pub fn btn(ui: &mut Ui, text: &str) -> Response {
    ui.add(
        egui::Button::new(egui::RichText::new(text).font(font("HUD", 13.)))
            .corner_radius(3)
            .min_size(vec2(0., 31.)),
    )
}
pub fn seg(ui: &mut Ui, text: &str, selected: bool) -> Response {
    ui.add(
        egui::Button::new(hud(text, 11.))
            .selected(selected)
            .corner_radius(1)
            .min_size(vec2(58., 24.)),
    )
}
pub fn rail(ui: &mut Ui, r: Rect, name: &str, frac: f32, t: Tint) -> (Response, Option<f32>) {
    let resp = ui.interact(r, ui.scope_id().with(name), Sense::click_and_drag());
    let primary = egui::PointerButton::Primary;
    let released = resp.clicked_by(primary) || resp.drag_stopped_by(primary);
    let held = resp.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_down());
    let value = if released || held {
        ui.input(|i| i.pointer.interact_pos())
            .map(|pt| ((pt.x - r.left()) / r.width().max(1.)).clamp(0., 1.))
    } else {
        None
    };
    let frac = value.unwrap_or(frac);
    resp.widget_info(|| {
        WidgetInfo::labeled(
            Role::Slider,
            true,
            format!("{name} {:.0} percent", frac * 100.),
        )
    });
    let y = r.center().y;
    let range = r.x_range();
    let x = egui::lerp(range, frac.clamp(0., 1.));
    let p = ui.painter();
    // the web version's slider: a faint 3 px rail (5 px under the pointer), filled with a glowing tint gradient
    let hot = resp.hovered() || resp.dragged() || resp.has_focus();
    let h = if hot { 5. } else { 3. };
    let rail = Rect::from_min_max(pos2(r.left(), y - h / 2.), pos2(r.right(), y + h / 2.));
    p.rect_filled(rail, 0, white(0.1));
    let fill = rail.with_max_x(x);
    if fill.width() > 0.5 {
        glow(p, fill, 0., 10., t.ink(0.16));
        gradient(p, fill, t.ink(0.5), t.ink(1.), true);
    }
    if hot {
        let knob = Rect::from_center_size(pos2(x, y), Vec2::splat(12.));
        glow(p, knob, 6., 12., t.ink(0.45));
        p.circle_filled(pos2(x, y), 6., t.ink(1.));
    }
    (resp, value)
}
pub fn sleeve(p: &Painter, r: Rect, art: Option<covers::Art>) {
    p.rect_filled(r, 1, PANEL2);
    if let Some(art) = art {
        image(p, r, &art.cover)
    }
    p.rect_stroke(r, 1, (1., HAIR), StrokeKind::Inside);
}
pub fn image(p: &Painter, r: Rect, tex: &egui::TextureHandle) {
    p.image(
        tex.id(),
        r,
        Rect::from_min_max(Pos2::ZERO, pos2(1., 1.)),
        Color32::WHITE,
    );
}
pub fn context_menu(r: &Response, items: impl FnOnce(&mut Ui)) {
    r.context_menu(items);
}
#[derive(Default)]
pub struct Toast {
    current: Option<(String, Instant)>,
}
impl Toast {
    pub fn set(&mut self, message: impl Into<String>) {
        self.current = Some((message.into(), Instant::now()));
    }
    pub fn show(&mut self, ctx: &Context) {
        if let Some((msg, at)) = &self.current {
            let ttl = Duration::from_secs(3);
            if at.elapsed() >= ttl {
                self.current = None;
                return;
            }
            // above everything else, the icon credit's card included: a toast may say a save failed. An area keeps
            // its first size, so each message gets its own and a long one isn't wrapped to a short one's width.
            egui::Area::new(Id::unique(("toast", msg.as_str())))
                .anchor(Align2::CENTER_BOTTOM, [0., -BAR - 10.])
                .order(egui::Order::Tooltip)
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| ui.label(msg));
                });
            ctx.request_repaint_after(ttl.saturating_sub(at.elapsed()));
        }
    }
}
pub fn confirm(ctx: &Context, title: &str, message: &str, accept: &str) -> Option<bool> {
    let response = egui::Modal::new(Id::unique("confirm")).show(ctx, |ui| {
        ui.set_max_width(420.);
        ui.heading(title);
        ui.label(message);
        ui.separator();
        ui.horizontal(|ui| {
            if btn(ui, "Cancel").clicked() {
                Some(false)
            } else if btn(ui, accept).clicked() {
                Some(true)
            } else {
                None
            }
        })
        .inner
    });
    response
        .inner
        .or_else(|| response.should_close().then_some(false))
}
