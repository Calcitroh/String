use super::{
    cards::{self, Art, Card, Target},
    library::{self, Index, count, duration},
};
use crate::{
    model::Ids,
    route::Route,
    theme::*,
    ui::{self, area},
    view::View,
};
use chrono::Timelike;
use egui::{Align2, Color32, Id, Rect, Shape, Stroke, Ui, Vec2, pos2, vec2};
use serde_json::json;
use std::cmp::Reverse;

pub fn show(ui: &mut Ui, v: &mut View<'_>) {
    if empty(ui, v) {
        return;
    }
    let catalog = Index::get(ui, v);
    let hour = chrono::Local::now().hour();
    let greeting = match hour {
        0..5 => "Late night",
        5..12 => "Good morning",
        12..18 => "Good afternoon",
        _ => "Good evening",
    };
    let plays = v.s.field("plays");
    let revision = Id::unique((v.s.generation, v.s.plays_revision));
    let lists = library::cached(ui, "home played tracks", revision, || {
        let mut ids: Vec<_> = plays
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(id, _)| v.s.models.by_id.contains_key(*id) && !v.s.state.hidden(id))
            .map(|(id, _)| id.clone())
            .collect();
        ids.sort_by_key(|id| Reverse(plays[id][1].as_i64().unwrap_or(0)));
        let lately: Ids = ids.iter().take(14).cloned().collect();
        ids.sort_by_key(|id| {
            Reverse((
                plays[id][0].as_u64().unwrap_or(0),
                plays[id][1].as_i64().unwrap_or(0),
            ))
        });
        ids.truncate(14);
        (lately, Ids::from(ids))
    });
    egui::ScrollArea::vertical().show(ui, |ui| {
        library::heading(
            ui,
            greeting,
            &format!(
                "{} · {}",
                count(v.s.models.songs.len(), "song"),
                duration(v.s.models.duration)
            ),
        );
        ui.add_space(14.);
        let shuffle = Card {
            target: Target::Shuffle,
            title: "Shuffle everything".into(),
            subtitle: String::new(),
            art: Art::Shuffle,
        };
        let liked = Card::page(Route::Liked, "Liked songs", String::new(), Art::Heart);
        let mut tiles = vec![&shuffle];
        if !v.s.models.liked.is_empty() {
            tiles.push(&liked);
        }
        for recent in v.s.field("recent").as_array().into_iter().flatten() {
            let Some(k) = recent["key"].as_str() else {
                continue;
            };
            let route = match recent["kind"].as_str() {
                Some("artist") => Route::Artist(k.into()),
                Some("album") => Route::Album(v.s.models.catalog.resolve_album_key(k).into()),
                Some("pl") => Route::Playlist(k.into()),
                _ => continue,
            };
            if tiles.len() == 8 {
                break;
            }
            if !tiles.iter().any(|c| c.is(&route))
                && let Some(card) = catalog.page(&route)
            {
                tiles.push(card);
            }
        }
        for pl in &catalog.playlists {
            if tiles.len() == 8 {
                break;
            }
            if !tiles.iter().any(|c| std::ptr::eq(*c, pl)) {
                tiles.push(pl);
            }
        }
        cards::grid(ui, v, tiles, true);
        track_row(ui, v, "Played lately", "lately", &lists.0, None);
        if catalog.albums.len() >= 3 {
            cards::row(
                ui,
                v,
                "Recently added albums",
                catalog
                    .recent_albums
                    .iter()
                    .map(|&i| &catalog.albums[i].card),
                Some(Route::Albums),
                None,
            );
        }
        let recent: Ids = v.s.models.recent_songs.clone().into();
        track_row(
            ui,
            v,
            "Recently added songs",
            "added",
            &recent,
            Some(Route::Songs),
        );
        if lists.1.len() >= 3 {
            track_row(ui, v, "On repeat", "top", &lists.1, None);
        }
        if catalog.most_artists.len() >= 2 {
            cards::row(
                ui,
                v,
                "Artists",
                catalog
                    .most_artists
                    .iter()
                    .map(|&i| &catalog.artists[i].card),
                Some(Route::Artists),
                None,
            );
        }
    });
}
fn track_row(ui: &mut Ui, v: &mut View<'_>, name: &str, key: &str, ids: &Ids, link: Option<Route>) {
    let cards: Vec<_> = ids
        .iter()
        .map(|id| Card::track(&v.s.models.by_id[id]))
        .collect();
    let ctx =
        json!({"kind":"recent","key":key,"name":if key=="added" {"Recently added"}else{name}});
    cards::row(ui, v, name, &cards, link, Some((ids, &ctx)));
}
fn welcome_button(ui: &mut Ui, r: Rect, title: &str, icon: &str, primary: bool) -> bool {
    let response = ui::hit(ui, r, title, title);
    let p = ui.painter();
    let ink = if primary { BG } else { TEXT };
    if primary {
        let cut = 8.;
        p.add(Shape::convex_polygon(
            vec![
                r.left_top() + vec2(cut, 0.),
                r.right_top(),
                r.right_bottom() - vec2(0., cut),
                r.right_bottom() - vec2(cut, 0.),
                r.left_bottom(),
                r.left_top() + vec2(0., cut),
            ],
            TEXT,
            Stroke::NONE,
        ));
    } else {
        p.rect(
            r,
            0,
            if response.hovered() { RAISED } else { PANEL },
            (1., HAIR3),
            egui::StrokeKind::Inside,
        );
    }
    ui::glyph::draw(
        p,
        area(r.left() + 21., r.center().y - 8., 16., 16.),
        icon,
        ink,
    );
    let mut text = ui::child(ui, r.with_min_x(r.left() + 43.), title);
    text.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.label(hud(title, 12.).color(ink).strong())
    });
    response.clicked()
}

pub fn empty(ui: &mut Ui, v: &mut View<'_>) -> bool {
    if !v.s.models.songs.is_empty() {
        return false;
    }
    let first = ["roots", "files"]
        .iter()
        .all(|k| v.s.field(k).as_array().is_none_or(Vec::is_empty));
    let scanning = !first && v.scan.running();
    let r = ui.available_rect_before_wrap();
    let compact = r.height() < 530.;
    let side = if compact { 120. } else { 210. };
    let cx = r.center().x;
    let top = (r.center().y - (side + 215.) / 2. - 43.).max(r.top() + 6.);
    let pos = pos2(cx, top + side / 2.);
    let p = ui.painter().clone();
    for i in (1..=28).rev() {
        p.circle_filled(
            pos,
            side * (0.5 + i as f32 / 70.),
            v.tint.light(0.008 * (1. - i as f32 / 30.)),
        );
    }
    p.circle_filled(pos, side / 2., BG);
    for i in 0..80 {
        let a = i as f32 / 80. * std::f32::consts::TAU;
        let b = (i + 1) as f32 / 80. * std::f32::consts::TAU;
        let c = v.tint.ink((a + 1.1).cos().abs().powi(10) * 0.13);
        p.add(Shape::convex_polygon(
            vec![
                pos,
                pos + Vec2::angled(a) * side / 2.,
                pos + Vec2::angled(b) * side / 2.,
            ],
            c,
            Stroke::NONE,
        ));
    }
    for i in 1..=(side / 10.) as usize {
        p.circle_stroke(pos, i as f32 * 5., (1., v.tint.ink(0.12)));
    }
    p.circle_stroke(pos, side / 2., (1., v.tint.ink(0.32)));
    p.circle_filled(pos, side * 0.19, v.tint.ink(0.14));
    p.circle_filled(pos, side * 0.077, Color32::BLACK);
    p.circle_filled(pos, side * 0.026, TEXT);
    let y = top + side + if compact { 10. } else { 27. };
    let title = if first {
        "YOUR RECORDS GO HERE"
    } else if scanning {
        "READING YOUR SONGS"
    } else {
        "NO SONGS HERE YET"
    };
    let mut text = ui::child(ui, area(cx - 240., y, 480., 100.), "welcome text");
    text.vertical_centered(|ui|{
        ui.label(hud(title,20.).extra_letter_spacing(5.4).strong());
        ui.add_space(5.);
        let desc=if first{"Pick the folders your songs are in. String plays them where they are: nothing is moved, copied or changed. Add more folders any time."}else if scanning{"Looking through your folders…"}else{"String found no songs in the folders you picked. It plays MP3, FLAC, M4A, OGG, Opus and WAV."};
        ui.set_max_width(460.);ui.label(egui::RichText::new(desc).font(font("UI",13.5)).color(MUTED));
    });
    if !scanning {
        let by = y + if compact { 73. } else { 101. };
        if welcome_button(
            ui,
            area(cx - 173., by, 177., 46.),
            "ADD A FOLDER",
            "folder",
            true,
        ) {
            v.pick_folders();
        }
        if welcome_button(
            ui,
            area(cx + 14., by, 159., 46.),
            if first { "ADD SONGS" } else { "SETTINGS" },
            if first { "music" } else { "gear" },
            false,
        ) {
            if first {
                v.pick_songs();
            } else {
                v.go(Route::Settings);
            }
        }
        if first {
            p.text(
                pos2(cx, by + 75.),
                Align2::CENTER_CENTER,
                "Or drop folders and songs anywhere on this window",
                font("UI", 13.5),
                MUTED,
            );
        }
    }
    true
}
