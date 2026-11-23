use super::{
    cards, home,
    library::{self, Index, count},
};
use crate::{model::norm, theme::*, ui, view::View};
use egui::{Id, Ui};
use std::cmp::Reverse;

pub fn show(ui: &mut Ui, v: &mut View<'_>) {
    if home::empty(ui, v) {
        return;
    }
    let index = Index::get(ui, v);
    let mode =
        v.s.field("albumSort")
            .as_str()
            .unwrap_or("artist")
            .to_owned();
    let order = library::cached(
        ui,
        "album order",
        Id::unique((v.s.generation, &mode)),
        || {
            let mut order: Vec<_> = (0..index.albums.len()).collect();
            let albums = &index.albums;
            match mode.as_str() {
                "name" => {}
                "recent" => order.sort_by_key(|&i| Reverse(albums[i].added)),
                "year" => order.sort_by_key(|&i| Reverse(albums[i].year)),
                _ => order.sort_by_cached_key(|&i| (norm(&albums[i].artist), albums[i].year)),
            }
            order
        },
    );
    egui::ScrollArea::vertical().show(ui, |ui| {
        let r = library::title_bar(ui, 30., "Albums", &count(order.len(), "album"), 236.);
        let bar = ui::area(r.right() - 227., r.top(), 227., 30.);
        ui.painter()
            .rect_stroke(bar, 0, (1., HAIR2), egui::StrokeKind::Inside);
        let mut x = bar.left() + 4.;
        for (key, name, width) in [
            ("artist", "ARTIST", 60.),
            ("name", "A–Z", 43.),
            ("recent", "RECENT", 64.),
            ("year", "YEAR", 50.),
        ] {
            let rect = ui::area(x, bar.top() + 3., width, 24.);
            let resp = ui::hit(ui, rect, key, name);
            let selected = mode == key;
            if selected || resp.hovered() {
                ui.painter()
                    .rect(rect, 0, RAISED, (1., HAIR3), egui::StrokeKind::Inside);
            }
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                name,
                font("HUD", 11.),
                if selected { TEXT } else { MUTED },
            );
            if resp.clicked() {
                v.s.set_field("albumSort", key.into());
            }
            x += width;
        }
        ui.add_space(15.);
        if order.is_empty() {
            library::note(ui, "No albums: songs with an album tag show up here");
        }
        cards::grid(ui, v, order.iter().map(|&i| &index.albums[i].card), false);
    });
}
