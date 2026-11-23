//! The HUD kit for pages made of panels: headers, rows, switches, segments, buttons, fields and dialogs.
use super::{glow, glyph, gradient, text_glow};
use crate::theme::*;
use egui::{
    Align, Color32, Context, CornerRadius, Frame, Id, Key, Label, Layout, Margin, Rect, Response,
    RichText, Role, Sense, Shape, Stroke, StrokeKind, Ui, WidgetInfo, pos2, text::LayoutJob, vec2,
};
use std::sync::Arc;

#[path = "settings_select.rs"]
pub mod select;

pub const INK: Color32 = Color32::from_rgb(5, 5, 6);
pub fn white(opacity: f32) -> Color32 {
    Color32::from_white_alpha((opacity * 255.) as u8)
}
pub fn black(opacity: f32) -> Color32 {
    Color32::from_black_alpha((opacity * 255.) as u8)
}
/// One line in the given face, with letter spacing in ems. Its colour comes from the paint call.
pub fn line(ui: &Ui, text: &str, face: &str, size: f32, spacing: f32) -> Arc<egui::Galley> {
    let mut job = LayoutJob::simple_singleline(text.into(), font(face, size), Color32::PLACEHOLDER);
    job.sections[0].format.extra_letter_spacing = size * spacing;
    ui.painter().layout_job(job)
}
fn caps(ui: &Ui, text: &str, size: f32, spacing: f32) -> Arc<egui::Galley> {
    line(ui, &text.to_uppercase(), "HUD", size, spacing)
}
/// Body text of a row or a paragraph.
pub fn body(text: &str) -> RichText {
    RichText::new(text).font(font("UI", 14.)).color(SECONDARY)
}
pub fn page_title(ui: &mut Ui, title: &str, sub: &str, actions: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.set_min_height(30.);
        let t = line(ui, &title.to_uppercase(), "HUD", 22., 0.3);
        let s = line(ui, &sub.to_uppercase(), "Mono", 11., 0.08);
        let (r, _) =
            ui.allocate_exact_size(vec2(t.size().x + 10. + s.size().x, 26.), Sense::hover());
        let p = ui.painter();
        p.galley(
            pos2(r.left(), r.center().y - t.size().y * 0.5),
            t.clone(),
            TEXT,
        );
        p.galley(
            pos2(r.left() + t.size().x + 10., r.bottom() - s.size().y - 4.),
            s,
            FAINT,
        );
        ui.with_layout(Layout::right_to_left(Align::Center), actions);
    });
    ui.add_space(16.);
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Plain,
    Primary,
    Ghost,
    Danger,
}
/// A HUD button: `Btn::new("Rescan", tint).icon("reload").sm().show(ui)`.
pub struct Btn<'a> {
    text: &'a str,
    icon: Option<&'a str>,
    kind: Kind,
    small: bool,
    enabled: bool,
    hint: &'a str,
    tint: Tint,
}
impl<'a> Btn<'a> {
    pub fn new(text: &'a str, tint: Tint) -> Self {
        Self {
            text,
            icon: None,
            kind: Kind::Plain,
            small: false,
            enabled: true,
            hint: "",
            tint,
        }
    }
    pub fn icon(mut self, name: &'a str) -> Self {
        self.icon = Some(name);
        self
    }
    pub fn kind(mut self, kind: Kind) -> Self {
        self.kind = kind;
        self
    }
    pub fn sm(mut self) -> Self {
        self.small = true;
        self
    }
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
    pub fn hint(mut self, hint: &'a str) -> Self {
        self.hint = hint;
        self
    }
    pub fn show(self, ui: &mut Ui) -> Response {
        let (size, spacing, h, pad, icon) = if self.small {
            (10.5, 0.12, 28., 11., 14.)
        } else {
            (11.5, 0.14, 36., 17., 15.)
        };
        let text = caps(ui, self.text, size, spacing);
        let lead = if self.icon.is_some() { icon + 9. } else { 0. };
        let w = (pad * 2. + lead + text.size().x - size * spacing).min(ui.max_rect().width());
        let sense = if self.enabled {
            Sense::click()
        } else {
            Sense::hover()
        };
        let (r, resp) = ui.allocate_exact_size(vec2(w, h), sense);
        resp.widget_info(|| WidgetInfo::labeled(Role::Button, self.enabled, self.text));
        let hot = self.enabled && (resp.hovered() || resp.has_focus());
        let t = self.tint;
        let (fill, edge, mut ink) = match (self.kind, hot) {
            (Kind::Plain, false) => (white(0.03), HAIR3, SECONDARY),
            (Kind::Plain, true) => (t.ink(0.08), t.ink(0.6), TEXT),
            (Kind::Primary, false) => (t.ink(1.), Color32::TRANSPARENT, INK),
            (Kind::Primary, true) => (Color32::WHITE, Color32::TRANSPARENT, INK),
            (Kind::Ghost, false) => (Color32::TRANSPARENT, Color32::TRANSPARENT, MUTED),
            (Kind::Ghost, true) => (white(0.04), HAIR2, TEXT),
            (Kind::Danger, false) => (Color32::TRANSPARENT, white(0.5), TEXT),
            (Kind::Danger, true) => (Color32::WHITE, Color32::WHITE, INK),
        };
        let fade = if self.enabled { 1. } else { 0.35 };
        let (fill, edge) = (fill.gamma_multiply(fade), edge.gamma_multiply(fade));
        ink = ink.gamma_multiply(fade);
        let p = ui.painter();
        if self.kind == Kind::Primary {
            let c = if self.small { 5. } else { 8. };
            p.add(Shape::convex_polygon(
                vec![
                    pos2(r.left() + c, r.top()),
                    r.right_top(),
                    pos2(r.right(), r.bottom() - c),
                    pos2(r.right() - c, r.bottom()),
                    r.left_bottom(),
                    pos2(r.left(), r.top() + c),
                ],
                fill,
                Stroke::NONE,
            ));
        } else {
            p.rect_filled(r, 0, fill);
            if self.kind == Kind::Danger && !hot {
                let stripes = p.with_clip_rect(r.intersect(p.clip_rect()));
                let mut x = r.left() - r.height();
                while x < r.right() {
                    stripes.line_segment(
                        [pos2(x, r.bottom()), pos2(x + r.height(), r.top())],
                        (6., white(0.07 * fade)),
                    );
                    x += 17.;
                }
            }
            p.rect_stroke(r, 0, (1., edge), StrokeKind::Inside);
        }
        if let Some(name) = self.icon {
            glyph::draw(
                p,
                Rect::from_center_size(
                    pos2(r.left() + pad + icon * 0.5, r.center().y),
                    vec2(icon, icon),
                ),
                name,
                ink,
            );
        }
        p.with_clip_rect(r.intersect(ui.clip_rect())).galley(
            pos2(r.left() + pad + lead, r.center().y - text.size().y * 0.5),
            text,
            ink,
        );
        let resp = if self.enabled {
            resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        } else {
            resp
        };
        if self.hint.is_empty() {
            resp
        } else {
            resp.on_hover_text(self.hint)
        }
    }
}
/// A panel: border, a faint light from the top, and room inside. Returns its height before `min_height` stretched
/// it, so a row of panels can share the tallest one's.
pub fn panel<R>(ui: &mut Ui, min_height: f32, add: impl FnOnce(&mut Ui) -> R) -> (R, f32) {
    Frame::new()
        .fill(Color32::from_rgba_unmultiplied(7, 7, 9, 209))
        .stroke((1., HAIR2))
        .inner_margin(Margin::symmetric(22, 20))
        .show(ui, |ui| {
            let glow = ui.painter().add(Shape::Noop);
            ui.set_width(ui.available_width());
            let out = add(ui);
            let natural = ui.min_rect().height() + 42.;
            let inner = ui.min_rect();
            ui.expand_to_include_rect(Rect::from_min_size(
                inner.min,
                vec2(inner.width(), min_height - 42.),
            ));
            let r = ui.min_rect().expand2(vec2(22., 20.));
            let mut mesh = egui::Mesh::default();
            let low = r.top() + 120.0_f32.min(r.height());
            for (pt, c) in [
                (r.left_top(), white(0.03)),
                (r.right_top(), white(0.03)),
                (pos2(r.right(), low), Color32::TRANSPARENT),
                (pos2(r.left(), low), Color32::TRANSPARENT),
            ] {
                mesh.colored_vertex(pt, c);
            }
            mesh.add_triangle(0, 1, 2);
            mesh.add_triangle(0, 2, 3);
            ui.painter().set(glow, mesh);
            (out, natural)
        })
        .inner
}
/// A panel's header: its name, a hairline, and a note at the right.
pub fn panel_h(ui: &mut Ui, label: &str, note: &str, tint: Tint) {
    let name = caps(ui, label, 10.5, 0.2);
    let mut job = LayoutJob::simple_singleline(note.into(), font("Mono", 10.5), FAINT);
    job.sections[0].format.extra_letter_spacing = 0.63;
    job.sections[0].format.line_height = Some(10.5);
    job.wrap.max_width = (ui.available_width() - name.size().x - 20.).max(1.);
    job.halign = Align::Max;
    let note = ui.painter().layout_job(job);
    let (r, _) = ui.allocate_exact_size(
        vec2(ui.available_width(), note.size().y.max(10.5)),
        Sense::hover(),
    );
    let p = ui
        .painter()
        .with_clip_rect(r.expand2(vec2(0., 8.)).intersect(ui.clip_rect()));
    let y = r.center().y;
    let from = r.left() + name.size().x + 10.;
    let to = if note.size().x > 0. {
        (r.right() - note.size().x - 10.).max(from)
    } else {
        r.right()
    };
    let at = pos2(r.left(), y - name.size().y * 0.5);
    p.add(text_glow(
        name.mesh_bounds.translate(at.to_vec2()),
        8.,
        0.35,
        tint,
    ));
    p.galley(at, name, tint.ink(1.));
    if to > from {
        gradient(
            &p,
            Rect::from_min_max(pos2(from, y), pos2(to, y + 1.)),
            HAIR2,
            Color32::TRANSPARENT,
            true,
        );
    }
    p.galley(pos2(r.right(), y - note.size().y * 0.5), note, FAINT);
    ui.add_space(16.);
}
/// A settings row: its name at the left, the control at the right.
pub fn row<R>(ui: &mut Ui, label: &str, control: impl FnOnce(&mut Ui) -> R) -> R {
    Frame::new()
        .inner_margin(Margin::symmetric(10, 9))
        .show(ui, |ui| {
            ui.allocate_ui_with_layout(
                vec2(ui.available_width(), 20.),
                Layout::right_to_left(Align::Center),
                |ui| {
                    ui.spacing_mut().item_spacing.x = 10.;
                    let out = control(ui);
                    ui.add_space(6.);
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        ui.add(
                            Label::new(
                                RichText::new(label)
                                    .font(font("UI", 13.5))
                                    .line_height(Some(20.))
                                    .color(TEXT),
                            )
                            .selectable(false)
                            .wrap(),
                        );
                    });
                    out
                },
            )
            .inner
        })
        .inner
}
/// A settings row whose control is too wide to share a line with its name.
pub fn segment_row<'a>(
    ui: &mut Ui,
    label: &str,
    options: &[(&'a str, &str)],
    current: &str,
    tint: Tint,
) -> Option<&'a str> {
    let width = options
        .iter()
        .map(|(_, name)| caps(ui, name, 10.5, 0.12).size().x + 20.)
        .sum::<f32>()
        + 6.;
    let label_width = line(ui, label, "UI", 13.5, 0.).size().x.min(120.);
    if ui.available_width() >= width + label_width + 36. {
        return row(ui, label, |ui| seg(ui, options, current, tint));
    }
    Frame::new()
        .inner_margin(Margin::symmetric(10, 9))
        .show(ui, |ui| {
            ui.label(RichText::new(label).font(font("UI", 13.5)).color(TEXT));
            ui.add_space(8.);
            ui.allocate_ui_with_layout(
                vec2(ui.available_width(), 30.),
                Layout::right_to_left(Align::Center),
                |ui| seg(ui, options, current, tint),
            )
            .inner
        })
        .inner
}
/// A row with a switch. True when the person flipped it.
pub fn switch(ui: &mut Ui, label: &str, on: &mut bool, tint: Tint) -> bool {
    let (r, mut resp) = ui.allocate_exact_size(vec2(ui.available_width(), 38.), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let state = *on;
    resp.widget_info(|| WidgetInfo::selected(Role::CheckBox, true, state, label));
    let p = ui.painter().with_clip_rect(r.intersect(ui.clip_rect()));
    if resp.hovered() || resp.has_focus() {
        p.rect_filled(r, 0, white(0.04));
    }
    p.text(
        pos2(r.left() + 10., r.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        font("UI", 13.5),
        SECONDARY,
    );
    let b = Rect::from_min_size(pos2(r.right() - 44., r.center().y - 8.), vec2(34., 16.));
    let k = ui.ctx().animate_bool_with_time(resp.id, state, 0.2);
    p.rect(
        b,
        0,
        black(0.4).lerp_to_gamma(tint.ink(0.14), k),
        (1., HAIR3.lerp_to_gamma(tint.ink(0.7), k)),
        StrokeKind::Inside,
    );
    let knob = Rect::from_min_size(pos2(b.left() + 3. + 18. * k, b.top() + 3.), vec2(10., 10.));
    if k > 0. {
        glow(&p, knob, 0., 8., tint.ink(0.9 * k));
    }
    p.rect_filled(knob, 0, MUTED.lerp_to_gamma(tint.ink(1.), k));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
}
/// A segmented choice. Returns the key the person picked.
pub fn seg<'a>(
    ui: &mut Ui,
    options: &[(&'a str, &str)],
    current: &str,
    tint: Tint,
) -> Option<&'a str> {
    let texts: Vec<_> = options
        .iter()
        .map(|(_, name)| caps(ui, name, 10.5, 0.12))
        .collect();
    let w: f32 = texts.iter().map(|g| g.size().x + 20.).sum::<f32>() + 6.;
    let (r, _) = ui.allocate_exact_size(vec2(w, 30.), Sense::hover());
    ui.painter()
        .rect(r, 0, black(0.35), (1., HAIR2), StrokeKind::Inside);
    let mut picked = None;
    let mut x = r.left() + 3.;
    for ((key, name), text) in options.iter().zip(texts) {
        let cell = Rect::from_min_size(pos2(x, r.top() + 3.), vec2(text.size().x + 20., 24.));
        x = cell.right();
        let on = *key == current;
        // The name keeps two switches on one panel apart when they share a key (both have "mono").
        let resp = ui.interact(
            cell,
            ui.scope_id().with(("seg", key, *name)),
            Sense::click(),
        );
        resp.widget_info(|| WidgetInfo::selected(Role::RadioButton, true, on, *name));
        let p = ui.painter();
        if on {
            p.rect(
                cell,
                0,
                tint.ink(0.14),
                (1., tint.ink(0.35)),
                StrokeKind::Inside,
            );
        }
        let ink = if on {
            tint.ink(1.)
        } else if resp.hovered() || resp.has_focus() {
            TEXT
        } else {
            MUTED
        };
        let at = pos2(cell.left() + 10., cell.center().y - text.size().y * 0.5);
        if on {
            // the chosen name glows softly, inside its own cell
            let lit = Rect::from_min_size(at, text.size()).expand2(vec2(3., 0.));
            glow(p, lit, 3., 7., tint.ink(0.22));
        }
        p.galley(at, text, ink);
        if resp
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
            && !on
        {
            picked = Some(*key);
        }
    }
    picked
}
/// Small print under a row.
pub fn note(ui: &mut Ui, text: &str) {
    Frame::new()
        .inner_margin(Margin {
            left: 10,
            right: 10,
            top: 0,
            bottom: 12,
        })
        .show(ui, |ui| {
            ui.add(Label::new(mono(text, 10.5).line_height(Some(10.5)).color(FAINT)).wrap());
        });
}
/// Small print with a link at its end. True when the link was clicked.
pub fn note_link(ui: &mut Ui, text: &str, link: &str) -> bool {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.;
        ui.add(Label::new(mono(&format!("{text} · "), 11.).color(FAINT)).selectable(false));
        let resp = ui.add(
            Label::new(mono(link, 11.).color(MUTED).underline())
                .selectable(false)
                .sense(Sense::click()),
        );
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
    })
    .inner
}
pub fn empty_note(ui: &mut Ui, text: &str) {
    let g = line(ui, &text.to_uppercase(), "Mono", 11., 0.08);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 54.), Sense::hover());
    let p = ui.painter();
    let c = [
        r.left_top(),
        r.right_top(),
        r.right_bottom(),
        r.left_bottom(),
        r.left_top(),
    ];
    p.extend(Shape::dashed_line(&c, (1., HAIR2), 4., 3.));
    p.galley(r.center() - g.size() * 0.5, g, FAINT);
}
/// A text box in the HUD look.
pub fn field(ui: &mut Ui, text: &mut String, hint: &str, face: &str, width: f32) -> Response {
    let response = ui.add(
        egui::TextEdit::singleline(text)
            .frame(
                Frame::new()
                    .fill(black(0.5))
                    .inner_margin(Margin::symmetric(13, 0)),
            )
            .font(font(face, 13.5))
            .text_color(TEXT)
            .hint_text(RichText::new(hint).font(font(face, 13.5)).color(FAINT))
            .desired_width(width)
            .min_size(vec2(0., 40.))
            .align(egui::Align2::LEFT_CENTER),
    );
    let tint = super::tint(ui.ctx());
    if response.has_focus() {
        ui.painter().rect_stroke(
            response.rect.expand(1.5),
            0,
            (3., tint.ink(0.08)),
            StrokeKind::Middle,
        );
    }
    ui.painter().rect_stroke(
        response.rect,
        0,
        (
            1.,
            if response.has_focus() {
                tint.ink(0.6)
            } else {
                HAIR2
            },
        ),
        StrokeKind::Inside,
    );
    response
}
/// A small label above a field.
pub fn lbl(ui: &mut Ui, text: &str) {
    let g = caps(ui, text, 10.5, 0.2);
    let (r, _) = ui.allocate_exact_size(vec2(g.size().x, 12.), Sense::hover());
    ui.painter().galley(r.min, g, FAINT);
}
/// A number with its name above and a line below.
pub fn kpi(ui: &mut Ui, width: f32, label: &str, value: &str, sub: &str, size: f32) {
    let (r, _) = ui.allocate_exact_size(vec2(width, 100.), Sense::hover());
    let p = ui.painter().with_clip_rect(r.intersect(ui.clip_rect()));
    p.rect(r, 0, white(0.02), (1., HAIR2), StrokeKind::Inside);
    let inner = r.shrink2(vec2(16., 15.));
    let name = caps(ui, label, 10.5, 0.2);
    p.galley(inner.min, name, FAINT);
    let clip = p.with_clip_rect(inner.intersect(p.clip_rect()));
    let mut job = LayoutJob::simple_singleline(value.into(), font("Mono", size), Color32::WHITE);
    job.wrap = egui::text::TextWrapping::truncate_at_width(inner.width());
    let text = clip.layout_job(job);
    let at = pos2(inner.left(), inner.top() + 36. - text.size().y / 2.);
    clip.add(text_glow(
        text.mesh_bounds.translate(at.to_vec2()),
        18.,
        0.3,
        super::tint(ui.ctx()),
    ));
    clip.galley(at, text, Color32::WHITE);
    clip.text(
        pos2(inner.left(), inner.bottom() - 6.),
        egui::Align2::LEFT_CENTER,
        sub,
        font("Mono", 11.),
        FAINT,
    );
}
/// A dialog over a dimmed page. The second value is true when the person dismissed it.
pub fn dialog<R>(
    ctx: &Context,
    id: &str,
    title: &str,
    tint: Tint,
    add: impl FnOnce(&mut Ui) -> R,
) -> (R, bool) {
    let m = egui::Modal::new(Id::unique(id))
        .backdrop_color(black(0.62))
        .frame(
            Frame::new()
                .fill(Color32::from_rgb(8, 8, 10))
                .stroke((1., HAIR3))
                .corner_radius(CornerRadius::ZERO)
                .inner_margin(Margin::symmetric(24, 22)),
        )
        .show(ctx, |ui| {
            ui.set_width(412.);
            let t = caps(ui, title, 13., 0.24);
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 14.), Sense::hover());
            ui.painter()
                .with_clip_rect(r.expand(2.))
                .galley(r.min, t, tint.ink(1.));
            ui.add_space(10.);
            add(ui)
        });
    let closed = m.should_close();
    (m.inner, closed)
}
/// A yes-or-no question. `Some(true)` for yes, `Some(false)` for no, `None` while it waits.
pub fn ask(
    ctx: &Context,
    title: &str,
    text: &str,
    ok: &str,
    danger: bool,
    tint: Tint,
) -> Option<bool> {
    let (answer, closed) = dialog(ctx, "ask", title, tint, |ui| {
        ui.add(Label::new(body(text)).wrap());
        ui.add_space(14.);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let kind = if danger { Kind::Danger } else { Kind::Primary };
            if Btn::new(ok, tint).kind(kind).show(ui).clicked()
                || ui.input(|i| i.key_pressed(Key::Enter))
            {
                Some(true)
            } else if Btn::new("Cancel", tint)
                .kind(Kind::Ghost)
                .show(ui)
                .clicked()
            {
                Some(false)
            } else {
                None
            }
        })
        .inner
    });
    answer.or(closed.then_some(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hover_and_selection_paint_the_hud_switch_and_segment_states() {
        let ctx = Context::default();
        crate::theme::install(&ctx);
        let tint = Tint::Mono;
        let frame = |time, pointer, mut on, selected| {
            let options = [("a", "First"), ("b", "Second")];
            let out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(300., 120.))),
                    time: Some(time),
                    events: vec![egui::Event::PointerMoved(pointer)],
                    ..Default::default()
                },
                |ui| {
                    switch(ui, "Example", &mut on, tint);
                    seg(ui, &options, selected, tint);
                },
            );
            let mut row_hovered = false;
            let mut thumb = None;
            let mut second = None;
            for clipped in &out.shapes {
                match &clipped.shape {
                    Shape::Rect(r) if r.rect.height() == 38. && r.fill == white(0.04) => {
                        row_hovered = true
                    }
                    Shape::Rect(r) if r.rect.size() == vec2(10., 10.) => {
                        thumb = Some((r.rect.left(), r.fill))
                    }
                    Shape::Text(t) if t.galley.job.text == "SECOND" => {
                        second = Some((t.pos, t.fallback_color))
                    }
                    _ => {}
                }
            }
            out.drop_without_applying_deltas();
            (row_hovered, thumb.unwrap(), second.unwrap())
        };
        let outside = pos2(290., 110.);
        let (hovered, off, second) = frame(0., outside, false, "a");
        assert!(!hovered);
        assert_eq!(off.1, MUTED);
        assert_eq!(second.1, MUTED);
        assert!(frame(0.1, pos2(12., 15.), false, "a").0);
        let over_second = second.0 + vec2(2., 5.);
        assert_eq!(frame(0.2, over_second, true, "a").2.1, TEXT);
        let (_, on, selected) = frame(1., over_second, true, "b");
        assert_eq!(on, (off.0 + 18., tint.ink(1.)));
        assert_eq!(selected.1, tint.ink(1.));
    }
}
