use super::cards::{Art, Card, Target};
use crate::{
    model::{CoverSource, Ids, norm},
    playback::Session,
    route::Route,
    theme::*,
    ui,
    view::View,
};
use egui::{Align, Align2, Color32, Id, Rect, Ui, vec2};
use serde_json::{Value, json};
use std::{
    cmp::Reverse,
    collections::{HashMap, HashSet},
    sync::Arc,
};

/// "1,196 songs", as the web version's `plural` (a thousands comma, as toLocaleString writes in English).
pub fn count(n: usize, unit: &str) -> String {
    let digits = n.to_string();
    let mut grouped = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    format!("{grouped} {unit}{}", if n == 1 { "" } else { "s" })
}
pub fn duration(s: f64) -> String {
    let s = s.round().max(0.) as u64;
    if s >= 3600 {
        format!("{} hr {} min", s / 3600, s % 3600 / 60)
    } else if s >= 60 {
        format!("{} min {} sec", s / 60, s % 60)
    } else {
        format!("{s} sec")
    }
}
/// A page title in letter-spaced HUD capitals with the web version's soft halo, and its detail beside it.
pub fn heading(ui: &mut Ui, title: &str, detail: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 14.;
        let (at, galley, _) = egui::Label::new(
            hud(&title.to_uppercase(), 22.)
                .extra_letter_spacing(6.6)
                .color(TEXT),
        )
        .layout_in_ui(ui);
        let ink = galley.mesh_bounds.translate(at.to_vec2());
        // The title sits at the very top of the scrolling page, whose edge would cut the haze into a flat-topped
        // band that reads as a highlight; like the web version's, it spreads past it (not into the sidebar, which
        // is 29 px to the left; the player bar and queue are drawn over it).
        // (A painter only ever narrows its clip, so this is a fresh one for the same layer.)
        let clip = ui.clip_rect();
        let room = egui::Rect::from_min_max(clip.min - vec2(29., 44.), clip.max + vec2(44., 44.));
        ui.ctx()
            .layer_painter(ui.layer_id())
            .with_clip_rect(room)
            .add(ui::text_glow(ink, 22., 0.35, ui::tint(ui.ctx())));
        ui.painter().galley(at, galley, TEXT);
        ui.label(
            mono(&detail.to_uppercase(), 11.)
                .extra_letter_spacing(0.88)
                .color(FAINT),
        );
    });
}
pub fn section(ui: &mut Ui, title: &str, link: Option<Route>, v: &mut View<'_>) {
    ui.add_space(24.);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 12.;
        ui.label(hud(&title.to_uppercase(), 12.5).extra_letter_spacing(3.));
        if let Some(route) = link
            && ui
                .link(mono("Show all", 10.5).extra_letter_spacing(0.63).color(FAINT))
                .clicked()
        {
            v.go(route);
        }
        // a hairline fading out to the right
        let r = ui.available_rect_before_wrap();
        let y = r.center().y;
        ui::gradient(
            ui.painter(),
            egui::Rect::from_min_max(egui::pos2(r.left(), y), egui::pos2(r.right(), y + 1.)),
            HAIR2,
            Color32::TRANSPARENT,
            true,
        );
    });
    ui.add_space(4.);
}
pub fn note(ui: &mut Ui, text: &str) {
    ui.add_space(10.);
    ui.label(mono(text, 12.).color(MUTED));
}

// Keep a single derived result per widget, including while a search changes.
pub fn cached<T: Send + Sync + 'static>(
    ui: &Ui,
    slot: &'static str,
    revision: Id,
    build: impl FnOnce() -> T,
) -> Arc<T> {
    let slot = Id::unique(slot);
    if let Some((old, data)) = ui.data(|d| d.get_temp::<(Id, Arc<T>)>(slot))
        && old == revision
    {
        return data;
    }
    let data = Arc::new(build());
    ui.data_mut(|d| d.insert_temp(slot, (revision, data.clone())));
    data
}

pub struct Album {
    pub card: Card,
    pub artist: String,
    pub artist_key: String,
    pub year: u32,
    pub duration: f64,
    pub added: i64,
}
pub struct Artist {
    pub card: Card,
    pub albums: Vec<usize>,
    pub songs: usize,
}
pub struct Index {
    pub albums: Vec<Album>,
    pub artists: Vec<Artist>,
    pub playlists: Vec<Card>,
    pub recent_albums: Vec<usize>,
    pub most_artists: Vec<usize>,
}

fn has_image(c: &CoverSource) -> bool {
    !c.file.is_empty() || !c.embedded.is_empty() || !c.fallback.is_empty()
}
fn cover(s: &Session, ids: &[String]) -> Arc<CoverSource> {
    let mut it = ids.iter().map(|id| &s.models.by_id[id].cover);
    it.clone()
        .find(|c| has_image(c))
        .or_else(|| it.next())
        .cloned()
        .unwrap_or_default()
}
impl Index {
    pub fn get(ui: &Ui, v: &View<'_>) -> Arc<Self> {
        cached(
            ui,
            "library catalog cards",
            Id::unique(v.s.generation),
            || Self::build(v.s),
        )
    }
    fn build(s: &Session) -> Self {
        let m = &s.models;
        let tags: HashMap<_, _> = s
            .library
            .tracks
            .iter()
            .map(|t| (t.id(), t.tags.album_artist.as_str()))
            .collect();
        let mut album_idx = HashMap::new();
        let albums: Vec<_> = m
            .catalog
            .albums
            .iter()
            .enumerate()
            .map(|(i, g)| {
                album_idx.insert(&g.key, i);
                let mut names = HashMap::<&str, usize>::new();
                let mut year = 0;
                let mut added = 0;
                let mut duration = 0.;
                for id in &g.ids {
                    let song = &m.by_id[id];
                    let aa = tags.get(id).copied().unwrap_or("");
                    let name = if aa.is_empty()
                        || s.field("edits")[id]["artist"]
                            .as_str()
                            .is_some_and(|a| !a.is_empty())
                    {
                        &song.artist
                    } else {
                        aa
                    };
                    if !name.is_empty() {
                        *names.entry(name).or_default() += 1;
                    }
                    year = year.max(song.year);
                    added = added.max(song.added);
                    duration += song.duration;
                }
                let name = names
                    .iter()
                    .min_by_key(|&(name, n)| (Reverse(n), name))
                    .map_or("", |(name, _)| *name);
                let artist = if names.len() > 2
                    && g.ids
                        .first()
                        .and_then(|id| tags.get(id))
                        .is_none_or(|a| a.is_empty())
                {
                    "Various artists"
                } else if name.is_empty() {
                    "Unknown artist"
                } else {
                    name
                };
                let caption = if year == 0 {
                    artist.to_owned()
                } else {
                    format!("{artist} · {year}")
                };
                Album {
                    card: Card::page(
                        Route::Album(g.key.clone()),
                        &g.name,
                        caption,
                        Art::Cover(cover(s, &g.ids)),
                    ),
                    artist: artist.into(),
                    artist_key: norm(name),
                    year,
                    added,
                    duration,
                }
            })
            .collect();
        let mut artists: Vec<_> = m
            .catalog
            .artists
            .iter()
            .map(|g| {
                let mut records: Vec<_> = g
                    .ids
                    .iter()
                    .filter_map(|id| album_idx.get(&m.by_id[id].album_key).copied())
                    .collect();
                records.sort_unstable();
                records.dedup();
                let art = records.first().map_or_else(
                    || Art::Cover(cover(s, &g.ids)),
                    |&i| albums[i].card.art.clone(),
                );
                records.sort_by_key(|&i| Reverse(albums[i].year));
                Artist {
                    card: Card::page(
                        Route::Artist(g.key.clone()),
                        &g.name,
                        count(g.ids.len(), "song"),
                        art,
                    ),
                    albums: records,
                    songs: g.ids.len(),
                }
            })
            .collect();
        artists.sort_by_key(
            |a| matches!(&a.card.target,Target::Page(Route::Artist(k)) if k.is_empty()),
        );
        let playlists = m
            .playlists
            .iter()
            .map(|p| {
                let mut seen = HashSet::new();
                let picks: Vec<_> = p
                    .ids
                    .iter()
                    .map(|id| &m.by_id[id])
                    .filter(|t| has_image(&t.cover) && seen.insert(&t.cover.seed))
                    .take(4)
                    .map(|t| t.cover.clone())
                    .collect();
                let art = if picks.len() == 4 {
                    Art::Mosaic(picks)
                } else {
                    Art::Cover(picks.into_iter().next().unwrap_or_else(|| {
                        Arc::new(CoverSource {
                            seed: format!("pl:{}", p.id),
                            ..Default::default()
                        })
                    }))
                };
                Card::page(
                    Route::Playlist(p.id.clone()),
                    &p.name,
                    count(p.ids.len(), "song"),
                    art,
                )
            })
            .collect();
        // Newest first; albums added at the same moment keep the order their first song has in the library, as in
        // the web version.
        let mut first = HashMap::new();
        for (i, t) in s.library.tracks.iter().enumerate() {
            if let Some(song) = m.by_id.get(&t.id())
                && !s.state.hidden(&song.id)
            {
                first.entry(song.album_key.as_str()).or_insert(i);
            }
        }
        let mut recent_albums: Vec<_> = (0..albums.len()).collect();
        recent_albums.sort_by_key(|&i| {
            (
                Reverse(albums[i].added),
                first
                    .get(m.catalog.albums[i].key.as_str())
                    .copied()
                    .unwrap_or(usize::MAX),
            )
        });
        recent_albums.truncate(14);
        let mut most_artists:Vec<_>=(0..artists.len()).filter(|&i| matches!(&artists[i].card.target, Target::Page(Route::Artist(k)) if !k.is_empty())).collect();
        most_artists.sort_by_key(|&i| Reverse(artists[i].songs));
        most_artists.truncate(14);
        Self {
            albums,
            artists,
            playlists,
            recent_albums,
            most_artists,
        }
    }
    pub fn page(&self, route: &Route) -> Option<&Card> {
        match route {
            Route::Album(_) => self.albums.iter().map(|a| &a.card).find(|c| c.is(route)),
            Route::Artist(_) => self.artists.iter().map(|a| &a.card).find(|c| c.is(route)),
            Route::Playlist(_) => self.playlists.iter().find(|c| c.is(route)),
            _ => None,
        }
    }
}

pub fn page_context(route: &Route, name: &str) -> Value {
    let (kind, key) = match route {
        Route::Album(k) => ("album", k.as_str()),
        Route::Artist(k) => ("artist", k.as_str()),
        Route::Playlist(k) => ("pl", k.as_str()),
        Route::Liked => ("liked", ""),
        _ => ("songs", ""),
    };
    json!({"kind":kind,"key":key,"name":name})
}
pub fn is_now(v: &View<'_>, ctx: &Value) -> bool {
    let current = v.s.state.queue.context(None);
    v.s.current().is_some()
        && current["kind"] == ctx["kind"]
        && current["key"].as_str().unwrap_or("") == ctx["key"].as_str().unwrap_or("")
}
pub fn play(v: &mut View<'_>, ids: &[String], ctx: Value, shuffle: bool, filtered: bool) {
    if ids.is_empty() {
        v.views.toast.set("Nothing to play there yet");
        return;
    }
    if !shuffle && !filtered && is_now(v, &ctx) {
        v.s.toggle(v.player);
        return;
    }
    if shuffle && !v.s.state.queue.shuffled() {
        v.shuffle();
    }
    let start = if v.s.state.queue.shuffled() {
        rand::random_range(0..ids.len())
    } else {
        0
    };
    v.s.play_ids(v.player, ids, start as isize, ctx);
}
pub fn filtered(ui: &Ui, v: &mut View<'_>, ids: &Ids) -> Ids {
    if v.views.filter.trim().is_empty() {
        return ids.clone();
    }
    let key = Id::unique((
        v.s.generation,
        v.views.history.current.path(),
        &v.views.filter,
    ));
    cached(ui, "filtered library tracks", key, || {
        let q = norm(&v.views.filter);
        ids.iter()
            .filter(|id| v.s.models.by_id[*id].matches(&q))
            .cloned()
            .collect::<Ids>()
    })
    .as_ref()
    .clone()
}
pub fn clip_label(ui: &Ui, r: Rect, text: &str, face: &str, size: f32, color: Color32) {
    ui::spaced(ui.painter(), r, text, face, size, 0., color, Align2::LEFT_CENTER);
}
pub fn title_bar(ui: &mut Ui, height: f32, title: &str, meta: &str, controls: f32) -> Rect {
    let (_, r) = ui.allocate_space(vec2(ui.available_width(), height));
    let mut left = ui::child(
        ui,
        r.with_max_x((r.right() - controls - 8.).max(r.left())),
        "page title",
    );
    left.with_layout(egui::Layout::left_to_right(Align::Center), |ui| {
        heading(ui, title, meta)
    });
    r
}
