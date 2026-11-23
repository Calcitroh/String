use super::{
    cards,
    library::{self, Index, count, section},
};
use crate::{
    model::{Ids, norm},
    ui::songs::Columns,
    view::View,
};
use egui::{Id, Ui};
use serde_json::json;
struct Results {
    tracks: Ids,
    albums: Vec<usize>,
    artists: Vec<usize>,
    playlists: Vec<usize>,
}
fn results(ui: &Ui, v: &View<'_>, index: &Index) -> std::sync::Arc<Results> {
    let query = v.views.query.trim();
    library::cached(
        ui,
        "library search",
        Id::unique((v.s.generation, query)),
        || {
            let q = norm(query);
            let matches = |name: &str| {
                let name = norm(name);
                q.split_whitespace().all(|s| name.contains(s))
            };
            Results {
                tracks: v.s.models.search(query),
                albums: index
                    .albums
                    .iter()
                    .enumerate()
                    .filter(|(_, a)| matches(&format!("{} {}", a.card.title, a.artist)))
                    .map(|(i, _)| i)
                    .collect(),
                artists: index
                    .artists
                    .iter()
                    .enumerate()
                    .filter(|(_, a)| {
                        !a.card.is(&crate::route::Route::Artist(String::new()))
                            && matches(&a.card.title)
                    })
                    .map(|(i, _)| i)
                    .collect(),
                playlists: index
                    .playlists
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| matches(&p.title))
                    .map(|(i, _)| i)
                    .collect(),
            }
        },
    )
}
fn context(q: &str) -> serde_json::Value {
    json!({"kind":"search","key":q,"name":format!("“{q}”")})
}
pub fn enter(ui: &Ui, v: &mut View<'_>) {
    if !v.views.query.trim().is_empty() {
        let r = results(ui, v, &Index::get(ui, v));
        if !r.tracks.is_empty() {
            v.s.play_ids(v.player, &r.tracks, 0, context(v.views.query.trim()));
        }
    }
}
pub fn show(ui: &mut Ui, v: &mut View<'_>) {
    let query = v.views.query.trim().to_owned();
    if query.is_empty() {
        library::heading(
            ui,
            "Search",
            "Type above to find songs, albums, artists and playlists",
        );
        return;
    }
    let index = Index::get(ui, v);
    let r = results(ui, v, &index);
    let meta = if r.tracks.is_empty()
        && r.albums.is_empty()
        && r.artists.is_empty()
        && r.playlists.is_empty()
    {
        "Nothing matches".into()
    } else {
        format!(
            "{} · {} · {}",
            count(r.tracks.len(), "song"),
            count(r.albums.len(), "album"),
            count(r.artists.len(), "artist")
        )
    };
    egui::ScrollArea::vertical().show(ui, |ui| {
        library::heading(ui, &format!("“{query}”"), &meta);
        cards::row(
            ui,
            v,
            "Artists",
            r.artists.iter().take(14).map(|&i| &index.artists[i].card),
            None,
            None,
        );
        cards::row(
            ui,
            v,
            "Albums",
            r.albums.iter().take(14).map(|&i| &index.albums[i].card),
            None,
            None,
        );
        cards::row(
            ui,
            v,
            "Playlists",
            r.playlists.iter().map(|&i| &index.playlists[i]),
            None,
            None,
        );
        if !r.tracks.is_empty() {
            section(ui, "Songs", None, v);
            super::track_rows(ui, v, &r.tracks, context(&query), Columns::Plain);
        }
    });
}
