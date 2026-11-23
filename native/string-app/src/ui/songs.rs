use super::*;
use crate::{
    model::{Ids, Models},
    ui::covers::{CoverCache, Size},
};
use std::collections::BTreeSet;
#[derive(Default, Clone)]
pub struct Selection {
    pub indices: BTreeSet<usize>,
    anchor: Option<usize>,
    pub reveal: Option<usize>,
}
impl Selection {
    pub fn choose(&mut self, i: usize, ctrl: bool, shift: bool) {
        if shift {
            let a = self.anchor.unwrap_or(i);
            if !ctrl {
                self.indices.clear()
            }
            self.indices.extend(a.min(i)..=a.max(i));
        } else if ctrl {
            if !self.indices.remove(&i) {
                self.indices.insert(i);
            }
            self.anchor = Some(i);
        } else {
            self.indices.clear();
            self.indices.insert(i);
            self.anchor = Some(i);
        }
    }
    pub fn ids(&self, ids: &[String]) -> Ids {
        self.indices
            .iter()
            .filter_map(|&i| ids.get(i))
            .cloned()
            .collect()
    }
}
#[derive(Debug, Clone, Copy)]
pub enum Action {
    Play(usize),
    PlayNext,
    Append,
    Like,
    Sort(&'static str),
    Remove,
    LikeOne(usize),
    Toggle,
    Album(usize),
    Artist(usize),
    /// Right-click on this row: the caller opens the song menu here.
    Menu(usize, egui::Pos2),
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Columns {
    Added,
    Album,
    Plain,
    Liked,
    Popular,
}
pub struct Table<'a> {
    pub ids: &'a Ids,
    pub models: &'a Models,
    pub columns: Columns,
    pub liked: &'a serde_json::Value,
    pub plays: &'a serde_json::Value,
    pub now: Option<&'a str>,
    pub playing: bool,
    pub tint: Tint,
    pub mono: bool,
    pub playlist: bool,
    pub sort: Option<(&'a str, bool)>,
}
/// Drag payloads use Ids. A right-click returns `Action::Menu`; the caller opens `ui::menus::track_menu`.
pub fn show(
    ui: &mut Ui,
    table: Table<'_>,
    selection: &mut Selection,
    covers: &mut CoverCache,
) -> Option<Action> {
    let Table {
        ids,
        models,
        columns,
        liked,
        plays,
        now,
        playing,
        tint,
        mono,
        playlist,
        sort,
    } = table;
    let mut action = None;
    let w = ui.available_width();
    let compact = w < 670.;
    let album = !compact && columns != Columns::Album;
    let extra = !compact && matches!(columns, Columns::Liked | Columns::Added | Columns::Popular);
    let like_x = w - 154.;
    let title_end = if album {
        66. + (w - 260.).max(180.) * 0.50
    } else {
        like_x
    };
    let extra_x = if extra {
        like_x - (w * 0.16).max(88.)
    } else {
        like_x
    };
    let cols = [12., 66., title_end, extra_x, w - 104., w - 58.];
    let (_, header) = ui.allocate_space(vec2(w, 36.));
    let base = header.left();
    let extra_title = match columns {
        Columns::Popular => "PLAYS",
        Columns::Liked => "LIKED",
        _ => "ADDED",
    };
    for (a, b, title, k) in [
        (0, 1, "#", ""),
        (1, 2, "TITLE", "title"),
        (2, 3, "ALBUM", "album"),
        (3, 4, extra_title, "added"),
        (4, 5, "TIME", "duration"),
    ] {
        if (a == 2 && !album) || (a == 3 && !extra) {
            continue;
        }
        let mut r = area(base + cols[a], header.top(), cols[b] - cols[a], 36.);
        if a == 0 {
            r = r.with_min_x(base + 28.);
        }
        if a == 4 {
            r = r.with_min_x(r.right() - 31.);
        }
        let caption = if sort.is_some_and(|(key, _)| key == k) {
            format!("{title} {}", if sort.unwrap().1 { "▴" } else { "▾" })
        } else {
            title.into()
        };
        label(
            ui.painter(),
            r,
            &caption,
            "Mono",
            10.,
            if sort.is_some_and(|(key, _)| key == k) {
                TEXT
            } else {
                MUTED
            },
        );
        if sort.is_some() && !k.is_empty() && hit(ui, r, ("sort", k), title).clicked() {
            action = Some(Action::Sort(k));
        }
    }
    ui.painter()
        .hline(header.x_range(), header.bottom(), (1., HAIR2));
    ui.spacing_mut().item_spacing.y = 0.;
    if ids.is_empty() {
        return action;
    }
    let reveal = selection.reveal.take();
    let mut paint_rows = |ui: &mut Ui, range: std::ops::Range<usize>| {
        for i in range {
            let Some(song) = models.by_id.get(&ids[i]) else {
                continue;
            };
            let (r, response) = ui.allocate_exact_size(vec2(w, ROW), Sense::click_and_drag());
            let p = ui.painter();
            let active = now == Some(song.id.as_str());
            let selected = selection.indices.contains(&i);
            response.widget_info(|| {
                WidgetInfo::selected(
                    Role::ListBoxOption,
                    true,
                    selected,
                    format!("{} · {}", song.title, song.artist_name()),
                )
            });
            if selected || response.hovered() || active {
                p.rect_filled(
                    r,
                    0,
                    tint.ink(if selected {
                        0.105
                    } else if active {
                        0.06
                    } else {
                        0.035
                    }),
                );
            }
            if playlist {
                p.hline(r.x_range(), r.bottom() - 0.5, (1., HAIR));
            }
            let line =
                |a: usize, b: usize| area(r.left() + cols[a], r.top(), cols[b] - cols[a] - 6., ROW);
            let num = area(r.left() + 10., r.top(), 44., ROW);
            let over = ui.rect_contains_pointer(r);
            if over {
                let res = hit(ui, num, ("play row", i), "Play or pause song");
                glyph::draw(
                    ui.painter(),
                    Rect::from_center_size(num.center(), Vec2::splat(17.)),
                    if active && playing { "pausef" } else { "playf" },
                    TEXT,
                );
                if res.clicked() {
                    action = Some(if active {
                        Action::Toggle
                    } else {
                        Action::Play(i)
                    });
                }
            } else if active {
                glyph::draw(
                    ui.painter(),
                    Rect::from_center_size(num.center(), Vec2::splat(15.)),
                    if playing { "pulse" } else { "pause" },
                    tint.ink(1.),
                );
            } else {
                let n = if columns == Columns::Album && song.number > 0 {
                    song.number as usize
                } else {
                    i + 1
                };
                ui.painter().text(
                    num.center(),
                    Align2::CENTER_CENTER,
                    n.to_string(),
                    font("Mono", 11.),
                    FAINT,
                );
            }
            let p = ui.painter().clone();
            let cover = area(r.left() + cols[1], r.center().y - 19., 38., 38.);
            sleeve(
                &p,
                cover,
                covers.get(&song.cover, Size::Small, mono.then_some(tint)),
            );
            let x = cover.right() + 12.;
            let end = r.left() + cols[2] - 8.;
            label(
                &p,
                area(x, r.top() + 6., end - x, 21.),
                &song.title,
                "Strong",
                14.,
                TEXT,
            );
            label(
                &p,
                area(x, r.top() + 27., end - x, 18.),
                song.artist_name(),
                "UI",
                12.,
                MUTED,
            );
            if over {
                let sub = area(
                    x,
                    r.top() + 26.,
                    (end - x).min(song.artist_name().chars().count() as f32 * 7. + 6.),
                    21.,
                );
                if !song.artist_key.is_empty()
                    && hit(ui, sub, ("row artist", i), song.artist_name()).clicked()
                {
                    action = Some(Action::Artist(i));
                }
            }
            if album {
                label(&p, line(2, 3), &song.album, "UI", 13., MUTED);
                if over
                    && !song.album_key.is_empty()
                    && hit(ui, line(2, 3), ("row album", i), &song.album).clicked()
                {
                    action = Some(Action::Album(i));
                }
            }
            if extra {
                let value = if columns == Columns::Popular {
                    plays[&song.id][0]
                        .as_u64()
                        .map(|n| format!("{n} play{}", if n == 1 { "" } else { "s" }))
                        .unwrap_or_default()
                } else {
                    let ts = if columns == Columns::Liked {
                        liked[&song.id].as_i64().unwrap_or(0)
                    } else {
                        song.added
                    };
                    chrono::DateTime::from_timestamp_millis(ts)
                        .filter(|_| ts > 0)
                        .map_or_else(String::new, |t| {
                            let date = t.with_timezone(&chrono::Local).date_naive();
                            match (chrono::Local::now().date_naive() - date).num_days() {
                                0 => "Today".into(),
                                1 => "Yesterday".into(),
                                _ => date.format("%Y-%m-%d").to_string(),
                            }
                        })
                };
                label(
                    &p,
                    area(r.left() + extra_x, r.top(), like_x - extra_x, ROW),
                    &value,
                    "Mono",
                    11.,
                    FAINT,
                );
            }
            let is_liked =
                !liked[&song.id].is_null() && liked[&song.id] != false && liked[&song.id] != 0;
            if over || is_liked {
                let hr = area(r.left() + like_x, r.center().y - 15., 30., 30.);
                let icon = Rect::from_center_size(hr.center(), Vec2::splat(13.));
                if is_liked {
                    crate::screens::cards::heart(&p, icon, tint);
                } else {
                    glyph::draw(&p, icon, "heart", MUTED);
                }
                if hit(ui, hr, ("row like", i), "Toggle liked").clicked() {
                    action = Some(Action::LikeOne(i));
                }
            }
            p.text(
                pos2(r.right() - 58., r.center().y),
                Align2::RIGHT_CENTER,
                time(song.duration),
                font("Mono", 12.),
                MUTED,
            );
            if response.clicked() {
                let m = ui.input(|s| s.modifiers);
                selection.choose(i, m.ctrl, m.shift);
            }
            if (response.secondary_clicked() || response.drag_started())
                && !selection.indices.contains(&i)
            {
                selection.choose(i, false, false);
            }
            if response.drag_started() {
                response.dnd_set_drag_payload(selection.ids(ids));
            }
            if response.double_clicked() {
                action = Some(Action::Play(i));
            }
            if let Some(at) = crate::ui::menus::menu_pos(&response) {
                action = Some(Action::Menu(i, at));
            }
        }
    };
    if columns == Columns::Added {
        let mut scroll = egui::ScrollArea::vertical().auto_shrink([false, false]);
        if let Some(i) = reveal {
            scroll = scroll.vertical_scroll_offset(i.saturating_sub(2) as f32 * ROW);
        }
        scroll.show_rows(ui, ROW, ids.len(), paint_rows);
    } else {
        let rect = ui.allocate_space(vec2(w, ids.len() as f32 * ROW)).1;
        let visible = rect.intersect(ui.clip_rect());
        if visible.is_positive() {
            let start = ((visible.top() - rect.top()) / ROW).floor() as usize;
            let end = (((visible.bottom() - rect.top()) / ROW).ceil() as usize).min(ids.len());
            let mut rows = child(
                ui,
                area(
                    rect.left(),
                    rect.top() + start as f32 * ROW,
                    w,
                    (end - start) as f32 * ROW,
                ),
                "tracks",
            );
            rows.skip_ahead_auto_ids(start);
            paint_rows(&mut rows, start..end);
        }
    }
    action
}
