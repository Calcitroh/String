use super::{
    cards,
    detail::{self, Hero},
    library::{self, Index, count, section},
};
use crate::{model::Ids, ui::songs::Columns, view::View};
use egui::{Id, Ui};
use std::cmp::Reverse;
pub fn show(ui: &mut Ui, v: &mut View<'_>) {
    let index = Index::get(ui, v);
    let Some(a) = index
        .artists
        .iter()
        .find(|a| a.card.is(&v.views.history.current))
    else {
        library::note(ui, "That artist isn't in the library any more");
        return;
    };
    let ids = detail::group_ids(ui, v);
    let mut meta = count(ids.len(), "song");
    if !a.albums.is_empty() {
        meta.push_str(&format!(" · {}", count(a.albums.len(), "album")));
    }
    let plays = v.s.field("plays");
    let rank_key = Id::unique((
        v.views.history.current.path(),
        v.s.generation,
        v.s.plays_revision,
    ));
    let popular = library::cached(ui, "artist popular tracks", rank_key, || {
        let mut result = ids.to_vec();
        result.sort_by_key(|id| Reverse(plays[id][0].as_u64().unwrap_or(0)));
        result.truncate(5);
        Ids::from(result)
    });
    let heading = if popular
        .first()
        .is_some_and(|id| plays[id][0].as_u64().unwrap_or(0) > 0)
    {
        "Popular"
    } else {
        "Songs"
    };
    egui::ScrollArea::vertical().show(ui, |ui| {
        detail::hero(
            ui,
            v,
            Hero {
                label: "Artist",
                title: &a.card.title,
                meta: &meta,
                artist: None,
                art: &a.card.art,
                round: true,
            },
        );
        detail::actions(ui, v, &ids, true, None);
        section(ui, heading, None, v);
        let ctx = v.context();
        ui.push_id("popular tracks", |ui| {
            std::mem::swap(
                &mut v.views.artist_popular_selection,
                &mut v.views.selection,
            );
            super::track_rows(ui, v, &popular, ctx.clone(), Columns::Popular);
            std::mem::swap(
                &mut v.views.artist_popular_selection,
                &mut v.views.selection,
            );
        });
        if !a.albums.is_empty() {
            section(ui, "Albums", None, v);
            cards::grid(
                ui,
                v,
                a.albums.iter().map(|&i| &index.albums[i].card),
                false,
            );
        }
        if ids.len() > 5 {
            section(ui, "All songs", None, v);
            super::track_rows(ui, v, &ids, ctx, Columns::Plain);
        }
    });
}
