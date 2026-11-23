use super::worker::Translator;
use crate::theme::{self, Tint, font};
use egui::{Align, Color32, Galley, Painter, Pos2, Rect, ScrollArea, Sense, Ui, Vec2, pos2, vec2};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use string_lyrics::{Line, LyricsResult, line_at};

#[derive(Default)]
pub(super) struct Follow {
    offsets: Vec<f32>,
    layout: Option<(f32, f32, f32, u64)>,
    target: Option<Option<usize>>,
    manual_until: Option<Instant>,
}

struct Row {
    words: Arc<Galley>,
    translation: Option<Arc<Galley>>,
}

impl Row {
    fn layout(p: &Painter, line: &Line, translated: Option<&str>, size: f32, width: f32) -> Self {
        let (text, size) = if line.text.is_empty() {
            ("♪", 20.)
        } else {
            (line.text.as_str(), size)
        };
        let words = p.layout(text.to_owned(), font("Strong", size), Color32::WHITE, width);
        let translation = translated
            .map(|s| p.layout(s.to_owned(), font("UI", size * 0.5), Color32::WHITE, width));
        Self { words, translation }
    }
    fn height(&self) -> f32 {
        self.words.size().y + self.translation.as_ref().map_or(0., |g| g.size().y + 4.) + 14.
    }
}

// Clip each fade band separately so even a paragraph taller than the viewport stays legible.
fn faded(p: &Painter, view: Rect, pos: Pos2, g: &Arc<Galley>, color: Color32) {
    let text = Rect::from_min_size(pos, g.size());
    let top = view.top() + view.height() * 0.14;
    let bottom = view.top() + view.height() * 0.78;
    let paint = |lo: f32, hi: f32, opacity: f32| {
        if text.bottom() >= lo && text.top() <= hi {
            let clip = view.with_min_y(lo).with_max_y(hi).intersect(p.clip_rect());
            p.with_clip_rect(clip).galley_with_override_text_color(
                pos,
                g.clone(),
                color.gamma_multiply(opacity),
            );
        }
    };
    paint(top, bottom, 1.);
    for (lo, hi, reverse) in [(view.top(), top, false), (bottom, view.bottom(), true)] {
        if text.top() >= hi || text.bottom() <= lo {
            continue;
        }
        for i in 0..16 {
            let a = i as f32 / 16.;
            let b = (i + 1) as f32 / 16.;
            let alpha = (a + b) * 0.5;
            paint(
                egui::lerp(lo..=hi, a),
                egui::lerp(lo..=hi, b),
                if reverse { 1. - alpha } else { alpha },
            );
        }
    }
}

impl Follow {
    pub fn show(
        &mut self,
        ui: &mut Ui,
        id: &str,
        lyrics: &LyricsResult,
        tr: &Translator,
        time: f64,
        tint: Tint,
    ) -> Option<f64> {
        let view = ui.max_rect();
        let now = Instant::now();
        if ui.rect_contains_pointer(view)
            && ui.input(|i| i.smooth_scroll_delta().y != 0. || i.pointer.is_decidedly_dragging())
        {
            self.manual_until = Some(now + Duration::from_millis(3500));
            self.target = None;
        }
        if let Some(until) = self.manual_until
            && now < until
        {
            ui.ctx().request_repaint_after(until - now);
        }
        let active = lyrics
            .synced
            .then(|| line_at(&lyrics.lines, time + 0.15))
            .flatten();
        let screen = ui.ctx().content_rect();
        let size = (screen.width() * 0.023).clamp(20., 32.);
        let pad = screen.height() * 0.32;
        let mut seek = None;
        ScrollArea::vertical()
            .id_salt(("lyrics", id))
            .auto_shrink([false, false])
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .show_viewport(ui, |ui, viewport| {
                let width = ui.available_width();
                let layout = (width, size, ui.ctx().pixels_per_point(), tr.layout_revision);
                let row_layout = |p: &Painter, i: usize, size| {
                    Row::layout(
                        p,
                        &lyrics.lines[i],
                        tr.rows.get(i).and_then(Option::as_deref),
                        size,
                        width,
                    )
                };
                if self.layout != Some(layout) {
                    self.offsets.clear();
                    self.offsets.push(0.);
                    let mut offset = 0.;
                    for i in 0..lyrics.lines.len() {
                        offset += row_layout(ui.painter(), i, size).height();
                        self.offsets.push(offset);
                    }
                    self.layout = Some(layout);
                    self.target = None;
                }
                let height = self.offsets.last().copied().unwrap_or(0.);
                let origin = ui.max_rect().min + vec2(0., pad);
                ui.set_height(height + pad * 2.);
                let row_rect = |i: usize| {
                    Rect::from_min_max(
                        origin + vec2(0., self.offsets[i]),
                        origin + vec2(width, self.offsets[i + 1]),
                    )
                };
                if lyrics.synced
                    && !lyrics.lines.is_empty()
                    && self.target != Some(active)
                    && self.manual_until.is_none_or(|until| now >= until)
                {
                    let center =
                        row_rect(active.unwrap_or(0)).center() + vec2(0., view.height() * 0.12);
                    ui.scroll_to_rect(
                        Rect::from_center_size(center, Vec2::ZERO),
                        Some(Align::Center),
                    );
                    self.target = Some(active);
                }
                let start = self
                    .offsets
                    .partition_point(|&y| y + pad < viewport.top())
                    .saturating_sub(1);
                let end = self
                    .offsets
                    .partition_point(|&y| y + pad <= viewport.bottom())
                    .min(lyrics.lines.len());
                for i in start..end {
                    let rect = row_rect(i);
                    let sung = active == Some(i);
                    let mut color = if sung {
                        Color32::WHITE
                    } else if !lyrics.synced {
                        theme::SECONDARY
                    } else if active.is_some_and(|k| i < k) {
                        Color32::from_gray(96)
                    } else {
                        theme::FAINT
                    };
                    if lyrics.synced {
                        let res = ui
                            .interact(rect, ui.scope_id().with(("line", i)), Sense::click())
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                        res.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::Role::Button,
                                true,
                                &lyrics.lines[i].text,
                            )
                        });
                        if !sung && res.hovered() {
                            color = theme::MUTED;
                        }
                        if res.clicked() {
                            seek = lyrics.lines[i].t;
                            self.target = None;
                            self.manual_until = None;
                        }
                    }
                    let row = row_layout(ui.painter(), i, size);
                    let pos = rect.min + vec2(0., 7.);
                    if sung {
                        for (radius, alpha) in [(3., 0.025), (1.5, 0.04)] {
                            for (x, y) in [(1., 0.), (-1., 0.), (0., 1.), (0., -1.)] {
                                faded(
                                    ui.painter(),
                                    view,
                                    pos + vec2(x, y) * radius,
                                    &row.words,
                                    tint.ink(alpha),
                                );
                            }
                        }
                    }
                    faded(ui.painter(), view, pos, &row.words, color);
                    if let Some(g) = row.translation {
                        let ink = if sung { theme::SECONDARY } else { theme::FAINT };
                        faded(
                            ui.painter(),
                            view,
                            pos2(pos.x, pos.y + row.words.size().y + 4.),
                            &g,
                            ink,
                        );
                    }
                }
            });
        seek
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Context, Event, Modifiers, PointerButton, RawInput};
    use string_lyrics::{Ollama, Source, parse_lrc};

    #[test]
    fn clicking_a_wrapped_lyric_seeks_to_its_timestamp() {
        let ctx = Context::default();
        theme::install(&ctx);
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(360., 500.));
        let parsed = parse_lrc(
            "[00:01]The first lyric wraps across several words on this narrow screen\n[00:08]Second line",
        );
        let lyrics = LyricsResult {
            source: Source::File,
            text: String::new(),
            synced: true,
            lines: parsed.lines,
            instrumental: false,
            online: false,
            busy: false,
            retry: false,
        };
        let tr = Translator::new(Ollama::with_base_url(".", "http://127.0.0.1:0").unwrap());
        let mut follow = Follow {
            manual_until: Some(Instant::now() + Duration::from_secs(30)),
            ..Default::default()
        };
        let mut frame = |events| {
            let mut seek = None;
            let mut at = Pos2::ZERO;
            let out = ctx.run_ui(
                RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                },
                |ui| {
                    seek = follow.show(ui, "sample", &lyrics, &tr, 1., Tint::Mono);
                    at = pos2(40., screen.height() * 0.32 + follow.offsets[1] + 16.);
                },
            );
            out.drop_without_applying_deltas();
            (at, seek)
        };
        let (pos, _) = frame(vec![]);
        frame(vec![Event::PointerMoved(pos)]);
        let button = |pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        assert_eq!(frame(vec![button(true)]).1, None);
        assert_eq!(frame(vec![button(false)]).1, Some(8.));
    }
}
