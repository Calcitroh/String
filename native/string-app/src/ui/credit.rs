//! A thank-you to the icon's maker: the first time String opens a library with this icon, a small card above the player
//! bar says who made it, until it's closed (then never again for that library: `set.iconCreditSeen`).
use crate::{theme::*, ui, view::View};
use egui::{Align2, Color32, Context, Id, RichText, Sense, TextureHandle, TextureOptions, vec2};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};

const SEEN: &str = "iconCreditSeen";
const TEXT_CREDIT: &str = "Credits to Discord — awepollyon for the icon!";
static LOGO: Mutex<Option<TextureHandle>> = Mutex::new(None);
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
        return;
    }
    let mut closed = false;
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
                        ui.add(egui::Image::new(&logo(ctx)).fit_to_exact_size(vec2(40., 40.)));
                        ui.label(RichText::new(TEXT_CREDIT).font(font("UI", 13.5)).color(TEXT));
                        let (r, _) = ui.allocate_exact_size(vec2(28., 28.), Sense::hover());
                        closed = ui::icon_btn(ui, r, "x", "Close", false, v.tint).clicked();
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
    pub const SHOWN: Case = Case {
        name: "50-icon-credit",
        prepare: |_| IN_TEST.store(true, Ordering::Relaxed),
        ready: |_| DRAWN.load(Ordering::Relaxed),
    };
    pub const CLOSED: Case = Case {
        name: "51-icon-credit-closed",
        prepare: |_| IN_TEST.store(false, Ordering::Relaxed),
        ready: |_| !DRAWN.load(Ordering::Relaxed),
    };
}
