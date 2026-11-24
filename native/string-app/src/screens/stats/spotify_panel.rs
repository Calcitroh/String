//! The "Spotify history" panel on Stats: drop or choose an export, see what came in, take it out again.
use super::{STATS_PANEL, panel_header};
use crate::{
    integration::{
        files::LibraryPicker,
        spotify::{self, JobKind},
    },
    playback::SpotifySummary,
    theme::*,
    ui,
    view::View,
};
use chrono::Datelike;
use egui::{Align, Color32, Sense, Shape, Ui, pos2, text::LayoutJob, vec2};

const DROP: &str =
    "Drop your Spotify export here (\"Extended streaming history\"), or click to choose it.";

pub(super) fn show(ui: &mut Ui, v: &mut View<'_>) {
    // Files dragged over the window: the drop goes to this panel's import if it holds a history.
    let lit = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());
    let summary = v.s.spotify_summary();
    egui::Frame::new()
        .fill(STATS_PANEL)
        .stroke((1., if lit { v.tint.ink(0.42) } else { HAIR2 }))
        .inner_margin(egui::Margin::symmetric(21, 19))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            let note = summary.map_or(String::new(), |s| {
                format!("{} plays", spotify::grouped(s.plays))
            });
            panel_header(ui, "Spotify history", &note, None);
            body(ui, v, summary, lit);
        });
    let ctx = ui.ctx().clone();
    if v.views.stats.confirm_remove
        && let Some(yes) = ui::confirm(
            &ctx,
            "Take your Spotify history out of Stats?",
            "Your own listening stays.",
            "Remove",
        )
    {
        v.views.stats.confirm_remove = false;
        if yes && let Err(why) = v.s.remove_spotify() {
            v.views.toast.set(why);
        }
    }
}

fn body(ui: &mut Ui, v: &mut View<'_>, summary: Option<SpotifySummary>, lit: bool) {
    if let Some(job) = v.s.spotify_job() {
        let progress = job.progress();
        let text = match (job.kind(), progress) {
            (JobKind::Import, Some((done, total))) => format!(
                "Reading your Spotify history: file {} of {total}",
                (done + 1).min(total)
            ),
            (JobKind::Import, None) => "Reading your Spotify history".into(),
            (JobKind::Remove, _) => "Taking your Spotify history out of Stats".into(),
        };
        ui.label(mono(&text, 11.5).color(SECONDARY));
        ui.add_space(10.);
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 2.), Sense::hover());
        ui.painter().rect_filled(r, 1, PANEL2);
        if let Some((done, total)) = progress.filter(|&(_, total)| total > 0) {
            let part = r.width() * done as f32 / total as f32;
            ui.painter()
                .rect_filled(r.with_max_x(r.left() + part), 1, v.tint.ink(0.5));
        }
        return;
    }
    if v.s.spotify_loading() {
        ui.label(mono("Reading your Spotify history", 11.5).color(MUTED));
        return;
    }
    if v.s.spotify.is_some() {
        let line = summary.map_or_else(
            || "Your Spotify history holds no song plays".into(),
            |s| summary_line(&s),
        );
        if with_link(ui, &line, SECONDARY, " · ", "Remove") {
            v.views.stats.confirm_remove = true;
        }
        ui.add_space(6.);
        if with_link(
            ui,
            "A newer export adds only its new plays: drop it on this page, or",
            FAINT,
            " ",
            "choose it",
        ) {
            v.platform.picker = Some(LibraryPicker::SpotifyExport);
        }
        return;
    }
    if let Some(error) = v.s.spotify_error.clone() {
        // The file is there but can't be read: an import would be refused until it's set aside.
        if with_link(ui, &error, SECONDARY, " · ", "Remove") {
            v.views.stats.confirm_remove = true;
        }
        return;
    }
    if drop_area(ui, v.tint, lit).clicked() {
        v.platform.picker = Some(LibraryPicker::SpotifyExport);
    }
    ui.add_space(10.);
    if ui
        .link(mono("Or choose the unzipped folder", 11.).color(FAINT))
        .clicked()
    {
        v.platform.picker = Some(LibraryPicker::SpotifyFolder);
    }
    if let Some(outcome) = v.s.spotify_outcome.as_ref().filter(|o| o.failed) {
        ui.add_space(8.);
        ui.label(mono(&outcome.message, 11.).color(MUTED));
    }
}

/// "2020 – Nov 23, 2026 · 4,212 h · 61% of your Spotify listening is your songs"
pub(super) fn summary_line(s: &SpotifySummary) -> String {
    let last = s.last.format("%b %-d, %Y").to_string();
    let span = if s.first == s.last {
        last
    } else if s.first.year() == s.last.year() {
        format!("{} – {last}", s.first.format("%b %-d"))
    } else {
        format!("{} – {last}", s.first.year())
    };
    format!(
        "{span} · {} h · {}% of your Spotify listening is your songs",
        spotify::hours(s.seconds),
        spotify::percent(s.matched_share)
    )
}

/// Text and a link after it, wrapping as one line of words; true when the link was clicked.
fn with_link(ui: &mut Ui, text: &str, color: Color32, gap: &str, link: &str) -> bool {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.;
        ui.label(mono(text, 11.5).color(color));
        ui.label(mono(gap, 11.5).color(FAINT));
        ui.link(mono(link, 11.5).color(MUTED)).clicked()
    })
    .inner
}

/// The dashed place to drop or click. While files are dragged over the window it lights up, softly.
fn drop_area(ui: &mut Ui, tint: Tint, lit: bool) -> egui::Response {
    let w = ui.available_width();
    let mut job = LayoutJob::simple(
        DROP.into(),
        font("Mono", 11.5),
        Color32::PLACEHOLDER,
        (w - 48.).max(40.),
    );
    job.halign = Align::Center;
    let galley = ui.painter().layout_job(job);
    let (r, _) = ui.allocate_exact_size(vec2(w, galley.size().y + 46.), Sense::hover());
    let response = ui::hit(ui, r, "spotify drop", "Choose your Spotify export")
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let (dash, text) = if lit {
        (tint.ink(0.55), TEXT)
    } else if response.hovered() {
        (tint.ink(0.3), SECONDARY)
    } else {
        (HAIR3, MUTED)
    };
    let p = ui.painter();
    if lit {
        p.rect_filled(r, 0, tint.ink(0.04));
    }
    let c = [
        r.left_top(),
        r.right_top(),
        r.right_bottom(),
        r.left_bottom(),
        r.left_top(),
    ];
    p.extend(Shape::dashed_line(&c, (1., dash), 5., 4.));
    // Centred text is laid out around x = 0, as in `menus::empty_note`.
    let top = r.center().y - galley.size().y / 2.;
    p.galley(pos2(r.center().x, top), galley, text);
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn the_summary_says_when_how_long_and_how_much_is_yours() {
        let day = |y, m, d| NaiveDate::from_ymd_opt(y, m, d).unwrap();
        let mut s = SpotifySummary {
            first: day(2020, 3, 14),
            last: day(2026, 11, 23),
            seconds: 4212. * 3600. + 100.,
            matched_share: 0.6149,
            plays: 166_474,
        };
        assert_eq!(
            summary_line(&s),
            "2020 – Nov 23, 2026 · 4,212 h · 61% of your Spotify listening is your songs"
        );
        s.first = day(2026, 11, 16);
        s.seconds = 600.;
        assert_eq!(
            summary_line(&s),
            "Nov 16 – Nov 23, 2026 · < 1 h · 61% of your Spotify listening is your songs"
        );
    }
}
