//! A thank-you to the icon's maker: the first time String opens a library with this icon, a small card above the player
//! bar says who made it, until it's closed (then never again for that library: `set.iconCreditSeen`).
use crate::{theme::*, ui, view::View};
use egui::{Align2, Color32, Context, Id, Rect, RichText, Sense, TextureHandle, TextureOptions, Vec2, vec2};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};

const SEEN: &str = "iconCreditSeen";
const TEXT_CREDIT: &str = "Credits to Discord — awepollyon for the icon!";
static LOGO: Mutex<Option<TextureHandle>> = Mutex::new(None);
// The self-test's way to close it through the same path as the Close button.
static CLOSE_NOW: AtomicBool = AtomicBool::new(false);
// The self-test never shows it on its own (its pictures would all have the card); a case asks for it.
static IN_TEST: AtomicBool = AtomicBool::new(false);
static DRAWN: AtomicBool = AtomicBool::new(false);
fn logo(ctx: &Context) -> TextureHandle {
    LOGO.lock()
        .unwrap()
        .get_or_insert_with(|| {
            let im = image::load_from_memory(include_bytes!("../../assets/String.png"))
                .map(|i| i.into_rgba8())
                .unwrap_or_else(|_| image::RgbaImage::new(1, 1));
            let size = [im.width() as usize, im.height() as usize];
            ctx.load_texture(
                "icon credit",
                egui::ColorImage::from_rgba_unmultiplied(size, im.as_raw()),
                TextureOptions::LINEAR,
            )
        })
        .clone()
}
pub fn show(ctx: &Context, v: &mut View<'_>) {
    // `force_mute` is the self-test's mark
    let test = v.force_mute;
    let wanted = if test {
        IN_TEST.load(Ordering::Relaxed)
    } else {
        v.s.setting(SEEN) != true
    };
    DRAWN.store(false, Ordering::Relaxed);
    if !wanted {
        CLOSE_NOW.store(false, Ordering::Relaxed);
        return;
    }
    // In a narrow window the text wraps, and without room the logo goes, so the card never runs off the left edge.
    let room = ctx.content_rect().width() - 36. - 24. - 24. - 28.;
    let with_logo = room >= 40. + 12. + 160.;
    let text_width = (room - if with_logo { 52. } else { 0. }).clamp(40., 360.);
    let mut closed = CLOSE_NOW.swap(false, Ordering::Relaxed);
    egui::Area::new(Id::unique("icon credit"))
        .anchor(Align2::RIGHT_BOTTOM, [-18., -BAR - 14.])
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(PANEL)
                .stroke((1., HAIR2))
                .inner_margin(egui::Margin::symmetric(12, 10))
                .shadow(egui::epaint::Shadow {
                    offset: [0, 10],
                    blur: 30,
                    spread: 0,
                    color: Color32::from_black_alpha(170),
                })
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 12.;
                        if with_logo {
                            ui.add(egui::Image::new(&logo(ctx)).fit_to_exact_size(vec2(40., 40.)));
                        }
                        ui.scope(|ui| {
                            ui.set_max_width(text_width);
                            ui.add(
                                egui::Label::new(RichText::new(TEXT_CREDIT).font(font("UI", 13.5)).color(TEXT))
                                    .wrap(),
                            );
                        });
                        // Close is clicked, never focused: Space and Enter keep doing exactly what they do without the
                        // card (play and pause, play the selected row), held down or with Ctrl, every frame.
                        let (r, _) = ui.allocate_exact_size(vec2(28., 28.), Sense::hover());
                        let close = ui.interact(r, Id::unique("icon credit close"), Sense::CLICK);
                        if close.hovered() {
                            ui.painter().rect(r, 0, white(0.05), (1., HAIR), egui::StrokeKind::Inside);
                        }
                        ui::glyph::draw(
                            ui.painter(),
                            Rect::from_center_size(r.center(), Vec2::splat(16.)),
                            "x",
                            if close.hovered() { TEXT } else { MUTED },
                        );
                        closed |= close.on_hover_text("Close").clicked();
                    });
                });
        });
    DRAWN.store(true, Ordering::Relaxed);
    if closed {
        if test {
            IN_TEST.store(false, Ordering::Relaxed);
        } else {
            v.s.set_setting(SEEN, true.into());
        }
    }
}
pub mod smoke {
    use super::*;
    use crate::smoke::Case;
    use std::sync::atomic::AtomicU64;
    // what was playing when the card was closed, as (playing, a fingerprint of the current song)
    static BEFORE: AtomicU64 = AtomicU64::new(0);
    fn now(v: &View<'_>) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        v.s.state.queue.current().hash(&mut h);
        (h.finish() << 1) | u64::from(v.st.playing)
    }
    pub const SHOWN: Case = Case {
        name: "50-icon-credit",
        prepare: |_| IN_TEST.store(true, Ordering::Relaxed),
        ready: |_| DRAWN.load(Ordering::Relaxed),
    };
    // closed through its own close path (what Close and its keys do), and nothing else changes
    pub const CLOSED: Case = Case {
        name: "51-icon-credit-closed",
        prepare: |v| {
            BEFORE.store(now(v), Ordering::Relaxed);
            CLOSE_NOW.store(true, Ordering::Relaxed);
        },
        ready: |v| {
            !DRAWN.load(Ordering::Relaxed)
                && !IN_TEST.load(Ordering::Relaxed)
                && now(v) == BEFORE.load(Ordering::Relaxed)
        },
    };
}
