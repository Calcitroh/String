use crate::{route::Route, smoke::Case, ui::covers::Size, view::View};
use serde_json::json;
fn covers(v: &mut View<'_>, size: Size) -> bool {
    v.s.models.songs.iter().take(14).all(|id| {
        v.covers
            .get(&v.s.models.by_id[id].cover, size, v.mono.then_some(v.tint))
            .is_some()
    })
}
pub const HOME: Case = Case {
    name: "03-home",
    prepare: |v| v.go(Route::Home),
    ready: |v| v.views.history.current == Route::Home && covers(v, Size::Medium),
};
pub const ALBUMS: Case = Case {
    name: "05-albums",
    prepare: |v| v.go(Route::Albums),
    ready: |v| v.views.history.current == Route::Albums && covers(v, Size::Medium),
};
pub const ARTISTS: Case = Case {
    name: "06-artists",
    prepare: |v| v.go(Route::Artists),
    ready: |v| v.views.history.current == Route::Artists && covers(v, Size::Medium),
};
fn album(v: &mut View<'_>) {
    if let Some(a) = v.s.models.catalog.albums.iter().max_by_key(|a| a.ids.len()) {
        v.go(Route::Album(a.key.clone()));
    }
}
fn hero_ready(v: &mut View<'_>) -> bool {
    let ids = v.s.models.context_ids(&v.context());
    let Some(id) = ids.first() else {
        return false;
    };
    v.covers
        .get(
            &v.s.models.by_id[id].cover,
            Size::Large,
            v.mono.then_some(v.tint),
        )
        .is_some()
        && covers(v, Size::Small)
}
pub const ALBUM: Case = Case {
    name: "07-album",
    prepare: album,
    ready: hero_ready,
};
pub const ARTIST: Case = Case {
    name: "07b-artist",
    prepare: |v| {
        if let Some(a) =
            v.s.models
                .catalog
                .artists
                .iter()
                .filter(|a| !a.key.is_empty())
                .max_by_key(|a| a.ids.len())
        {
            v.go(Route::Artist(a.key.clone()));
        } else {
            v.go(Route::Artist(String::new()));
        }
    },
    ready: hero_ready,
};
pub const LIKED: Case = Case {
    name: "12-liked",
    prepare: |v| {
        if v.s.models.liked.is_empty()
            && let Some(id) = v.s.models.songs.first().cloned()
        {
            v.s.toggle_like(&[id]);
        }
        v.go(Route::Liked);
    },
    ready: |v| {
        v.views.history.current == Route::Liked
            && !v.s.models.liked.is_empty()
            && covers(v, Size::Small)
    },
};
pub const SEARCH: Case = Case {
    name: "13-search",
    prepare: |v| {
        let id =
            v.s.models
                .songs
                .iter()
                .find(|id| v.s.models.by_id[*id].title.contains("Orbit"))
                .or_else(|| v.s.models.songs.first())
                .unwrap();
        let title = &v.s.models.by_id[id].title;
        v.views.query = title
            .split_whitespace()
            .next_back()
            .unwrap_or(title)
            .to_lowercase();
        v.go(Route::Search);
    },
    ready: |v| !v.s.models.search(&v.views.query).is_empty(),
};
pub const SEARCH_GROUPS: Case = Case {
    name: "13b-search-groups",
    prepare: |v| {
        v.views.query =
            v.s.models
                .catalog
                .artists
                .iter()
                .filter(|a| !a.key.is_empty())
                .max_by_key(|a| a.ids.len())
                .map(|a| a.name.clone())
                .unwrap_or_else(|| v.views.query.clone());
        v.go(Route::Search);
    },
    ready: |v| !v.s.models.search(&v.views.query).is_empty() && covers(v, Size::Medium),
};
pub const HOME_PLAYING: Case = Case {
    name: "26-home-playing",
    prepare: |v| {
        v.views.query.clear();
        let records = &v.s.models.catalog.albums;
        let (ids, ctx) = if let Some(a) = records.iter().max_by_key(|a| a.ids.len()) {
            (
                a.ids.clone(),
                json!({"kind":"album","key":a.key,"name":a.name}),
            )
        } else {
            (
                v.s.models.songs.to_vec(),
                json!({"kind":"songs","key":"","name":"All songs"}),
            )
        };
        let now = chrono::Local::now().timestamp_millis();
        let mut plays = v.s.field("plays").clone();
        for (i, id) in ids.iter().take(3).enumerate() {
            plays[id] = json!([3 - i, now - i as i64 * 1000]);
        }
        v.s.set_field("plays", plays);
        if v.s.models.playlists.is_empty() {
            v.s.add_playlist("Smoke mix".into(), &ids[..1]);
        }
        v.s.play_ids(v.player, &ids, 0, ctx);
        v.go(Route::Home);
    },
    ready: |v| v.st.playing && v.st.position > 0.15 && covers(v, Size::Medium),
};
pub fn skip(name: &str, v: &View<'_>) -> bool {
    name == ALBUM.name && v.s.models.catalog.albums.is_empty()
}
