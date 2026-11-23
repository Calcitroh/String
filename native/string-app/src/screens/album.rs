use super::{
    detail::{self, Hero},
    library::{self, Index, count, duration},
};
use crate::{model::Models, route::Route, ui::songs::Columns, view::View};
use egui::Ui;
fn current_album<'a>(
    albums: &'a [library::Album],
    route: &mut Route,
    models: &Models,
) -> Option<&'a library::Album> {
    if let Route::Album(key) = route {
        let resolved = models.catalog.resolve_album_key(key);
        // Keep the saved key until the scan has an album for it. The page and its
        // playback controls can then share the same catalog key.
        if resolved != key && models.album_info.contains_key(resolved) {
            *key = resolved.to_owned();
        }
    }
    albums.iter().find(|a| a.card.is(route))
}
pub fn show(ui: &mut Ui, v: &mut View<'_>) {
    let index = Index::get(ui, v);
    let Some(a) = current_album(&index.albums, &mut v.views.history.current, &v.s.models) else {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::cards::{Art, Card};
    use string_core::library::{Library, Track};

    #[test]
    fn restored_drive_root_route_opens_the_album_when_the_catalog_is_ready() {
        let track = Track {
            path: r"F:\Song.mp3".into(),
            tags: string_core::tags::Tags {
                album: "Record".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut library = Library::default();
        library.tracks.push(track.clone());
        let album_cards = |m: &Models| {
            m.catalog
                .albums
                .iter()
                .map(|g| library::Album {
                    card: Card::page(
                        Route::Album(g.key.clone()),
                        &g.name,
                        String::new(),
                        Art::Cover(Default::default()),
                    ),
                    artist: String::new(),
                    artist_key: String::new(),
                    year: 0,
                    added: 0,
                    duration: 0.,
                })
                .collect::<Vec<_>>()
        };
        let old = Route::parse("album/record%01f%3A%5C");
        let mut route = old.clone();
        assert!(current_album(&[], &mut route, &Models::default()).is_none());
        assert_eq!(route, old);
        let models = Models::build(&library, &Default::default());
        let cards = album_cards(&models);
        assert!(current_album(&cards, &mut route, &models).is_some());
        assert_eq!(route.path(), "album/record%01f%3A");
        // A literal album-artist credit can have exactly the old key.
        let mut exact = track;
        exact.path = r"F:\Other.mp3".into();
        exact.tags.album_artist = r"F:\".into();
        library.tracks.push(exact);
        let models = Models::build(&library, &Default::default());
        let mut route = old.clone();
        assert!(current_album(&album_cards(&models), &mut route, &models).is_some());
        assert_eq!(route, old);
    }
}
