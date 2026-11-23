//! A playlist's page: its cover, its songs with the day each was added, the filter, and reordering by drag.
use crate::{
    model::{Ids, norm},
    route::Route,
    theme::*,
    ui::{
        self, area,
        covers::Size,
        glyph,
        menus::{self, Origin, caps},
        side_playlists::{cover_picks, mosaic},
    },
    view::View,
};
use egui::{
    Align2, Color32, Key, Painter, Pos2, Rangef, Rect, Sense, StrokeKind, Ui, Vec2, epaint::Shadow,
    pos2, vec2,
};
use serde_json::json;
use std::sync::Mutex;

struct Entry {
    /// The entry's place in the playlist's items.
    place: usize,
    id: String,
    added: i64,
}
struct Page {
    key: String,
    generation: u64,
    filter: String,
    rows: Vec<Entry>,
    ids: Ids,
    /// Entries in the playlist, and how many of them the library still has.
    total: usize,
    have: usize,
    duration: f64,
    /// The rows being dragged to a new place.
    drag: Vec<usize>,
    scroll: Option<f32>,
}
static PAGE: Mutex<Option<Page>> = Mutex::new(None);
const HERO: f32 = 262.;
const ACTIONS: f32 = 80.;
const HEAD: f32 = 40.;

fn build(v: &View<'_>, key: &str) -> Page {
    let entries = v.s.playlist_entries(key);
    let total = entries.len();
    let filter = v.views.filter.clone();
    let terms = norm(&filter);
    let terms: Vec<_> = terms.split_whitespace().collect();
    let (mut have, mut duration) = (0, 0.);
    let mut rows = vec![];
    for (place, id, added) in entries {
        let Some(song) = v.s.models.by_id.get(&id).filter(|_| !v.s.state.hidden(&id)) else {
            continue;
        };
        have += 1;
        duration += song.duration;
        if !terms.is_empty() {
            let text = norm(&format!("{} {} {}", song.title, song.artist, song.album));
            if !terms.iter().all(|t| text.contains(t)) {
                continue;
            }
        }
        rows.push(Entry { place, id, added });
    }
    Page {
        key: key.into(),
        generation: v.s.generation,
        filter,
        ids: rows.iter().map(|r| r.id.clone()).collect(),
        rows,
        total,
        have,
        duration,
        drag: vec![],
        scroll: None,
    }
}

struct Cols {
    num: Rangef,
    title: Rangef,
    album: Option<Rangef>,
    extra: Option<Rangef>,
    like: Rangef,
    dur: Rangef,
    more: Rangef,
}
fn cols(left: f32, w: f32) -> Cols {
    let mut x = left + 10.;
    let mut take = |width: f32| {
        let r = Rangef::new(x, x + width);
        x += width + 12.;
        r
    };
    if w < 640. {
        let title = (w - 20. - 36. - 40. - 56. - 32. - 48.).max(140.);
        return Cols {
            num: take(36.),
            title: take(title),
            album: None,
            extra: None,
            like: take(40.),
            dur: take(56.),
            more: take(32.),
        };
    }
    let fr = (w - 20. - 184. - 72.).max(0.) / 8.;
    Cols {
        num: take(44.),
        title: take((fr * 4.).max(180.)),
        album: Some(take((fr * 2.6).max(120.))),
        extra: Some(take((fr * 1.4).max(90.))),
        like: take(40.),
        dur: take(64.),
        more: take(36.),
    }
}
fn header(p: &Painter, r: Rect, c: &Cols) {
    let y = r.top() + 18.;
    let word = |x: f32, anchor: Align2, text: &str| {
        caps(p, pos2(x, y), anchor, text, "HUD", 10., 0.16, FAINT);
    };
    word(c.num.center(), Align2::CENTER_CENTER, "#");
    word(c.title.min, Align2::LEFT_CENTER, "Title");
    if let Some(a) = c.album {
        word(a.min, Align2::LEFT_CENTER, "Album");
    }
    if let Some(e) = c.extra {
        word(e.min, Align2::LEFT_CENTER, "Added");
    }
    word(c.dur.max, Align2::RIGHT_CENTER, "Time");
    p.hline(r.x_range(), r.top() + 36., (1., HAIR2));
}
/// A filled heart: the outline drawn again and again, smaller each time.
fn heart(p: &Painter, r: Rect, color: Color32) {
    let mut size = 16.;
    while size > 2. {
        glyph::draw(
            p,
            Rect::from_center_size(
                r.center() + vec2(0., (16. - size) * 0.04),
                Vec2::splat(size),
            ),
            "heart",
            color,
        );
        size -= 1.6;
    }
}
/// Text that acts as a link: brighter and underlined under the pointer. True when clicked.
fn link(
    ui: &mut Ui,
    id: impl egui::AsIdSalt,
    r: Rect,
    text: &str,
    face: &str,
    size: f32,
    c: Color32,
) -> bool {
    let p = ui.painter().clone();
    let width = p
        .layout_no_wrap(text.to_owned(), font(face, size), c)
        .size()
        .x
        .min(r.width());
    let hit = Rect::from_min_size(
        pos2(r.left(), r.center().y - size * 0.7),
        vec2(width, size * 1.4),
    );
    let res = ui.interact(hit, ui.scope_id().with(id), Sense::click());
    let hot = res.hovered();
    ui::label(&p, r, text, face, size, if hot { TEXT } else { c });
    if hot {
        p.hline(
            hit.x_range(),
            hit.bottom() - 1.,
            (1., Color32::from_white_alpha(110)),
        );
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    res.clicked()
}
enum Act {
    Play(usize),
    Choose(usize, bool, bool),
    Menu(usize, Pos2),
    Like(String),
    Go(Route),
    Drag,
}

pub fn show(ui: &mut Ui, v: &mut View<'_>) {
    let Route::Playlist(key) = v.views.history.current.clone() else {
        return;
    };
    let Some(name) =
        v.s.models
            .playlists
            .iter()
            .find(|p| p.id == key)
            .map(|p| p.name.clone())
    else {
        ui.add_space(20.);
        menus::empty_note(ui, "That playlist was deleted");
        return;
    };
    let ctx = ui.ctx().clone();
    let hi = v.tint;
    let old = PAGE.lock().unwrap().take().filter(|p| p.key == key);
    let mut page = match old {
        Some(p) if p.generation == v.s.generation && p.filter == v.views.filter => p,
        old => {
            let mut page = build(v, &key);
            if old.is_some_and(|o| o.filter == page.filter) {
                v.views.selection.indices.retain(|&i| i < page.rows.len());
            } else {
                v.views.selection = Default::default();
            }
            page.scroll = None;
            page
        }
    };
    let n = page.rows.len();
    let ids = page.ids.clone();
    let list_ctx = json!({"kind":"pl","key":key,"name":name});
    let filtered = !page.filter.trim().is_empty();
    let visible = filtered.then(|| ids.clone());
    let show_list = page.have > 0 || filtered;
    if let Some(id) = v.views.reveal_id.take()
        && let Some(i) = ids.iter().position(|x| x == &id)
    {
        v.views.selection.choose(i, false, false);
        page.scroll = Some((HERO + ACTIONS + i as f32 * ROW - 2. * ROW).max(0.));
    }
    let outer = ui.max_rect();
    let w = outer.width();
    let c = cols(outer.left(), w);
    let now = v.s.state.queue.current().map(str::to_owned);
    let here = menus::same_ctx(v.s.state.queue.context(None), &list_ctx);
    let time = ctx.input(|i| i.time);
    let dragging = !page.drag.is_empty() && egui::DragAndDrop::has_payload_of_type::<Ids>(&ctx);
    let mut act = None;
    let mut drop_at = None;
    let mut scroll = egui::ScrollArea::vertical()
        .id_salt(("playlist", &key))
        .auto_shrink([false, false]);
    if let Some(y) = page.scroll.take() {
        scroll = scroll.vertical_scroll_offset(y);
    }
    ui.spacing_mut().item_spacing = Vec2::ZERO;
    let out = scroll.show(ui, |ui| {
        /* ---- the hero ---- */
        let (hero, _) = ui.allocate_exact_size(vec2(w, HERO), Sense::hover());
        let glow = Rect::from_min_max(
            pos2(outer.left() - 29., hero.top() - 14.),
            pos2(outer.right() + 29., hero.bottom()),
        );
        // The cover's blurred light behind the hero, out to the edges of the main area.
        let wide = ctx
            .layer_painter(ui.layer_id())
            .with_clip_rect(Rect::from_min_max(
                pos2(glow.left(), outer.top() - 14.),
                pos2(glow.right(), outer.bottom()),
            ));
        let lead = cover_picks(&v.s.models, &ids).into_iter().next();
        if let Some(tex) = lead
            .and_then(|c| v.covers.get(&c, Size::Large, v.mono.then_some(hi)))
            .and_then(|a| a.ambient)
        {
            let mut light = egui::Mesh::with_texture(tex.id());
            for (at, alpha) in [(0., 0.), (0.16, 0.9), (0.6, 0.5), (1., 0.)] {
                let y = glow.top() + glow.height() * at;
                let color = Color32::from_white_alpha((alpha * 255.) as u8);
                let n = light.vertices.len() as u32;
                for (x, u) in [(glow.left(), 0.), (glow.right(), 1.)] {
                    light.vertices.push(egui::epaint::Vertex {
                        pos: pos2(x, y),
                        uv: pos2(u, 0.1 + 0.6 * at),
                        color,
                    });
                }
                if n > 0 {
                    light.add_triangle(n - 2, n - 1, n + 1);
                    light.add_triangle(n - 2, n + 1, n);
                }
            }
            wide.add(light);
        }
        let side = if w < 640. { 150. } else { 216. };
        let art = area(hero.left(), hero.bottom() - 26. - side, side, side);
        let p = ui.painter().clone();
        p.add(
            Shadow {
                offset: [0, 30],
                blur: 70,
                spread: 0,
                color: Color32::from_black_alpha(178),
            }
            .as_shape(art, 0),
        );
        mosaic(&p, art, v, &ids, &format!("pl:{key}"), Size::Medium);
        let x = art.right() + 28.;
        let room = (hero.right() - x).max(40.);
        let meta_y = art.bottom() - 4. - 9.4;
        let mut size = (ctx.content_rect().width() * 0.05).clamp(28., 64.);
        let fits = |size: f32| p.layout_no_wrap(name.clone(), font("Strong", size), Color32::WHITE);
        let mut title = fits(size);
        if title.size().x > room {
            size = (size * room / title.size().x).max(28.);
            title = fits(size);
        }
        let title_h = size * 1.02;
        let title_bottom = art.bottom() - 4. - 18.75 - 12.;
        let title_rect = Rect::from_min_size(
            pos2(x, title_bottom - title_h),
            vec2(title.size().x.min(room), title_h),
        );
        caps(
            &p,
            pos2(x, title_rect.top() - 10. - 5.),
            Align2::LEFT_CENTER,
            "Playlist",
            "HUD",
            10.5,
            0.2,
            hi.ink(1.),
        );
        p.with_clip_rect(title_rect.expand2(vec2(0., 8.)).intersect(p.clip_rect()))
            .galley(
                pos2(x, title_rect.center().y - title.size().y / 2.),
                title,
                Color32::WHITE,
            );
        let rename = ui
            .interact(title_rect, ui.scope_id().with("rename"), Sense::click())
            .on_hover_cursor(egui::CursorIcon::Text)
            .on_hover_text("Rename");
        if rename.clicked() {
            menus::rename_playlist(v, &key);
        }
        let meta = if page.total == 0 {
            "Empty: add songs from their ⋯ menu, or drag them onto the playlist".to_owned()
        } else {
            let mut m = format!(
                "{}, {}",
                menus::plural(page.total, "song"),
                menus::long(page.duration)
            );
            if page.have < page.total {
                m += &format!(" · {} not found", page.total - page.have);
            }
            m
        };
        ui::label(
            &p,
            area(x, meta_y - 10., room, 20.),
            &meta,
            "Mono",
            12.5,
            SECONDARY,
        );

        /* ---- play, shuffle, more, filter ---- */
        let (bar, _) = ui.allocate_exact_size(vec2(w, ACTIONS), Sense::hover());
        let cy = bar.top() + 34.;
        if ui::play_big(
            ui,
            area(bar.left(), cy - 28., 56., 56.),
            here && v.st.playing,
            hi,
        )
        .clicked()
        {
            menus::play_ctx(v, &list_ctx, visible.as_ref());
        }
        if ui::icon_btn(
            ui,
            area(bar.left() + 70., cy - 16., 32., 32.),
            "shuffle",
            "Shuffle",
            false,
            hi,
        )
        .clicked()
        {
            menus::shuffle_ctx(v, &list_ctx, visible.as_ref());
        }
        let more = area(bar.left() + 116., cy - 16., 32., 32.);
        if ui::icon_btn(ui, more, "more", "More", false, hi).clicked() {
            menus::ctx_menu(
                list_ctx.clone(),
                pos2(more.left(), more.bottom() + 4.),
                visible.clone(),
            );
        }
        if show_list {
            let field = area(bar.right() - 220., cy - 16., 220., 32.);
            let id = ui.scope_id().with("filter");
            let focused = ui.memory(|m| m.has_focus(id));
            let p = ui.painter().clone();
            p.rect(
                field,
                16,
                Color32::from_black_alpha(115),
                (1., if focused { hi.ink(0.6) } else { HAIR2 }),
                StrokeKind::Inside,
            );
            glyph::draw(
                &p,
                area(field.left() + 11., cy - 7., 14., 14.),
                "search",
                FAINT,
            );
            // In a child, so placing the field leaves this page's own layout where it is.
            let edit = ui::child(ui, field, "filter field").put(
                Rect::from_min_max(
                    pos2(field.left() + 32., field.top() + 5.),
                    pos2(field.right() - 28., field.bottom() - 5.),
                ),
                egui::TextEdit::singleline(&mut v.views.filter)
                    .id(id)
                    .font(font("UI", 12.5))
                    .hint_text("Filter playlist")
                    .frame(egui::Frame::NONE),
            );
            if edit.changed() {
                ctx.request_repaint();
            }
            if !v.views.filter.is_empty() {
                let x = Rect::from_center_size(pos2(field.right() - 16., cy), Vec2::splat(26.));
                let clear = ui::hit(ui, x, "clear filter", "Clear");
                if clear.hovered() {
                    p.circle_filled(x.center(), 13., Color32::from_white_alpha(13));
                }
                glyph::draw(
                    &p,
                    Rect::from_center_size(x.center(), Vec2::splat(13.)),
                    "x",
                    if clear.hovered() { TEXT } else { MUTED },
                );
                if clear.clicked() {
                    v.views.filter.clear();
                    edit.request_focus();
                }
            }
        }
        if !show_list {
            return;
        }

        /* ---- the list ---- */
        let (head, _) = ui.allocate_exact_size(vec2(w, HEAD), Sense::hover());
        header(ui.painter(), head, &c);
        if n == 0 {
            ui.add_space(10.);
            menus::empty_note(ui, "No songs match");
            return;
        }
        let (body, _) = ui.allocate_exact_size(vec2(w, n as f32 * ROW), Sense::hover());
        let clip = ui.clip_rect();
        let first = ((clip.top() - body.top()) / ROW).floor().max(0.) as usize;
        let last = (((clip.bottom() - body.top()) / ROW).ceil().max(0.) as usize).min(n);
        let pointer = ctx.pointer_latest_pos();
        for (i, entry) in page.rows.iter().enumerate().take(last).skip(first) {
            let Some(song) = v.s.models.by_id.get(&entry.id) else {
                continue;
            };
            let r = area(body.left(), body.top() + i as f32 * ROW, w, ROW);
            let res = ui.interact(
                r,
                ui.scope_id().with(("row", entry.place)),
                Sense::click_and_drag(),
            );
            let selected = v.views.selection.indices.contains(&i);
            res.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::Role::ListBoxOption,
                    true,
                    selected,
                    format!("{} · {}", song.title, song.artist_name()),
                )
            });
            let over = !dragging && ui.rect_contains_pointer(r);
            let is_now = now.as_deref() == Some(entry.id.as_str());
            let p = ui.painter().clone();
            if selected {
                p.rect_filled(r, 0, hi.ink(0.1));
                p.vline(r.left() + 1., r.y_range(), (2., hi.ink(0.7)));
            } else if over {
                p.rect_filled(r, 0, Color32::from_white_alpha(11));
            }
            let cy = r.center().y;
            let num = Rect::from_center_size(pos2(c.num.center(), cy), Vec2::splat(30.));
            if over {
                let go = ui::hit(ui, num, ("go", i), "Play");
                glyph::draw(
                    &p,
                    Rect::from_center_size(num.center(), Vec2::splat(16.)),
                    if is_now && here && v.st.playing {
                        "pausef"
                    } else {
                        "playf"
                    },
                    TEXT,
                );
                if go.clicked() {
                    act = Some(Act::Play(i));
                }
            } else if is_now {
                menus::eq_bars(&p, num, v.st.playing, time, hi);
            } else {
                p.text(
                    num.center(),
                    Align2::CENTER_CENTER,
                    (i + 1).to_string(),
                    font("Mono", 12.5),
                    FAINT,
                );
            }
            ui::sleeve(
                &p,
                area(c.title.min, cy - 19., 38., 38.),
                v.covers.get(&song.cover, Size::Small, v.mono.then_some(hi)),
            );
            let x = c.title.min + 50.;
            let room = c.title.max - x;
            ui::label(
                &p,
                area(x, r.top() + 8., room, 19.),
                &song.title,
                "Strong",
                13.5,
                if is_now { hi.ink(1.) } else { TEXT },
            );
            let sub = area(x, r.top() + 27., room, 17.);
            if song.artist.is_empty() {
                ui::label(&p, sub, "Unknown artist", "UI", 12., FAINT);
            } else if link(ui, ("artist", i), sub, &song.artist, "UI", 12., FAINT) {
                act = Some(Act::Go(Route::Artist(song.artist_key.clone())));
            }
            if let Some(a) = c.album
                && !song.album.is_empty()
                && link(
                    ui,
                    ("album", i),
                    area(a.min, r.top(), a.span(), ROW),
                    &song.album,
                    "UI",
                    12.5,
                    MUTED,
                )
            {
                act = Some(Act::Go(Route::Album(song.album_key.clone())));
            }
            if let Some(e) = c.extra {
                ui::label(
                    &p,
                    area(e.min, r.top(), e.span(), ROW),
                    &menus::when(entry.added),
                    "Mono",
                    11.5,
                    MUTED,
                );
            }
            let liked = menus::is_liked(v.s, &entry.id);
            if liked || over {
                let b = Rect::from_center_size(pos2(c.like.center(), cy), Vec2::splat(30.));
                let like = ui::hit(
                    ui,
                    b,
                    ("like", i),
                    if liked {
                        "Remove from Liked"
                    } else {
                        "Add to Liked"
                    },
                );
                if like.hovered() {
                    p.rect(
                        b,
                        0,
                        Color32::from_white_alpha(13),
                        (1., HAIR),
                        StrokeKind::Inside,
                    );
                }
                if liked {
                    heart(&p, b, hi.ink(1.));
                } else {
                    glyph::draw(
                        &p,
                        Rect::from_center_size(b.center(), Vec2::splat(16.)),
                        "heart",
                        if like.hovered() { TEXT } else { MUTED },
                    );
                }
                if like.clicked() {
                    act = Some(Act::Like(entry.id.clone()));
                }
            }
            p.text(
                pos2(c.dur.max, cy),
                Align2::RIGHT_CENTER,
                if song.duration > 0. {
                    time_text(song.duration)
                } else {
                    "–".into()
                },
                font("Mono", 12.),
                FAINT,
            );
            if over || selected {
                let b = Rect::from_center_size(pos2(c.more.center(), cy), Vec2::splat(30.));
                let more = ui::hit(ui, b, ("more", i), "More");
                if more.hovered() {
                    p.rect(
                        b,
                        0,
                        Color32::from_white_alpha(13),
                        (1., HAIR),
                        StrokeKind::Inside,
                    );
                }
                glyph::draw(
                    &p,
                    Rect::from_center_size(b.center(), Vec2::splat(16.)),
                    "more",
                    if more.hovered() { TEXT } else { MUTED },
                );
                if more.clicked() {
                    act = Some(Act::Menu(i, b.right_bottom()));
                }
            }
            if res.double_clicked() {
                act = Some(Act::Play(i));
            } else if res.clicked() {
                let m = ui.input(|s| s.modifiers);
                act = Some(Act::Choose(i, m.ctrl, m.shift));
            }
            if res.secondary_clicked() {
                act = Some(Act::Menu(
                    i,
                    res.interact_pointer_pos().unwrap_or(r.center()),
                ));
            }
            if res.drag_started() {
                if !selected {
                    v.views.selection.choose(i, false, false);
                }
                res.dnd_set_drag_payload(v.views.selection.ids(&ids));
                act = Some(Act::Drag);
            }
        }
        if dragging
            && let Some(at) = pointer.filter(|p| body.expand2(vec2(0., ROW / 2.)).contains(*p))
        {
            let k = (((at.y - body.top()) / ROW).round().max(0.) as usize).min(n);
            ui.painter().hline(
                body.x_range(),
                body.top() + k as f32 * ROW,
                (2., hi.ink(1.)),
            );
            drop_at = Some(k);
        }
        ui.add_space(20.);
    });

    // The column names stay in view once the list scrolls under them.
    if show_list && out.state.clamped_offset().y > HERO + ACTIONS {
        let pin = area(outer.left(), outer.top(), w, 37.);
        ui.interact(pin, ui.scope_id().with("pinned head"), Sense::click());
        ui.painter()
            .rect_filled(pin, 0, Color32::from_rgba_unmultiplied(5, 5, 6, 235));
        header(ui.painter(), pin, &c);
    }

    let selected_places = |v: &View<'_>, page: &Page| -> Vec<usize> {
        v.views
            .selection
            .indices
            .iter()
            .filter_map(|&i| page.rows.get(i))
            .map(|r| r.place)
            .collect()
    };
    if dragging {
        let (released, down) = ctx.input(|i| (i.pointer.any_released(), i.pointer.any_down()));
        if released {
            if let Some(at) = drop_at {
                let places: Vec<_> = page
                    .drag
                    .iter()
                    .filter_map(|&i| page.rows.get(i))
                    .map(|r| r.place)
                    .collect();
                let before = page.rows.get(at).map_or(page.total, |r| r.place);
                v.s.playlist_move(&key, &places, before);
                let start = at - page.drag.iter().filter(|&&i| i < at).count();
                v.views.selection = Default::default();
                for i in start..start + places.len() {
                    v.views.selection.choose(i, true, false);
                }
            }
            page.drag.clear();
        } else if !down {
            page.drag.clear();
        }
        ctx.request_repaint();
    } else if !page.drag.is_empty() && !ctx.input(|i| i.pointer.any_down()) {
        page.drag.clear();
    }
    if !ctx.text_edit_focused() && !menus::is_open() && n > 0 {
        let (all, enter, delete) = ctx.input(|i| {
            (
                i.modifiers.ctrl && i.key_pressed(Key::A),
                i.key_pressed(Key::Enter),
                i.key_pressed(Key::Delete),
            )
        });
        if all {
            v.views.selection.indices.extend(0..n);
        }
        if enter && let Some(&row) = v.views.selection.indices.first() {
            act = Some(Act::Play(row));
        }
        if delete && !v.views.selection.indices.is_empty() {
            let places = selected_places(v, &page);
            v.s.playlist_remove(&key, &places);
            v.views.selection = Default::default();
        }
    }
    match act {
        Some(Act::Play(i)) => {
            if now.as_deref() == ids.get(i).map(String::as_str) && here {
                v.s.toggle(v.player);
            } else {
                v.s.play_ids(v.player, &ids, i as isize, list_ctx);
            }
        }
        Some(Act::Choose(i, ctrl, shift)) => v.views.selection.choose(i, ctrl, shift),
        Some(Act::Menu(i, at)) => {
            if !v.views.selection.indices.contains(&i) {
                v.views.selection.choose(i, false, false);
            }
            let row = v.views.selection.indices.first().copied().unwrap_or(i);
            menus::track_menu(
                v.views.selection.ids(&ids),
                at,
                Origin {
                    list: Some((
                        ids.clone(),
                        if v.views.selection.indices.len() > 1 {
                            row
                        } else {
                            i
                        },
                    )),
                    ctx: Some(list_ctx),
                    playlist: Some((key.clone(), selected_places(v, &page))),
                },
            );
        }
        Some(Act::Like(id)) => v.s.toggle_like(&[id]),
        Some(Act::Go(route)) => v.go(route),
        Some(Act::Drag) => {
            page.drag = if filtered {
                vec![]
            } else {
                v.views.selection.indices.iter().copied().collect()
            };
        }
        None => {}
    }
    *PAGE.lock().unwrap() = Some(page);
}
fn time_text(seconds: f64) -> String {
    time(seconds)
}
