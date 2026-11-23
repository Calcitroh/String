use super::{Bucket, Snapshot, duration};
use crate::{theme::*, ui::area};
use egui::{Align2, Color32, CornerRadius, Rect, Sense, Ui, pos2, vec2};
use string_core::stats::PeriodKind;

fn scale(max: f64) -> (f64, f64, bool) {
    let mins = max < 5400.;
    let steps: &[f64] = if mins {
        &[60., 120., 300., 600., 900., 1800.]
    } else {
        &[
            3600., 7200., 10800., 21600., 43200., 86400., 172800., 432000., 864000.,
        ]
    };
    let step = steps
        .iter()
        .copied()
        .find(|s| max / s <= 4.)
        .unwrap_or_else(|| (max / 4. / 86400.).ceil() * 86400.);
    (step, step.max((max / step).ceil() * step), mins)
}
pub(super) fn bars(ui: &mut Ui, s: &Snapshot, tint: Tint) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 230.), Sense::hover());
    let plot = Rect::from_min_max(r.min + vec2(52., 24.), r.max - vec2(10., 28.));
    let max = s.buckets.iter().map(|b| b.seconds).fold(0., f64::max);
    let (step, top, mins) = scale(max);
    let y = |secs| plot.bottom() - (secs / top) as f32 * plot.height();
    let p = ui.painter();
    for tick in 0..=(top / step).round() as usize {
        let val = tick as f64 * step;
        let y = y(val);
        let label = if tick == 0 {
            "0".into()
        } else if mins {
            format!("{}m", val / 60.)
        } else {
            format!("{}h", val / 3600.)
        };
        p.hline(
            plot.x_range(),
            y,
            (1., if tick == 0 { HAIR3 } else { HAIR }),
        );
        p.text(
            pos2(plot.left() - 10., y),
            Align2::RIGHT_CENTER,
            label,
            font("Mono", 10.5),
            FAINT,
        );
    }
    let slot = plot.width() / s.buckets.len().max(1) as f32;
    let width = (slot * 0.62).clamp(3., 24.);
    let mut peak = false;
    for (
        i,
        Bucket {
            label,
            long,
            seconds,
        },
    ) in s.buckets.iter().enumerate()
    {
        let x = plot.left() + (i as f32 + 0.5) * slot;
        let hit = area(x - slot / 2., plot.top(), slot, plot.height());
        let hover = ui.interact(hit, ui.scope_id().with(("time bucket", i)), Sense::hover());
        if hover.hovered() {
            p.rect_filled(hit, 0, Color32::from_white_alpha(9));
        }
        hover.on_hover_ui(|ui| {
            ui.label(mono(long, 11.).color(MUTED));
            ui.strong(duration(*seconds, false));
        });
        let current = s.current == Some(i);
        let h = plot.bottom() - y(*seconds);
        if h >= 0.5 {
            let b = area(x - width / 2., y(*seconds), width, h);
            let rad = 4u8.min((width * 0.5).min(h) as u8);
            let corners = CornerRadius {
                nw: rad,
                ne: rad,
                sw: 0,
                se: 0,
            };
            if current {
                for px in (1..=4).rev() {
                    p.rect_filled(b.expand(px as f32), corners, tint.ink(0.025));
                }
            }
            p.rect_filled(b, corners, tint.ink(if current { 1. } else { 0.5 }));
            if *seconds == max && !peak {
                peak = true;
                p.text(
                    pos2(x, b.top() - 5.),
                    Align2::CENTER_BOTTOM,
                    duration(max, true),
                    font("Mono", 11.),
                    SECONDARY,
                );
            }
        }
        p.text(
            pos2(x, r.bottom() - 6.),
            Align2::CENTER_BOTTOM,
            label,
            font("Mono", 10.5),
            if current { SECONDARY } else { FAINT },
        );
    }
}
pub(super) fn table(ui: &mut Ui, s: &Snapshot) {
    let unit = match s.kind {
        PeriodKind::Day => "HOUR",
        PeriodKind::Year | PeriodKind::All => "MONTH",
        _ => "DAY",
    };
    let mut row = |a: &str, b: &str, header: bool, current: bool| {
        let (r, _) = ui.allocate_exact_size(
            vec2(ui.available_width(), if header { 30. } else { 36. }),
            Sense::hover(),
        );
        if !ui.is_rect_visible(r) {
            return;
        }
        let p = ui.painter();
        if !header && ui.rect_contains_pointer(r) {
            p.rect_filled(r, 0, HAIR);
        }
        let ink = if header {
            FAINT
        } else if current {
            TEXT
        } else {
            SECONDARY
        };
        let f = if header {
            font("HUD", 11.)
        } else {
            font("Mono", 12.5)
        };
        p.text(
            pos2(r.left() + 10., r.center().y),
            Align2::LEFT_CENTER,
            a,
            f.clone(),
            ink,
        );
        p.text(
            pos2(r.left() + r.width() * 0.65, r.center().y),
            Align2::LEFT_CENTER,
            b,
            f,
            ink,
        );
        p.hline(
            r.x_range(),
            r.bottom(),
            (1., if header { HAIR2 } else { HAIR }),
        );
    };
    row(unit, "LISTENED", true, false);
    for (i, b) in s.buckets.iter().enumerate() {
        row(
            &b.long,
            &duration(b.seconds, false),
            false,
            s.current == Some(i),
        );
    }
}
