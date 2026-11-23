//! The square field and dark option list used by the Library page.
use super::*;
use egui::{Popup, PopupCloseBehavior, ScrollArea, text::TextWrapping};

pub fn show<'a>(
    ui: &mut Ui,
    label: &str,
    current: &str,
    options: &[(&'a str, &str)],
    tint: Tint,
) -> (Response, Option<&'a str>) {
    let selected = options
        .iter()
        .find(|(key, _)| *key == current)
        .map_or(current, |(_, text)| *text);
    // Flex the label when that lets the complete selection fit. Even the narrowest
    // panel leaves room for the label's longest word and the dropdown arrow.
    let name_width = line(ui, label, "UI", 13.5, 0.).size().x.min(120.);
    let word_width = label
        .split_whitespace()
        .map(|s| line(ui, s, "UI", 13.5, 0.).size().x)
        .fold(0., f32::max);
    let room = ui.available_width() - 16.;
    let natural = line(ui, selected, "UI", 12.5, 0.).size().x + 28.;
    let width = (room - name_width)
        .max(natural.min(room - word_width))
        .clamp(64., 220.);
    let (rect, _) = ui.allocate_exact_size(vec2(width, 32.), Sense::hover());
    let response = ui
        .interact(rect, Id::unique(("settings select", label)), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    response.widget_info(|| {
        let mut info = WidgetInfo::labeled(Role::ComboBox, true, label);
        info.current_text_value = Some(selected.to_owned());
        info
    });
    let popup_id = Popup::default_response_id(&response);
    if response.has_focus() && ui.input(|i| i.key_pressed(Key::ArrowDown)) {
        Popup::open_id(ui.ctx(), popup_id);
    }
    let popup = Popup::menu(&response)
        .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
        .width(width)
        .frame(Frame::new().fill(PANEL).stroke((1., HAIR3)));
    let open = popup.is_open();
    let focused = response.has_focus() || open;
    let p = ui.painter();
    if focused {
        p.rect_stroke(
            rect.expand(1.5),
            0,
            (3., tint.ink(0.08)),
            StrokeKind::Middle,
        );
    }
    p.rect(
        rect,
        0,
        black(0.5),
        (1., if focused { tint.ink(0.6) } else { HAIR2 }),
        StrokeKind::Inside,
    );
    let mut job = LayoutJob::simple_singleline(selected.into(), font("UI", 12.5), TEXT);
    job.wrap = TextWrapping::truncate_at_width((width - 28.).max(1.));
    let text = p.layout_job(job);
    p.galley(
        pos2(rect.left() + 9., rect.center().y - text.size().y / 2.),
        text,
        TEXT,
    );
    let arrow = pos2(rect.right() - 9., rect.center().y);
    p.add(Shape::line(
        vec![
            arrow + vec2(-4., -2.),
            arrow + vec2(0., 2.),
            arrow + vec2(4., -2.),
        ],
        (1.6, TEXT),
    ));

    let mut picked = None;
    let menu = popup.show(|ui| {
        // A long installed model name may need more room in the menu than in the field.
        let room = ui.ctx().viewport_rect().width();
        let menu_width = options
            .iter()
            .map(|(_, s)| line(ui, s, "UI", 12.5, 0.).size().x + 26.)
            .fold(width - 2., f32::max)
            .min(room - 12.);
        ui.set_width(menu_width);
        ui.spacing_mut().item_spacing.y = 0.;
        ScrollArea::vertical().max_height(256.).show(ui, |ui| {
            for &(key, name) in options {
                let (_, r) = ui.allocate_space(vec2(ui.available_width(), 24.));
                let entry = ui
                    .interact(r, response.id.with(key), Sense::click())
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                entry.widget_info(|| {
                    WidgetInfo::selected(Role::ListBoxOption, true, key == current, name)
                });
                let hot = entry.hovered() || entry.has_focus();
                if hot || key == current {
                    ui.painter()
                        .rect_filled(r, 0, if hot { GHOST } else { RAISED });
                }
                ui.painter()
                    .with_clip_rect(r.intersect(ui.clip_rect()))
                    .text(
                        pos2(r.left() + 8., r.center().y),
                        egui::Align2::LEFT_CENTER,
                        name,
                        font("UI", 12.5),
                        TEXT,
                    );
                if entry.gained_focus() {
                    entry.scroll_to_me(None);
                }
                if entry.clicked() {
                    picked = Some(key);
                    ui.close();
                    response.request_focus();
                }
            }
        });
    });
    if menu.is_some() {
        ui.ctx()
            .accesskit_node_builder(popup_id, |node| node.set_role(Role::ListBox));
        // The player handles Escape before drawing its pages. It also dismisses an open field.
        if ui.input(|i| {
            i.raw.events.iter().any(|event| {
                matches!(
                    event,
                    egui::Event::Key {
                        key: Key::Escape,
                        pressed: true,
                        ..
                    }
                )
            })
        }) {
            Popup::close_id(ui.ctx(), popup_id);
            response.request_focus();
        }
    }
    (response.on_hover_text(selected), picked)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, Modifiers, PointerButton, Pos2, RawInput};

    #[test]
    fn model_field_fits_its_row_and_selects_the_automatic_value() {
        let ctx = Context::default();
        crate::theme::install(&ctx);
        let label = "Translation model";
        let id = Id::unique(("settings select", label));
        let options = [
            ("", "Best downloaded (automatic)"),
            ("sample", "Sample model"),
        ];
        let frame = |width, events| {
            let mut pick = None;
            let mut bounds = Rect::NOTHING;
            ctx.run_ui(
                RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(width, 360.))),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let (field, value) = row(ui, label, |ui| {
                        show(ui, label, "sample", &options, Tint::Mono)
                    });
                    bounds = field.rect;
                    pick = value;
                },
            )
            .drop_without_applying_deltas();
            (bounds, pick)
        };
        let (wide, _) = frame(540., vec![]);
        assert_eq!(wide.size(), vec2(220., 32.));
        let (narrow, _) = frame(280., vec![]);
        assert!(narrow.left() > 90. && narrow.right() <= 270.);
        let button = |pos, pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        let field = narrow.center();
        frame(280., vec![Event::PointerMoved(field)]);
        frame(280., vec![button(field, true)]);
        frame(280., vec![button(field, false)]);
        frame(280., vec![]);
        // The first menu pass measures the list. Use its interactive placement after the
        // popup has moved left to fit the full automatic option against the viewport edge.
        frame(280., vec![]);
        let entry = ctx.read_response(id.with("")).unwrap();
        assert!(entry.enabled() && entry.rect.top() >= narrow.bottom());
        let automatic = entry.rect.center();
        frame(280., vec![Event::PointerMoved(automatic)]);
        frame(280., vec![button(automatic, true)]);
        assert_eq!(frame(280., vec![button(automatic, false)]).1, Some(""));
        assert!(!Popup::is_id_open(
            &ctx,
            Popup::default_response_id(&ctx.read_response(id).unwrap())
        ));
    }
}
