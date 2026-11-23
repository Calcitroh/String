use crate::{
    theme::*,
    ui::{self, area, covers::Size},
    view::{Overlay, View},
};
use egui::{Align2, Color32, Rect, Ui, Vec2, pos2, vec2};
use string_core::queue::Repeat;
/// The player bar, laid out as the web version's grid: three columns (`minmax(200px, 1fr) minmax(320px, 1.6fr)
/// minmax(200px, 1fr)`, 18 px gaps and padding): the song on the left, the controls and seek bar in the middle, the
/// rest on the right.
pub fn show(ui: &mut Ui, v: &mut View<'_>) {
    let r = ui.max_rect();
    let p = ui.painter().clone();
    let t = v.tint;
    p.rect_filled(r, 0, DEEP);
    ui::gradient(
        &p,
        r.with_max_y(r.top() + r.height() * 0.6),
        white(0.035),
        Color32::TRANSPARENT,
        false,
    );
    p.hline(r.x_range(), r.top(), (1., HAIR2));
    let fr = (r.width() - 72.) / 3.6;
    let (c1, c2) = (fr.max(200.), (fr * 1.6).max(320.));
    let x0 = r.left() + 18.;
    let x1 = x0 + c1 + 18.;
    let x2 = x1 + c2 + 18.;
    let mid = x1 + c2 / 2.;
    let cy = r.center().y;
    let y = r.top() + 30.;
    let song = v.s.current();
    let duration =
        v.st.duration
            .or_else(|| song.map(|s| s.duration))
            .unwrap_or_default();
    // the lit line along the top edge, brightest where the song has got to
    if duration > 0. {
        let at = r.left() + r.width() * (v.st.position / duration).clamp(0., 1.) as f32;
        let line = Rect::from_min_max(pos2(r.left(), r.top()), pos2(at, r.top() + 1.));
        ui::gradient(&p, line, Color32::TRANSPARENT, t.ink(0.35), true);
        let tail = Rect::from_min_max(
            pos2(at, r.top()),
            pos2((at + r.width() * 0.12).min(r.right()), r.top() + 1.),
        );
        ui::gradient(&p, tail, t.ink(0.35), Color32::TRANSPARENT, true);
    }
    // left: the cover, the title over "artist · album", and the like button just after the longer of the two
    let sr = area(x0, cy - 28., 56., 56.);
    ui::sleeve(
        &p,
        sr,
        song.and_then(|s| v.covers.get(&s.cover, Size::Small, v.mono.then_some(t))),
    );
    let text_x = sr.right() + 13.;
    let text_w = (c1 - 56. - 13. - 13. - 32.).max(40.);
    let title = ui::spaced(
        &p,
        area(text_x, cy - 18., text_w, 18.),
        song.map_or("Nothing playing", |s| &s.title),
        "Medium",
        13.5,
        0.,
        TEXT,
        Align2::LEFT_CENTER,
    );
    let sub = ui::spaced(
        &p,
        area(text_x, cy + 2., text_w, 16.),
        &song.map_or_else(String::new, |s| {
            if s.album.is_empty() {
                s.artist_name().to_owned()
            } else {
                format!("{} · {}", s.artist_name(), s.album)
            }
        }),
        "UI",
        12.,
        0.,
        FAINT,
        Align2::LEFT_CENTER,
    );
    let text_end = title.right().max(sub.right()).max(text_x);
    let liked = song.is_some_and(|s| !v.s.field("liked")[&s.id].is_null());
    let has_song = song.is_some();
    let info = ui::hit(
        ui,
        area(sr.left(), sr.top(), text_end - sr.left(), 56.),
        "jump song",
        "Show current song",
    );
    if let (Some(at), Some(s)) = (ui::menus::menu_pos(&info), song) {
        ui::menus::track_menu(vec![s.id.clone()], at, ui::menus::Origin::none());
    }
    if info.clicked() {
        v.jump_current();
    }
    if has_song
        && ui::like_btn(
            ui,
            area(text_end + 13., cy - 16., 32., 32.),
            "Like current song",
            liked,
            t,
        )
        .clicked()
    {
        v.like_current();
    }
    // middle: shuffle, previous, play, next, repeat (10 px apart), the seek bar under them
    for (dx, icon, name, on) in [
        (-88., "shuffle", "Shuffle", v.s.state.queue.shuffled()),
        (-46., "prev", "Previous", false),
        (46., "next", "Next", false),
        (
            88.,
            "repeat",
            "Repeat",
            v.s.state.queue.repeat() != Repeat::Off,
        ),
    ] {
        let b = Rect::from_center_size(pos2(mid + dx, y), vec2(32., 32.));
        let mode = matches!(icon, "shuffle" | "repeat");
        if ui::round_btn(ui, b, icon, name, on, mode, t).clicked() {
            match icon {
                "shuffle" => v.shuffle(),
                "prev" => v.s.previous(v.player),
                "next" => v.s.next(v.player),
                _ => v.repeat(),
            }
        }
        if icon == "repeat" && v.s.state.queue.repeat() == Repeat::One {
            ui::label(
                &p,
                area(b.right() - 9., b.top() + 1., 8., 10.),
                "1",
                "Mono",
                8.5,
                t.ink(1.),
            );
        }
    }
    if ui::play_big(
        ui,
        Rect::from_center_size(pos2(mid, y), Vec2::splat(40.)),
        v.st.playing,
        t,
    )
    .clicked()
    {
        v.s.toggle(v.player);
    }
    let cy2 = r.top() + 65.;
    let seek_w = c2.min(620.);
    let seek = area(mid - seek_w / 2. + 58., cy2 - 9., seek_w - 116., 18.);
    let (_, val) = ui::rail(
        ui,
        seek,
        "Seek",
        (v.st.position / duration.max(0.01)) as f32,
        t,
    );
    if let Some(n) = val {
        v.s.seek(v.player, n as f64 * duration);
    }
    ui::spaced(
        &p,
        area(seek.left() - 58., cy2 - 9., 48., 18.),
        &time(v.st.position),
        "Mono",
        11.,
        0.,
        FAINT,
        Align2::CENTER_CENTER,
    );
    let remain = v.s.setting("timeLeft") == true;
    let end = if remain {
        format!("-{}", time(duration - v.st.position))
    } else {
        time(duration)
    };
    let total = area(seek.right() + 10., cy2 - 9., 48., 18.);
    ui::spaced(&p, total, &end, "Mono", 11., 0., FAINT, Align2::CENTER_CENTER);
    if ui::hit(ui, total, "duration", "Toggle time remaining").clicked() {
        v.s.set_setting("timeLeft", (!remain).into());
    }
    // right, from the edge in: Now Playing, the volume, mute, the queue, lyrics and the small visualizer (6 px apart)
    let right = r.right() - 18.;
    let expand = area(right - 32., cy - 16., 32., 32.);
    let vol_width = if r.width() > 1100. { 110. } else { 56. };
    let rail = area(expand.left() - 6. - vol_width, cy - 11., vol_width, 22.);
    let vol_start = rail.left() - 6. - 32.;
    let q = vol_start - 38.;
    let lyr = q - 38.;
    let mini = lyr - 12. - 92.;
    if mini > x2 {
        for i in 0..18 {
            let band = ((i as f32 / 18.).powf(1.25) * 64. * 0.82) as usize;
            let level = v.viz.level[band];
            let h = (level * 28.).max(2.);
            p.rect_filled(
                area(mini + 8. + i as f32 * 4.1, cy + 13. - h, 2.2, h),
                0,
                t.ink((0.24 + level * 0.7) * 0.9),
            );
        }
    }
    if ui::round_btn(
        ui,
        area(lyr, cy - 16., 32., 32.),
        "mic",
        "Lyrics",
        v.views.overlay == Some(Overlay::Lyrics),
        false,
        t,
    )
    .clicked()
    {
        v.record(Overlay::Lyrics);
    }
    ui::queue_drawer::button(ui, v, area(q, cy - 16., 32., 32.));
    let mute = ui::round_btn(
        ui,
        area(vol_start, cy - 16., 32., 32.),
        if v.s.muted() { "mute" } else { "vol" },
        "Mute",
        v.s.muted(),
        false,
        t,
    );
    if mute.clicked() {
        v.mute();
    }
    let (vol, n) = ui::rail(
        ui,
        rail,
        "Volume",
        if v.s.muted() { 0. } else { v.s.volume() },
        t,
    );
    if let Some(n) = n {
        v.volume(n);
    }
    if vol.hovered() || mute.hovered() {
        let wheel = ui.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::MouseWheel { delta, .. } => Some(delta.y),
                    _ => None,
                })
                .sum::<f32>()
        });
        if wheel != 0. {
            v.volume(v.s.volume() + 0.05 * wheel.signum());
        }
    }
    if ui::round_btn(
        ui,
        expand,
        "expand",
        "Now Playing",
        v.views.overlay.is_some(),
        false,
        t,
    )
    .clicked()
    {
        v.record(Overlay::Record);
    }
}
