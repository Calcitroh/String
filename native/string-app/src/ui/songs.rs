use super::*;
use crate::{
    model::{Ids, Models},
    ui::covers::{CoverCache, Size},
};
use std::{
    cell::RefCell,
    collections::{BTreeSet, HashMap},
    sync::{Arc, Weak},
};
#[derive(Clone)]
struct ExportedRows {
    ids: Weak<[String]>,
    indices: Vec<usize>,
}
#[derive(Default, Clone)]
pub struct Selection {
    pub indices: BTreeSet<usize>,
    // Indices always belong to this immutable list. A playlist row also has an added time.
    rows: Option<Ids>,
    added: Option<Arc<[i64]>>,
    // Both song tables and playlists export Ids. Track the exact occurrences while
    // the payload is held, including their original order across a sort.
    exported: RefCell<Option<ExportedRows>>,
    anchor: Option<usize>,
    pub(crate) shown: bool,
    pub reveal: Option<usize>,
}
impl Selection {
    pub fn display(&mut self, ids: &Ids, added: Option<&[i64]>) {
        if let Some(old) = &self.rows
            && !Arc::ptr_eq(old, ids)
        {
            super::menus::list_replaced(old, Some(ids));
        }
        self.bind(ids, added);
    }

    pub fn left_list(&mut self) {
        if let Some(old) = &self.rows {
            super::menus::list_replaced(old, None);
        }
        *self = Self::default();
    }

    pub fn bind(&mut self, ids: &Ids, added: Option<&[i64]>) {
        self.shown = true;
        if self.rows.as_ref().is_some_and(|old| Arc::ptr_eq(old, ids)) {
            return;
        }
        if let Some(old) = &self.rows {
            let exported = self.exported.get_mut();
            if exported.as_ref().is_some_and(|e| e.ids.strong_count() == 0) {
                *exported = None;
            }
            if !self.indices.is_empty() || self.anchor.is_some() || exported.is_some() {
                // For legacy entries without a timestamp, distinguish equal occurrences by their
                // order. If their count changed, there is no way to identify the surviving one.
                let old_added = self.added.as_deref();
                let key = |i: usize| (old[i].as_str(), old_added.map(|at| at[i]));
                let mut positions: HashMap<_, (Vec<usize>, Vec<usize>)> = self
                    .indices
                    .iter()
                    .copied()
                    .chain(self.anchor)
                    .chain(exported.iter().flat_map(|e| e.indices.iter().copied()))
                    .filter(|&i| i < old.len())
                    .map(|i| (key(i), Default::default()))
                    .collect();
                for i in 0..old.len() {
                    if let Some((before, _)) = positions.get_mut(&key(i)) {
                        before.push(i);
                    }
                }
                for (i, id) in ids.iter().enumerate() {
                    if let Some((_, after)) =
                        positions.get_mut(&(id.as_str(), added.map(|at| at[i])))
                    {
                        after.push(i);
                    }
                }
                let remap = |i: usize| {
                    if i >= old.len() {
                        return None;
                    }
                    let (before, after) = positions.get(&key(i))?;
                    if before.len() != after.len() {
                        return None;
                    }
                    Some(after[before.binary_search(&i).ok()?])
                };
                self.indices = self.indices.iter().copied().filter_map(remap).collect();
                self.anchor = self.anchor.and_then(remap);
                if let Some(e) = exported {
                    e.indices = e.indices.iter().copied().filter_map(remap).collect();
                }
            }
            self.reveal = None;
        } else {
            self.indices.retain(|&i| i < ids.len());
            self.anchor = self.anchor.filter(|&i| i < ids.len());
        }
        self.rows = Some(ids.clone());
        self.added = added.map(Arc::from);
    }

    /// Resolve a menu opened before playlist edits against the current raw playlist places.
    pub fn playlist_places(&mut self, entries: &[(usize, String, i64)]) -> Vec<usize> {
        let ids = entries.iter().map(|(_, id, _)| id.clone()).collect();
        let added: Vec<_> = entries.iter().map(|(_, _, at)| *at).collect();
        self.bind(&ids, Some(&added));
        self.indices.iter().map(|&i| entries[i].0).collect()
    }

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
    pub fn ids(&self) -> Ids {
        let ids = self.rows.as_deref().unwrap_or_default();
        let selected: Ids = self
            .indices
            .iter()
            .filter_map(|&i| ids.get(i))
            .cloned()
            .collect();
        *self.exported.borrow_mut() = Some(ExportedRows {
            ids: Arc::downgrade(&selected),
            indices: self
                .indices
                .iter()
                .copied()
                .filter(|&i| i < ids.len())
                .collect(),
        });
        selected
    }
    pub(super) fn dragged_ids(&self, payload: &Ids) -> Option<Vec<String>> {
        let exported = self.exported.borrow();
        let e = exported.as_ref()?;
        if !e.ids.ptr_eq(&Arc::downgrade(payload)) {
            return None;
        }
        let rows = self.rows.as_ref()?;
        Some(
            e.indices
                .iter()
                .filter(|i| self.indices.contains(i))
                .map(|&i| rows[i].clone())
                .collect(),
        )
    }
}
#[derive(Debug, Clone, Copy)]
pub enum Action {
    Play(usize),
    PlayNext,
    Append,
    Like,
    Sort(&'static str),
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
    selection.display(ids, None);
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
                    crate::screens::cards::lit_heart(&p, icon, tint);
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
                response.dnd_set_drag_payload(selection.ids());
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

#[cfg(test)]
mod selection_tests {
    use super::*;
    use crate::playback::Session;
    fn ids(names: &[&str]) -> Ids {
        names.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn drag_keeps_source_order_and_cannot_adopt_a_replacement_occurrence() {
        let mut source = Selection::default();
        source.bind(&ids(&["x", "b", "a", "x"]), Some(&[1, 2, 3, 4]));
        source.choose(0, false, false);
        source.choose(1, true, false);
        source.choose(2, true, false);
        let payload = source.ids();
        let mut other = source.clone();
        other.left_list();
        other.bind(&payload, None);
        assert!(other.dragged_ids(&payload).is_none());
        source.bind(&ids(&["a", "x", "b"]), Some(&[3, 4, 2]));
        source.indices.insert(1);
        assert_eq!(source.dragged_ids(&payload).unwrap(), ["b", "a"]);
    }

    #[test]
    fn audit_selection_plays_and_adds_only_the_surviving_song() {
        let mut selection = Selection::default();
        selection.bind(&ids(&["a", "b", "c", "d"]), None);
        selection.choose(1, false, false);
        selection.choose(3, true, false);
        let visible = ids(&["c", "a", "b"]);
        selection.bind(&visible, None);
        let mut library = string_core::library::Library::default();
        library.tracks = visible
            .iter()
            .map(|id| string_core::library::Track {
                path: id.clone(),
                ..Default::default()
            })
            .collect();
        let mut s = Session::new(Default::default(), Default::default(), library);
        let key = s.new_playlist("Mix", &[]);
        s.playlist_add(&key, &selection.ids());
        assert_eq!(s.playlist_entries(&key)[0].1, "b");
        assert_eq!(selection.indices.len(), 1);
        let start = *selection.indices.first().unwrap() as isize;
        assert!(s.state.play_ids(
            &visible,
            start,
            serde_json::json!({"kind":"songs"}),
            &s.models.catalog,
            1
        ));
        assert_eq!(s.state.queue.current(), Some("b"));
        selection.bind(&ids(&[]), None);
        selection.bind(&visible, None);
        assert!(selection.indices.is_empty());
    }
    #[test]
    fn audit_delete_removes_only_the_chosen_playlist_occurrence() {
        let mut selection = Selection::default();
        let before = ids(&["a", "a", "b"]);
        selection.bind(&before, Some(&[101, 102, 103]));
        selection.choose(1, false, false);
        let after = ids(&["b", "a", "a"]);
        selection.bind(&after, Some(&[103, 101, 102]));
        let mut s = Session::new(Default::default(), Default::default(), Default::default());
        s.set_field("playlists", serde_json::json!([{"id":"p","items":[{"id":"b","at":103},{"id":"a","at":101},{"id":"a","at":102}]}]));
        s.playlist_remove("p", &selection.indices.iter().copied().collect::<Vec<_>>());
        assert_eq!(
            s.playlist_entries("p"),
            vec![(0, "b".into(), 103), (1, "a".into(), 101)]
        );
    }
    #[test]
    fn open_menu_resolves_current_raw_places_and_drops_ambiguous_legacy_rows() {
        let mut picked = Selection::default();
        picked.bind(&ids(&["a", "a", "b", "b"]), Some(&[0, 0, 80, 90]));
        picked.choose(1, false, false);
        picked.choose(3, true, false);
        let current = [(0, "a".into(), 0), (2, "b".into(), 90), (3, "b".into(), 80)];
        assert_eq!(picked.playlist_places(&current), [2]);
        assert_eq!(picked.ids().as_ref(), ["b"]);
    }
}
