pub mod album;
pub mod albums;
pub mod artist;
pub mod artists;
pub(crate) mod cards;
pub(crate) mod detail;
pub mod dupes;
pub mod home;
pub(crate) mod library;
pub(crate) mod library_input;
pub(crate) mod library_smoke;
pub mod liked;
pub mod lyrics;
#[path = "../ui/library_motion.rs"]
mod motion;
pub mod now_playing;
pub mod playlist;
pub mod search;
pub mod settings;
pub mod songs;
pub mod stats;
use crate::{
    model::norm,
    route::Route,
    theme::*,
    ui::{
        self,
        songs::{Action, Table},
    },
    view::{ListCache, View},
};
use egui::{RichText, Ui};
use serde_json::json;
pub fn show(ui: &mut Ui, v: &mut View<'_>) {
    let draw = match v.views.history.current {
        Route::Home => home::show,
        Route::Songs => songs::show,
        Route::Albums => albums::show,
        Route::Artists => artists::show,
        Route::Album(_) => album::show,
        Route::Artist(_) => artist::show,
        Route::Playlist(_) => playlist::show,
        Route::Liked => liked::show,
        Route::Search => search::show,
        Route::Stats => stats::show,
        Route::Settings => settings::show,
        Route::Dupes => dupes::show,
    };
    // A fade keeps the destination controls in place for the very next click.
    // Cards have their own rise, including when the album order changes.
    let motion = motion::Motion::for_page(ui.ctx(), &v.views.history.current);
    ui.multiply_opacity(motion.page_progress(ui.ctx()));
    v.views.selection.shown = false;
    v.views.artist_popular_selection.shown = false;
    draw(ui, v);
    for selection in [
        &mut v.views.selection,
        &mut v.views.artist_popular_selection,
    ] {
        if !selection.shown {
            selection.left_list();
        }
    }
}
pub fn heading(ui: &mut Ui, title: &str, detail: &str) {
    library::heading(ui, title, detail);
    ui.add_space(14.);
}
pub fn library_table(ui: &mut Ui, v: &mut View<'_>) {
    let route = v.views.history.current.clone();
    let context = v.context();
    let title = match &route {
        Route::Songs => "Songs",
        Route::Search if !v.views.query.is_empty() => &v.views.query,
        _ => context["name"].as_str().unwrap_or("Songs"),
    };
    let dirty = v.views.cache.as_ref().is_none_or(|c| {
        c.route != route
            || c.generation != v.s.generation
            || c.query != v.views.query
            || c.filter != v.views.filter
    });
    if dirty {
        let mut ids = v.s.models.context_ids(&context);
        if !v.views.filter.is_empty() {
            let term = norm(&v.views.filter);
            ids = ids
                .iter()
                .filter(|id| {
                    let s = &v.s.models.by_id[*id];
                    norm(&format!("{} {} {}", s.title, s.artist, s.album)).contains(&term)
                })
                .cloned()
                .collect();
        }
        v.views.cache = Some(ListCache {
            ids,
            route: route.clone(),
            generation: v.s.generation,
            query: v.views.query.clone(),
            filter: v.views.filter.clone(),
        });
    }
    let ids = v.views.cache.as_ref().unwrap().ids.clone();
    if let Some(id) = v.views.reveal_id.take()
        && let Some(i) = ids.iter().position(|j| j == &id)
    {
        v.views.selection.display(&ids, None);
        v.views.selection.choose(i, false, false);
        v.views.selection.reveal = Some(i);
    }
    let (_, bar) = ui.allocate_space(egui::vec2(ui.available_width(), 65.));
    let p = ui.painter().clone();
    let dy = bar.center().y;
    let controls = if bar.width() < 650. { 220. } else { 343. };
    let mut title_ui = ui::child(
        ui,
        ui::area(bar.left(), dy - 17., bar.width() - controls, 35.),
        "list heading",
    );
    title_ui.horizontal_centered(|ui| {
        ui.label(hud(&title.to_uppercase(), 22.).strong());
        ui.label(mono(&format!("{} SONGS", ids.len()), 11.).color(FAINT));
    });
    let field = ui::area(bar.right() - controls, dy - 16., controls - 116., 32.);
    p.rect(field, 16, BG, (1., HAIR2), egui::StrokeKind::Inside);
    ui::glyph::draw(
        &p,
        ui::area(field.left() + 10., dy - 7., 14., 14.),
        "search",
        FAINT,
    );
    if ui
        .put(
            field
                .shrink2(egui::vec2(8., 5.))
                .with_min_x(field.left() + 32.),
            egui::TextEdit::singleline(&mut v.views.filter)
                .hint_text("Filter songs")
                .frame(egui::Frame::NONE),
        )
        .changed()
    {
        ui.ctx().request_repaint();
    }
    if ui::icon_btn(
        ui,
        ui::area(bar.right() - 101., dy - 15., 30., 30.),
        "shuffle",
        "Shuffle songs",
        false,
        v.tint,
    )
    .clicked()
        && !ids.is_empty()
    {
        if !v.s.state.queue.shuffled() {
            v.shuffle();
        }
        v.s.play_ids(
            v.player,
            &ids,
            rand::random_range(0..ids.len()) as isize,
            context.clone(),
        );
    }
    if ui::play_big(
        ui,
        ui::area(bar.right() - 55., dy - 27., 54., 54.),
        false,
        v.tint,
    )
    .clicked()
        && !ids.is_empty()
    {
        v.s.play_ids(v.player, &ids, 0, context.clone());
    }
    ui.add_space(36.);
    if ids.is_empty() {
        ui.label(RichText::new("No songs here yet.").color(MUTED));
        return;
    }
    track_rows(ui, v, &ids, context, ui::songs::Columns::Added);
}

fn track_rows(
    ui: &mut Ui,
    v: &mut View<'_>,
    ids: &crate::model::Ids,
    context: serde_json::Value,
    columns: ui::songs::Columns,
) {
    let route = v.views.history.current.clone();
    let can_sort = matches!(route, Route::Songs);
    let sort = v.s.field("sort").clone();
    let table = Table {
        ids,
        columns,
        liked: v.s.field("liked"),
        plays: v.s.field("plays"),
        models: &v.s.models,
        now: v.s.state.queue.current(),
        playing: v.st.playing,
        tint: v.tint,
        mono: v.mono,
        playlist: matches!(route, Route::Playlist(_)),
        sort: can_sort.then(|| (sort["key"].as_str().unwrap_or("title"), sort["dir"] != -1)),
    };
    let table_top = ui.cursor().top();
    let mut action = ui::songs::show(ui, table, &mut v.views.selection, v.covers);
    let keys_here = if matches!(route, Route::Artist(_)) {
        let rect = ui
            .min_rect()
            .with_min_y(table_top)
            .intersect(ui.clip_rect());
        let active = egui::Id::unique(("artist focused list", route.path()));
        let popular = columns == ui::songs::Columns::Popular;
        if ui.rect_contains_pointer(rect) && ui.input(|i| i.pointer.any_pressed()) {
            ui.data_mut(|d| d.insert_temp(active, popular));
        }
        ui.data(|d| d.get_temp::<bool>(active)).unwrap_or(true) == popular
    } else {
        true
    };
    if keys_here && !ui.ctx().text_edit_focused() && !ui::menus::is_open() {
        ui.input(|i| {
            if i.modifiers.ctrl && i.key_pressed(egui::Key::A) {
                v.views.selection.indices.extend(0..ids.len());
            }
            if i.key_pressed(egui::Key::Enter)
                && let Some(&row) = v.views.selection.indices.first()
            {
                action = Some(Action::Play(row));
            }
        });
    }
    let selected = || v.views.selection.ids();
    match action {
        Some(Action::Menu(i, at)) => {
            // Play from the menu starts the list at the first selected row.
            let row = v.views.selection.indices.iter().min().copied().unwrap_or(i);
            ui::menus::track_menu(
                selected(),
                at,
                ui::menus::Origin::list(ids.clone(), row, context),
            );
        }
        Some(Action::Play(i)) => v.s.play_ids(v.player, ids, i as isize, context),
        Some(Action::LikeOne(i)) => v.s.toggle_like(&ids[i..=i]),
        Some(Action::Toggle) => v.s.toggle(v.player),
        Some(Action::Album(i)) => v.go(Route::Album(v.s.models.by_id[&ids[i]].album_key.clone())),
        Some(Action::Artist(i)) => {
            v.go(Route::Artist(v.s.models.by_id[&ids[i]].artist_key.clone()))
        }
        Some(Action::PlayNext) => v.s.insert(v.player, &selected(), false),
        Some(Action::Append) => v.s.insert(v.player, &selected(), true),
        Some(Action::Like) => v.s.toggle_like(&selected()),
        Some(Action::Sort(k)) => {
            let dir = if sort["key"] == k && sort["dir"] != -1 {
                -1
            } else {
                1
            };
            v.s.set_field("sort", json!({"key":k,"dir":dir}));
            v.s.models.sort_songs(&v.s.state);
            v.s.models.refresh_lists(&v.s.state);
            v.s.generation += 1;
            v.s.queue_changed(v.player);
            v.views.cache = None;
        }
        None => {}
    }
}
