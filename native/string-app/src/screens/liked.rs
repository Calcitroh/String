use super::{
    cards::Art,
    detail::{self, Hero},
    library::{self, count, duration},
};
use crate::{ui::songs::Columns, view::View};
use egui::{Id, Ui};
pub fn show(ui: &mut Ui, v: &mut View<'_>) {
    let all = v.s.models.liked.clone();
    let meta = library::cached(ui, "liked total", Id::unique(v.s.generation), || {
        if all.is_empty() {
            "Songs you like show up here".into()
        } else {
            format!(
                "{}, {}",
                count(all.len(), "song"),
                duration(all.iter().map(|id| v.s.models.by_id[id].duration).sum())
            )
        }
    });
    egui::ScrollArea::vertical().show(ui, |ui| {
        detail::hero(
            ui,
            v,
            Hero {
                label: "Playlist",
                title: "Liked songs",
                meta: &meta,
                artist: None,
                art: &Art::Heart,
                round: false,
            },
        );
        let list = !all.is_empty() || !v.views.filter.is_empty();
        let ids = detail::actions(ui, v, &all, false, list.then_some("Filter liked songs"));
        if list {
            super::track_rows(ui, v, &ids, v.context(), Columns::Liked);
            if ids.is_empty() {
                library::note(ui, "No songs match");
            }
        } else {
            library::note(
                ui,
                "Press ♥ on a song (or L while it plays) to keep it here",
            );
        }
    });
}
