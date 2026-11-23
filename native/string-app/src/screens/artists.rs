use super::{
    cards, home,
    library::{self, Index, count},
};
use crate::view::View;
use egui::Ui;
pub fn show(ui: &mut Ui, v: &mut View<'_>) {
    if home::empty(ui, v) {
        return;
    }
    let index = Index::get(ui, v);
    egui::ScrollArea::vertical().show(ui, |ui| {
        let named =
            v.s.models
                .catalog
                .artists
                .iter()
                .filter(|a| !a.key.is_empty())
                .count();
        library::heading(ui, "Artists", &count(named, "artist"));
        ui.add_space(14.);
        cards::grid(ui, v, index.artists.iter().map(|a| &a.card), false);
    });
}
