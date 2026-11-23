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
    let song = v.s.bar_song(v.st);
    let duration = song.map_or(0., |s| v.st.duration.unwrap_or(s.duration));
    let position = if song.is_some() { v.st.position } else { 0. };
    let cy2 = r.top() + 65.;
    let seek_w = c2.min(620.);
    let seek = area(mid - seek_w / 2. + 58., cy2 - 9., seek_w - 116., 18.);
    let (response, val) = ui::rail(ui, seek, "Seek", (position / duration.max(0.01)) as f32, t);
    let preview = val.map_or(position, |n| f64::from(n) * duration);
    // the lit line along the top edge, brightest where the song has got to
    if duration > 0. {
        let at = r.left() + r.width() * (preview / duration).clamp(0., 1.) as f32;
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
        has_song && v.st.playing,
        t,
    )
    .clicked()
    {
        v.s.toggle(v.player);
    }
    if has_song && duration > 0. {
        if response.clicked_by(egui::PointerButton::Primary)
            || response.drag_stopped_by(egui::PointerButton::Primary)
        {
            v.s.seek(v.player, preview);
        }
        loop_controls(ui, v, &response, duration);
    }
    ui::spaced(
        &p,
        area(seek.left() - 58., cy2 - 9., 48., 18.),
        &time(preview),
        "Mono",
        11.,
        0.,
        FAINT,
        Align2::CENTER_CENTER,
    );
    let remain = v.s.setting("timeLeft") == true;
    let end = if remain {
        format!("-{}", time(duration - preview))
    } else {
        time(duration)
    };
    let total = area(seek.right() + 10., cy2 - 9., 48., 18.);
    ui::spaced(
        &p,
        total,
        &end,
        "Mono",
        11.,
        0.,
        FAINT,
        Align2::CENTER_CENTER,
    );
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
    if mini > x2 && v.s.setting("viz").as_bool().unwrap_or(true) {
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
    let (vol, n) = ui::rail(ui, rail, "Volume", v.s.audible_volume(), t);
    if let Some(n) = n {
        // Directly moving the volume slider also releases mute at its zero endpoint.
        if n == 0. && v.s.muted() {
            v.s.set_field("muted", false.into());
        }
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
            v.volume(v.s.audible_volume() + 0.05 * wheel.signum());
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

fn loop_controls(ui: &mut Ui, v: &mut View<'_>, response: &egui::Response, duration: f64) {
    let seek = response.rect;
    let mid = seek.center().x;
    let cy2 = seek.center().y;
    let loop_range = v.s.current().and_then(|s| v.s.loop_for(&s.id));
    let p = ui.painter().clone();
    let t = v.tint;
    let anchor = response.id.with("loop point");
    if response.secondary_clicked()
        && let Some(pt) = response.interact_pointer_pos()
    {
        let at = f64::from(((pt.x - seek.left()) / seek.width()).clamp(0., 1.)) * duration;
        ui.data_mut(|d| d.insert_temp(anchor, (v.st.key, at)));
    }
    response.context_menu(|menu| {
        let Some((key, at)) = menu.data(|d| d.get_temp::<(Option<u64>, f64)>(anchor)) else {
            return;
        };
        if key != v.st.key {
            menu.close();
            return;
        }
        menu.label(format!("Loop · at {}", time(at)));
        for (label, start) in [("Start the loop here", true), ("End the loop here", false)] {
            if menu.button(label).clicked() {
                v.s.set_loop(v.player, Some((start, at)));
                menu.close();
            }
        }
        if loop_range.is_some() {
            menu.separator();
            if menu.button("Stop looping").clicked() {
                v.s.set_loop(v.player, None);
                menu.close();
            }
        }
    });
    if let Some(lp) = loop_range {
        let x = |sec: f64| egui::lerp(seek.x_range(), (sec / duration).clamp(0., 1.) as f32);
        let band = Rect::from_min_max(
            pos2(x(lp.start), cy2 - 5.5),
            pos2(x(lp.end.unwrap_or(duration)), cy2 + 5.5),
        );
        p.rect_filled(band, 0, t.ink(0.14));
        if lp.start > 0. {
            p.vline(band.left(), band.y_range(), (2., t.ink(0.9)));
        }
        if lp.end.is_some() {
            p.vline(band.right(), band.y_range(), (2., t.ink(0.9)));
        }
        let a = if lp.start == 0. {
            "start".into()
        } else {
            time(lp.start)
        };
        let b = lp.end.map_or_else(|| "end".into(), time);
        let text = format!("{a}–{b}");
        let label = p.layout_no_wrap(text, font("Mono", 10.5), t.ink(1.));
        let width = label.size().x + 28.;
        // On narrower bars, float above the transport row so the chip cannot cover Next or Repeat.
        let left = (seek.right() - width).max(mid + 22.);
        let chip = if left >= mid + 105. {
            area(left, seek.top() - 19., width, 18.)
        } else {
            area(left, seek.top() - 56., width, 14.)
        };
        let hit = ui::hit(ui, chip, "loop chip", "Stop looping");
        p.rect_filled(chip, 2, RAISED);
        p.rect_stroke(
            chip,
            2,
            (1., t.ink(if hit.hovered() { 0.8 } else { 0.45 })),
            egui::StrokeKind::Inside,
        );
        ui::glyph::draw(
            &p,
            area(chip.left() + 5., chip.center().y - 6., 12., 12.),
            "repeat",
            t.ink(1.),
        );
        p.galley(
            pos2(chip.left() + 21., chip.center().y - label.size().y / 2.),
            label,
            t.ink(1.),
        );
        if hit.clicked() {
            v.s.set_loop(v.player, None);
        }
    }
}

#[cfg(test)]
mod player_tests {
    use super::*;
    use crate::{Args, platform::Platform, playback::Session};
    use egui::{Context, Event, Key, Modifiers, PointerButton, Pos2, RawInput, Shape};
    use serde_json::{Value, json};
    use std::sync::Arc;
    use string_audio::{Config, Host, Player, Status, Track};
    use string_platform::{
        errors_log::ErrorLog,
        instance::{self, Claim, Names},
    };

    fn with_bar(test: impl FnOnce(&Context, &mut View<'_>)) {
        struct Quiet;
        impl Host for Quiet {
            fn next_after(&mut self, _: &Track) -> Option<Track> {
                None
            }
            fn event(&mut self, _: string_audio::Event) {}
        }
        let dir = tempfile::tempdir().unwrap();
        let args = Args {
            root: dir.path().into(),
            explicit: true,
            smoke: Some(dir.path().into()),
            quit: false,
            add: None,
        };
        let names = Names::custom(&format!("StringBarTest{:x}", rand::random::<u64>()));
        let Claim::First(instance) = instance::claim_quietly(&names).unwrap() else {
            panic!()
        };
        let mut lib = string_core::library::Library::default();
        lib.tracks.push(
            serde_json::from_value(json!({"path":"test.wav","title":"Test","duration":200}))
                .unwrap(),
        );
        let mut s = Session::new(Default::default(), Default::default(), lib);
        assert!(
            s.state
                .queue
                .play_ids(&["test.wav".into()], 0, Value::Null, &s.models.catalog)
        );
        let ctx = Context::default();
        crate::theme::install(&ctx);
        let mut v = View {
            platform: &mut Platform::new(instance, &args, Arc::new(ErrorLog::new(dir.path()))),
            s: &mut s,
            player: &Player::new(
                Config {
                    muted: true,
                    ..Default::default()
                },
                Quiet,
            ),
            st: Status {
                key: Some(0),
                position: 10.,
                duration: Some(200.),
                ..Default::default()
            },
            views: &mut Default::default(),
            covers: &mut ui::covers::CoverCache::new(&ctx),
            viz: &Default::default(),
            scan: &mut Default::default(),
            tint: Tint::Mono,
            mono: true,
            force_mute: true,
        };
        test(&ctx, &mut v);
    }
    fn frame(ctx: &Context, v: &mut View<'_>, events: Vec<Event>) -> (Rect, Vec<Shape>) {
        let mut seek = Rect::NOTHING;
        let out = ctx.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1700., 500.))),
                events,
                ..Default::default()
            },
            |ui| {
                let mut bar = ui::child(ui, area(0., 416., 1700., 84.), "bar");
                crate::shell::shortcuts(ui, v);
                show(&mut bar, v);
                seek = ui
                    .ctx()
                    .read_response(bar.scope_id().with("Seek"))
                    .unwrap()
                    .rect;
            },
        );
        (seek, out.shapes.into_iter().map(|s| s.shape).collect())
    }
    fn button(at: Pos2, button: PointerButton, pressed: bool) -> Event {
        Event::PointerButton {
            pos: at,
            button,
            pressed,
            modifiers: Modifiers::NONE,
        }
    }
    fn painted(shapes: &[Shape], text: &str) -> bool {
        shapes
            .iter()
            .any(|s| matches!(s, Shape::Text(t) if t.galley.job.text == text))
    }
    #[test]
    fn loop_menu_uses_the_clicked_time_and_the_chip_leaves_the_rail_in_place() {
        with_bar(|ctx, v| {
            let (rail, _) = frame(ctx, v, vec![]);
            let click = |v: &mut View<'_>, at, which| {
                frame(ctx, v, vec![Event::PointerMoved(at)]);
                frame(ctx, v, vec![button(at, which, true)]);
                frame(ctx, v, vec![button(at, which, false)]);
            };
            let centre = |shapes: &[Shape], label: &str| {
                shapes
                    .iter()
                    .find_map(|s| match s {
                        Shape::Text(t) if t.galley.job.text == label => {
                            Some(t.pos + t.galley.size() * 0.5)
                        }
                        _ => None,
                    })
                    .unwrap_or_else(|| panic!("missing {label}"))
            };
            for (fraction, command) in [(0.25, "Start the loop here"), (0.75, "End the loop here")]
            {
                let at = pos2(egui::lerp(rail.x_range(), fraction), rail.center().y);
                click(v, at, PointerButton::Secondary);
                frame(ctx, v, vec![]);
                let (_, menu) = frame(ctx, v, vec![]);
                click(v, centre(&menu, command), PointerButton::Primary);
                assert_eq!(frame(ctx, v, vec![]).0, rail);
            }
            let lp = v.s.loop_for("test.wav").unwrap();
            assert!((lp.start - 50.).abs() < 0.001 && (lp.end.unwrap() - 150.).abs() < 0.001);
            let (_, shapes) = frame(ctx, v, vec![]);
            let band = Rect::from_min_max(
                pos2(egui::lerp(rail.x_range(), 0.25), rail.center().y - 5.5),
                pos2(egui::lerp(rail.x_range(), 0.75), rail.center().y + 5.5),
            );
            assert!(shapes.iter().any(|s| matches!(s, Shape::Rect(r) if r.rect == band)));
            let chip = centre(&shapes, "0:50–2:30");
            assert!(chip.y < rail.top());
            click(v, chip, PointerButton::Primary);
            assert!(v.s.loop_for("test.wav").is_none());
            assert_eq!(frame(ctx, v, vec![]).0, rail);
            v.s.set_loop(v.player, Some((true, 20.)));
            click(v, rail.center(), PointerButton::Secondary);
            frame(ctx, v, vec![]);
            let (_, menu) = frame(ctx, v, vec![]);
            click(v, centre(&menu, "Stop looping"), PointerButton::Primary);
            assert!(v.s.loop_for("test.wav").is_none());
            assert_eq!(v.s.seek_revision, 0);
        });
    }
    #[test]
    fn bar_waits_for_the_engine_key_before_showing_the_selected_title() {
        with_bar(|ctx, v| {
            v.st.key = Some(99);
            assert!(painted(&frame(ctx, v, vec![]).1, "Nothing playing"));
            v.st.key = Some(0);
            assert!(painted(&frame(ctx, v, vec![]).1, "Test"));
        });
    }
    #[test]
    fn seek_drag_previews_time_and_commits_once_on_release() {
        with_bar(|ctx, v| {
            let (r, _) = frame(ctx, v, vec![]);
            let at = |f| pos2(egui::lerp(r.x_range(), f), r.center().y);
            frame(ctx, v, vec![Event::PointerMoved(at(0.25))]);
            frame(ctx, v, vec![button(at(0.25), PointerButton::Primary, true)]);
            let (_, shapes) = frame(ctx, v, vec![Event::PointerMoved(at(0.5))]);
            assert_eq!(v.s.seek_revision, 0, "the pointer is still down");
            assert!(painted(&shapes, "1:40"));
            frame(
                ctx,
                v,
                vec![
                    Event::PointerMoved(at(1.2)),
                    button(at(1.2), PointerButton::Primary, false),
                ],
            );
            assert_eq!(v.s.seek_revision, 1);
            assert_eq!(v.s.seek_position, 200.);
            frame(ctx, v, vec![]);
            assert_eq!(v.s.seek_revision, 1);
        });
    }
    #[test]
    fn spectrum_switch_controls_the_painted_bars() {
        with_bar(|ctx, v| {
            for (on, count) in [(true, 18), (false, 0)] {
                v.s.set_setting("viz", on.into());
                let (_, shapes) = frame(ctx, v, vec![]);
                let bars = shapes.iter().filter(|s| matches!(s,Shape::Rect(r) if (r.rect.width()-2.2).abs()<0.001 && (r.rect.height()-2.).abs()<0.001)).count();
                assert_eq!(bars, count);
            }
        });
    }
    #[test]
    fn volume_keys_start_at_zero_while_muted() {
        with_bar(|ctx, v| {
            for (key, vol, muted) in [(Key::ArrowDown, 0., true), (Key::ArrowUp, 0.05, false)] {
                v.s.set_field("muted", true.into());
                v.s.set_field("vol", 0.8.into());
                frame(
                    ctx,
                    v,
                    vec![Event::Key {
                        key,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: Modifiers::CTRL,
                    }],
                );
                assert_eq!((v.s.volume(), v.s.muted()), (vol, muted));
            }
        });
    }
}
