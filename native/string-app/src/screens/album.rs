use super::{
    detail::{self, Hero},
    library::{self, Index, count, duration},
};
use crate::{ui::songs::Columns, view::View};
use egui::Ui;
pub fn show(ui: &mut Ui, v: &mut View<'_>) {
    let index = Index::get(ui, v);
    let Some(a) = index
        .albums
        .iter()
        .find(|a| a.card.is(&v.views.history.current))
    else {
        library::note(ui, "That album isn't in the library any more");
        return;
    };
    let ids = detail::group_ids(ui, v);
    let year = if a.year != 0 {
        format!(" · {}", a.year)
    } else {
        String::new()
    };
    let meta = format!(
        "{year} · {}, {}",
        count(ids.len(), "song"),
        duration(a.duration)
    );
    egui::ScrollArea::vertical().show(ui, |ui| {
        detail::hero(
            ui,
            v,
            Hero {
                label: "Album",
                title: &a.card.title,
                meta: &meta,
                artist: Some((&a.artist, &a.artist_key)),
                art: &a.card.art,
                round: false,
            },
        );
        detail::actions(ui, v, &ids, true, None);
        super::track_rows(ui, v, &ids, v.context(), Columns::Album);
    });
}
