mod lines;
pub(crate) mod smoke;
mod worker;

use crate::{
    model::Song,
    theme::*,
    ui,
    view::{Overlay, View},
};
use crossbeam_channel::Receiver;
use egui::{Context, Key, Modifiers, RichText, Ui, vec2};
use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};
use string_lyrics::{Lookup, Lyrics, LyricsResult, Ollama, Source};
use worker::{LanguageModel, Translator};

const SETTLE: Duration = Duration::from_millis(1200);

pub struct LyricsView {
    lyrics: Arc<Lyrics>,
    translator: Translator,
    page: Option<Page>,
}

struct Page {
    id: String,
    entered: Instant,
    permission: Option<bool>,
    online_started: bool,
    force: bool,
    lookup: Option<Receiver<LyricsResult>>,
    found: Option<LyricsResult>,
    scroll: lines::Follow,
}
impl Page {
    fn online_wait(&self, now: Instant) -> Option<Duration> {
        if self.online_started || self.permission != Some(true) {
            return None;
        }
        self.found
            .as_ref()
            .filter(|r| !r.synced && !r.online)
            .map(|_| SETTLE.saturating_sub(now.saturating_duration_since(self.entered)))
    }
}

fn fixture_url(var: &str) -> String {
    let port = std::env::var(var)
        .ok()
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);
    format!("http://127.0.0.1:{port}")
}

impl LyricsView {
    pub fn new(root: &Path, smoke: bool) -> Self {
        let (lyrics, translator) = if smoke {
            (
                Lyrics::with_base_url(root, &fixture_url("STRING_SMOKE_LRCLIB_PORT")),
                Ollama::with_base_url(root, &fixture_url("STRING_SMOKE_OLLAMA_PORT"))
                    .expect("loopback fixture URL"),
            )
        } else {
            (Lyrics::new(root), Ollama::new(root))
        };
        Self {
            lyrics: Arc::new(lyrics),
            translator: Translator::new(translator),
            page: None,
        }
    }

    pub(crate) fn ollama(&self) -> Arc<Ollama> {
        self.translator.service.clone()
    }

    pub fn check_visible(&mut self, overlay: Option<Overlay>, minimized: bool, song: Option<&str>) {
        if self
            .page
            .as_ref()
            .is_some_and(|p| minimized || overlay != Some(Overlay::Lyrics) || song != Some(&p.id))
        {
            self.page = None;
        }
        if self.page.is_none() {
            self.translator.poll();
        }
    }

    fn open(&mut self, ctx: &Context, song: &Song, permission: Option<bool>, force: bool) {
        self.translator.page_opened(&song.id, force);
        self.page = Some(Page {
            id: song.id.clone(),
            entered: Instant::now(),
            permission,
            online_started: false,
            force,
            lookup: Some(lookup(
                ctx,
                &self.lyrics,
                song,
                if permission == Some(true) {
                    Lookup::BeforeOnlineDelay
                } else {
                    Lookup::Offline
                },
                force,
            )),
            found: None,
            scroll: Default::default(),
        });
    }

    fn update(&mut self, ctx: &Context, song: &Song, permission: Option<bool>) {
        if self
            .page
            .as_ref()
            .is_none_or(|p| p.id != song.id || p.permission != permission)
        {
            self.open(ctx, song, permission, false);
        }
        let page = self.page.as_mut().unwrap();
        if let Some(result) = page.lookup.as_ref().and_then(|rx| rx.try_recv().ok()) {
            if page
                .found
                .as_ref()
                .is_some_and(|old| old.lines != result.lines)
            {
                self.translator.clear();
                page.scroll = Default::default();
            }
            page.found = Some(result);
            page.lookup = None;
        }
        if let Some(wait) = page.online_wait(Instant::now()) {
            if wait.is_zero() {
                page.online_started = true;
                page.lookup = Some(lookup(ctx, &self.lyrics, song, Lookup::Online, page.force));
            } else {
                ctx.request_repaint_after(wait);
            }
        }
    }
}

fn lookup(
    ctx: &Context,
    service: &Arc<Lyrics>,
    song: &Song,
    phase: Lookup,
    force: bool,
) -> Receiver<LyricsResult> {
    let (tx, rx) = crossbeam_channel::bounded(1);
    let (ctx, service, song) = (ctx.clone(), service.clone(), song.clone());
    std::thread::spawn(move || {
        let song = song.lyrics_song();
        let result = service.lookup(&song, phase, force);
        let _ = tx.send(result);
        ctx.request_repaint();
    });
    rx
}

fn source(song: &Song, found: Option<&LyricsResult>) -> String {
    let Some(r) = found else { return String::new() };
    let mut text = match r.source {
        Source::Lrc => format!(
            "From {}",
            Path::new(&song.path)
                .with_extension("lrc")
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        ),
        Source::File => "From the song file".to_owned(),
        Source::Lrclib => "Lyrics from LRCLIB".to_owned(),
        Source::None => return String::new(),
    };
    if !r.synced && !r.instrumental {
        text.push_str(" · not synced");
    }
    text
}

pub fn show(ui: &mut Ui, v: &mut View<'_>) {
    let Some(song) = v.s.current().cloned() else {
        return;
    };
    let Some(state) = &mut v.views.lyrics else {
        return;
    };
    let permission = v.s.setting("lyricsOnline").as_bool();
    state.update(ui.ctx(), &song, permission);
    let page = state.page.as_mut().unwrap();
    let found = page.found.as_ref();
    let has = found.is_some_and(|r| !r.lines.is_empty() && !r.instrumental);
    let enabled = v.s.setting("translate").as_bool() == Some(true);
    let mut options = LanguageModel::from_settings(v.s);
    let r = ui.available_rect_before_wrap();
    let header = r.with_max_y(r.top() + 48.);
    let mut controls = ui::child(ui, header.shrink2(vec2(6., 0.)), "lyric controls");
    let mut label_end = header.right() - 6.;
    controls.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if has {
            let (box_rect, _) = ui.allocate_exact_size(vec2(135., 26.), egui::Sense::hover());
            let menu_rect = box_rect.with_min_x(box_rect.right() - 22.);
            let toggle_rect = box_rect.with_max_x(menu_rect.left());
            let menu = ui::hit(
                ui,
                menu_rect,
                "translation preferences",
                "Translation language and model",
            )
            .on_hover_text("Translation language and model");
            egui::Popup::menu(&menu).show(|ui| state.translator.settings(ui, &mut options));
            let toggle = ui::hit(ui, toggle_rect, "translate", "Translate on this PC (T)")
                .on_hover_text("Translate on this PC (T)");
            let ink = if enabled || toggle.hovered() {
                TEXT
            } else {
                MUTED
            };
            let p = ui.painter();
            p.rect(
                box_rect,
                0,
                if enabled {
                    v.tint.ink(0.08)
                } else {
                    egui::Color32::TRANSPARENT
                },
                (1., if enabled { v.tint.ink(0.6) } else { HAIR2 }),
                egui::StrokeKind::Inside,
            );
            ui::glyph::draw(
                p,
                ui::area(box_rect.left() + 9., box_rect.center().y - 6.5, 13., 13.),
                "globe",
                ink,
            );
            ui::label(
                p,
                toggle_rect.with_min_x(box_rect.left() + 30.),
                "TRANSLATE",
                "HUD",
                11.,
                ink,
            );
            p.vline(
                menu_rect.left(),
                menu_rect.y_range().shrink(5.),
                (1., HAIR2),
            );
            ui::glyph::draw(
                p,
                egui::Rect::from_center_size(menu_rect.center(), vec2(11., 11.)),
                "down",
                MUTED,
            );
            if toggle.clicked()
                || (!ui.ctx().text_edit_focused()
                    && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::T)))
            {
                v.s.set_setting("translate", (!enabled).into());
            }
        }
        let src = source(&song, found);
        if found.is_some_and(|r| r.source == Source::Lrclib) {
            ui.hyperlink_to(mono(&src, 10.5).color(FAINT), "https://lrclib.net");
        } else if !src.is_empty() {
            ui.label(mono(&src, 10.5).color(FAINT));
        }
        label_end = ui.cursor().right() - 10.;
    });
    options.save_changes(v.s);
    let p = ui.painter();
    let left = header.left() + 6.;
    ui::label(
        p,
        ui::area(left, header.top(), label_end - left, 22.),
        &song.title,
        "Strong",
        15.,
        TEXT,
    );
    ui::label(
        p,
        ui::area(left, header.top() + 22., label_end - left, 17.),
        song.artist_name(),
        "Mono",
        12.,
        FAINT,
    );
    p.hline(header.x_range(), header.bottom(), (1., HAIR));
    let is_on = v.s.setting("translate").as_bool() == Some(true);
    if let Some(found) = found.filter(|_| has && is_on) {
        state.translator.update(ui.ctx(), &song, found, options);
    } else if !is_on || found.is_some() {
        state.translator.clear();
    }
    let status = &state.translator.status;
    let status_height = if has { 27. } else { 0. };
    if !status.is_empty() {
        let rect = ui::area(left, header.bottom() + 3., r.width() - 12., 22.);
        ui::label(ui.painter(), rect, status, "Mono", 11., FAINT);
        ui.interact(
            rect,
            ui.scope_id().with("translation status"),
            egui::Sense::hover(),
        )
        .on_hover_text(status);
    }
    let body = r
        .with_min_y(header.bottom() + status_height)
        .with_max_y(r.bottom() - 24.)
        .shrink2(vec2(6., 0.));
    let mut body = ui::child(ui, body, "words");
    let pending = found.is_none()
        || found.is_some_and(|r| {
            r.source == Source::None
                && permission == Some(true)
                && (!page.online_started || page.lookup.is_some())
                && !r.online
        });
    if pending {
        body.add_space(30.);
        for fraction in [0.84, 0.72, 0.62, 0.84, 0.72, 0.62, 0.84] {
            let (row, _) = body.allocate_exact_size(
                vec2(body.available_width() * fraction, 22.),
                egui::Sense::hover(),
            );
            body.painter().rect_filled(row, 2, HAIR);
            body.add_space(16.);
        }
    } else if let Some(result) = found {
        if has {
            if let Some(t) = page.scroll.show(
                &mut body,
                &page.id,
                result,
                &state.translator,
                v.st.position,
                v.tint,
            ) {
                v.s.seek(v.player, t);
            }
        } else if let Some(action) = empty(&mut body, permission, result) {
            let allow = match action {
                Action::Allow => Some(true),
                Action::Deny => Some(false),
                Action::Retry => permission,
            };
            if allow != permission {
                v.s.set_setting("lyricsOnline", allow.into());
            }
            state.open(ui.ctx(), &song, allow, !matches!(action, Action::Deny));
        }
    }
}

enum Action {
    Allow,
    Deny,
    Retry,
}

fn empty(ui: &mut Ui, allow: Option<bool>, result: &LyricsResult) -> Option<Action> {
    let ask = !result.instrumental && allow.is_none();
    let (title, text) = if result.instrumental {
        ("Instrumental", "No words in this one.")
    } else if ask {
        (
            "Look for lyrics online?",
            "This song has no lyrics of its own. String can ask LRCLIB (lrclib.net), a free, open lyrics library, which often has them synced to the music. It sends the song's title, artist, album and length, nothing else, and keeps what it finds on this PC.",
        )
    } else if result.busy {
        ("LRCLIB is busy", "Try again in a minute.")
    } else if allow == Some(true) {
        (
            "No lyrics for this one",
            "LRCLIB doesn't have this song (or not under this title).",
        )
    } else {
        (
            "No lyrics for this one",
            "Looking online is off (Library → Playback).",
        )
    };
    ui.add_space(((ui.available_height() - if ask { 174. } else { 134. }) * 0.5).max(0.));
    ui.set_max_width(ui.available_width().min(540.));
    ui.label(RichText::new(title).font(font("Strong", 26.)));
    ui.label(RichText::new(text).font(font("UI", 13.5)).color(MUTED));
    if result.instrumental {
        return None;
    }
    ui.add_space(4.);
    if ask {
        let action = ui
            .horizontal_wrapped(|ui| {
                if ui
                    .add(
                        egui::Button::new(hud("LOOK THEM UP", 11.).color(BG))
                            .fill(TEXT)
                            .corner_radius(1)
                            .min_size(vec2(110., 29.)),
                    )
                    .clicked()
                {
                    return Some(Action::Allow);
                }
                ui.add(
                    egui::Button::new(hud("ONLY SONGS' OWN LYRICS", 11.).color(MUTED))
                        .frame(false)
                        .min_size(vec2(158., 29.)),
                )
                .clicked()
                .then_some(Action::Deny)
            })
            .inner;
        ui.label(mono("You can change this in Library → Playback.", 11.5).color(FAINT));
        action
    } else {
        ui.label(
            mono(
                "Lyrics of your own: put a .lrc file named like the song next to it.",
                11.5,
            )
            .color(FAINT),
        );
        ui.button(hud("LOOK AGAIN", 11.))
            .clicked()
            .then_some(Action::Retry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(super) fn fixture() -> (tempfile::TempDir, Song, LyricsView, LanguageModel) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("song.flac");
        let lib =
            serde_json::from_value(serde_json::json!({"v":1,"tracks":[{"path":path}]})).unwrap();
        let session = crate::playback::Session::new(Default::default(), Default::default(), lib);
        let song = session.models.by_id.values().next().unwrap().clone();
        let view = LyricsView::new(dir.path(), true);
        let options = LanguageModel::from_settings(&session);
        (dir, song, view, options)
    }

    #[test]
    fn hiding_lyrics_preserves_the_translation_job() {
        let (_dir, song, mut view, options) = fixture();
        let ctx = Context::default();
        for (overlay, minimized) in [
            (Some(Overlay::Record), false),
            (Some(Overlay::Lyrics), true),
        ] {
            view.open(&ctx, &song, None, false);
            let result = view.lyrics.get(&song.lyrics_song(), false, false);
            view.translator
                .update(&ctx, &song, &result, options.clone());
            view.check_visible(overlay, minimized, Some(&song.id));
            assert!(!view.translator.status.is_empty());
        }
    }

    #[test]
    fn leaving_during_look_again_keeps_the_cached_online_answer() {
        let (dir, song, mut view, _) = fixture();
        let ctx = Context::default();
        view.lyrics.get(&song.lyrics_song(), false, false);
        let file = std::fs::read_dir(dir.path().join("lyrics"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let record = serde_json::json!({"v":1,"source":"none","online":true,"asked":true,"at":chrono::Utc::now().timestamp_millis()}).to_string();
        std::fs::write(&file, &record).unwrap();
        view.lyrics.forget(&song.id);
        view.open(&ctx, &song, Some(true), true);
        view.page
            .as_mut()
            .unwrap()
            .lookup
            .take()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        view.check_visible(None, false, Some(&song.id));
        assert_eq!(std::fs::read_to_string(file).unwrap(), record);
        view.open(&ctx, &song, Some(true), false);
        let page = view.page.as_mut().unwrap();
        page.found = Some(
            page.lookup
                .take()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .unwrap(),
        );
        assert_eq!(page.online_wait(Instant::now() + SETTLE), None);
    }

    #[test]
    fn leaving_lyrics_discards_the_online_deadline() {
        let now = Instant::now();
        let mut view = LyricsView::new(Path::new("."), true);
        let mut page = Page {
            id: "one".into(),
            entered: now,
            permission: None,
            online_started: false,
            force: false,
            lookup: None,
            found: Some(LyricsResult {
                source: Source::None,
                text: String::new(),
                synced: false,
                lines: vec![],
                instrumental: false,
                online: false,
                busy: false,
                retry: false,
            }),
            scroll: Default::default(),
        };
        assert_eq!(page.online_wait(now + SETTLE), None);
        page.permission = Some(false);
        assert_eq!(page.online_wait(now + SETTLE), None);
        page.permission = Some(true);
        assert_eq!(
            page.online_wait(now + Duration::from_millis(1199)),
            Some(Duration::from_millis(1))
        );
        assert_eq!(page.online_wait(now + SETTLE), Some(Duration::ZERO));
        page.online_started = true;
        assert_eq!(page.online_wait(now + SETTLE), None);
        view.page = Some(page);
        view.check_visible(Some(Overlay::Lyrics), false, Some("one"));
        assert!(view.page.is_some());
        view.check_visible(Some(Overlay::Record), false, Some("one"));
        assert!(view.page.is_none());
    }
}
