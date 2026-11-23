use crate::{
    theme::*,
    ui::{self, area, child, covers::Size},
    view::{Overlay, View},
};
use egui::{Align, Layout, Rect, RichText, Ui, pos2, vec2};
pub fn show(ui: &mut Ui, v: &mut View<'_>) {
    let r = ui.max_rect();
    let t = v.tint;
    let current = v.s.current().cloned();
    let art = current
        .as_ref()
        .and_then(|s| v.covers.get(&s.cover, Size::Large, v.mono.then_some(t)));
    let p = ui.painter().clone();
    p.rect_filled(r, 0, BG);
    if let Some(bg) = art.as_ref().and_then(|a| a.ambient.as_ref()) {
        ui::image(&p, r, bg);
    }
    let grid = egui::Color32::from_white_alpha(4);
    for i in 0..=(r.width() / 32.) as usize {
        p.vline(r.left() + i as f32 * 32., r.y_range(), (0.6, grid));
    }
    for i in 0..=(r.height() / 32.) as usize {
        p.hline(r.x_range(), r.top() + i as f32 * 32., (0.6, grid));
    }
    let lyrics = v.views.overlay == Some(Overlay::Lyrics);
    let split = if lyrics {
        r.left() + 86. + (r.width() - 142.) * 0.36
    } else {
        r.left() + r.width() * 0.54
    };
    let c = pos2(
        (split + r.left() + 24.) / 2.,
        r.center().y + if lyrics { 14. } else { 0. },
    );
    let radius = if lyrics {
        (split - r.left() - 86.).min((r.height() + BAR) * 0.50) * 0.35
    } else {
        (split - r.left() - 100.).min((r.height() + BAR) * 0.64) * 0.35
    }
    .max(35.);
    if let Some(tex) = art.as_ref().and_then(|a| a.record.as_ref()) {
        v.viz.paint(&p, c, radius, tex.id());
    } else {
        p.circle(c, radius, PANEL2, (1., HAIR2));
        p.circle_filled(c, radius * 0.12, BG);
    }
    let mut controls = child(
        ui,
        area(r.right() - 213., r.top() + 14., 207., 33.),
        "np tabs",
    );
    controls.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.;
        if ui::seg(ui, "RECORD", !lyrics).clicked() {
            v.views.overlay = Some(Overlay::Record);
        }
        if ui::seg(ui, "LYRICS", lyrics).clicked() {
            v.views.overlay = Some(Overlay::Lyrics);
        }
        let (b, _) = ui.allocate_exact_size(vec2(30., 26.), egui::Sense::hover());
        if ui::icon_btn(ui, b, "down", "Close Now Playing", false, t).clicked() {
            v.views.overlay = None;
        }
    });
    let Some(song) = current else {
        let mut info = child(
            ui,
            area(split, c.y - 40., r.right() - split - 25., 120.),
            "empty record",
        );
        info.heading("Nothing playing");
        return;
    };
    let width = (r.right() - split - 46.).max(140.);
    let top = if lyrics {
        r.top() + 68.
    } else {
        (c.y - 174.).max(r.top() + 49.)
    };
    let mut info = child(
        ui,
        area(split, top, width, r.bottom() - top - 12.),
        "record info",
    );
    if lyrics {
        super::lyrics::show(&mut info, v);
        return;
    }
    let context = v.s.state.queue.context(None)["name"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or("Library");
    info.label(hud(&format!("PLAYING FROM {}", context.to_uppercase()), 11.).color(t.ink(0.9)));
    info.add_space(2.);
    info.add(
        egui::Label::new(
            RichText::new(&song.title).font(font("Bold", (width * 0.092).clamp(30., 54.))),
        )
        .wrap(),
    );
    info.label(
        RichText::new(song.artist_name())
            .font(font("UI", 18.))
            .color(SECONDARY),
    );
    info.add_space(2.);
    let mut meta = vec![];
    if !song.album.is_empty() {
        meta.push(song.album.clone())
    }
    if song.year > 0 {
        meta.push(song.year.to_string())
    }
    meta.push(format!(
        "{} / {}",
        time(v.st.position),
        time(v.st.duration.unwrap_or(song.duration))
    ));
    if !song.format.is_empty() {
        meta.push(song.format.clone());
    }
    info.label(mono(&meta.join(" · "), 12.).color(MUTED));
    info.add_space(13.);
    info.horizontal(|ui| {
        for (icon, help, on) in [
            (
                "heart",
                "Like song",
                !v.s.field("liked")[&song.id].is_null(),
            ),
            ("plus", "Add to playlist", false),
            ("queue", "Open queue", v.views.queue),
            ("more", "Song actions", false),
        ] {
            let (r, _) = ui.allocate_exact_size(vec2(31., 32.), egui::Sense::hover());
            let resp = if icon == "heart" {
                ui::like_btn(ui, r, help, on, t)
            } else {
                ui::icon_btn(ui, r, icon, help, on, t)
            };
            match icon {
                "heart" if resp.clicked() => v.like_current(),
                "queue" if resp.clicked() => v.views.queue = !v.views.queue,
                "plus" => {
                    egui::Popup::menu(&resp).show(|ui| {
                        if ui.button("New playlist").clicked() {
                            v.views.adding_playlist = true;
                            ui.close();
                        }
                        let mut add = None;
                        for p in &v.s.models.playlists {
                            if ui.button(&p.name).clicked() {
                                add = Some(p.id.clone());
                                ui.close();
                            }
                        }
                        if let Some(p) = add {
                            v.s.add_to_playlist(&p, vec![song.id.clone()].into());
                        }
                    });
                }
                "more" => {
                    egui::Popup::menu(&resp).show(|ui| {
                        if ui.button("Show in songs").clicked() {
                            v.jump_current();
                            ui.close();
                        }
                        if ui.button("Play next").clicked() {
                            v.s.insert(v.player, std::slice::from_ref(&song.id), false);
                            ui.close();
                        }
                    });
                }
                _ => {}
            }
        }
    });
    info.add_space(15.);
    info.separator();
    info.add_space(5.);
    info.label(hud("UP NEXT", 12.));
    let mut jump = None;
    let first = v.s.state.queue.at().map_or(0, |i| i + 1);
    for (i, item) in v.s.state.queue.upcoming().iter().take(2).enumerate() {
        if let Some(next) = v.s.models.by_id.get(&item.id) {
            let (row, resp) = info.allocate_exact_size(vec2(width, 39.), egui::Sense::click());
            let cover = Rect::from_min_size(row.min, vec2(36., 36.));
            ui::sleeve(
                info.painter(),
                cover,
                v.covers.get(&next.cover, Size::Small, v.mono.then_some(t)),
            );
            let x = row.left() + 47.;
            ui::label(
                info.painter(),
                area(x, row.top(), width - 48., 19.),
                &next.title,
                "Strong",
                14.,
                TEXT,
            );
            ui::label(
                info.painter(),
                area(x, row.top() + 19., width - 48., 15.),
                next.artist_name(),
                "UI",
                12.,
                MUTED,
            );
            if resp.clicked() {
                jump = Some(first + i);
            }
        }
    }
    if let Some(i) = jump {
        v.jump(i);
    }
    if v.s.state.queue.upcoming().is_empty() {
        info.with_layout(Layout::left_to_right(Align::Center), |ui| {
            ui.label(RichText::new("Queue is empty").color(MUTED))
        });
    }
}
