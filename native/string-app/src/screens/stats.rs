mod chart;
pub(crate) mod smoke;

use crate::{
    model::{CoverSource, norm},
    route::Route,
    theme::*,
    ui::{self, area, covers::Size},
    view::View,
};
use chrono::{Datelike, Local, Weekday};
use egui::{Align2, CornerRadius, StrokeKind, Ui, pos2, vec2};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use string_core::stats::{Listening, PeriodKind, PeriodKind::*, Unit};
use windows::{
    Win32::Globalization::{
        GetUserPreferredUILanguages, MUI_LANGUAGE_NAME, U_ZERO_ERROR, UCAL_DEFAULT,
        UCAL_FIRST_DAY_OF_WEEK, ucal_close, ucal_getAttribute, ucal_open,
    },
    core::{PCSTR, PWSTR},
};

const KINDS: [(PeriodKind, &str, &str, &str); 5] = [
    (Day, "day", "Today", "Yesterday"),
    (Week, "week", "This week", "Last week"),
    (Month, "month", "This month", "Last month"),
    (Year, "year", "This year", "Last year"),
    (All, "all", "All time", ""),
];
const REFRESH: Duration = Duration::from_secs(20);

fn duration(seconds: f64, short: bool) -> String {
    let seconds = seconds.round() as u64;
    if seconds < 60 {
        return match (seconds > 0, short) {
            (true, true) => "<1m",
            (true, false) => "< 1 min",
            (false, true) => "0m",
            _ => "0 min",
        }
        .into();
    }
    let (h, m) = (seconds / 3600, seconds % 3600 / 60);
    let (hours, mins) = if short { ("h", "m") } else { (" hr", " min") };
    match (h, m) {
        (0, _) => format!("{m}{mins}"),
        (_, 0) => format!("{h}{hours}"),
        _ => format!("{h}{hours} {m}{mins}"),
    }
}

fn week_start() -> Weekday {
    let (mut count, mut size) = (0, 0);
    let language = unsafe {
        GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, None, &mut size)
            .ok()
            .and_then(|()| {
                let mut names = vec![0; size as usize];
                GetUserPreferredUILanguages(
                    MUI_LANGUAGE_NAME,
                    &mut count,
                    Some(PWSTR(names.as_mut_ptr())),
                    &mut size,
                )
                .ok()?;
                let first = names.split(|&c| c == 0).next().filter(|s| !s.is_empty())?;
                String::from_utf16(first).ok()
            })
    };
    week_start_for_language(language.as_deref().unwrap_or("en-US"))
}

// Intl.Locale's week data comes from the language's territory, without regional-format overrides.
fn week_start_for_language(language: &str) -> Weekday {
    let Ok(locale) = std::ffi::CString::new(language) else {
        return Weekday::Mon;
    };
    let mut status = U_ZERO_ERROR;
    unsafe {
        let cal = ucal_open(
            std::ptr::null(),
            0,
            PCSTR(locale.as_ptr().cast()),
            UCAL_DEFAULT,
            &mut status,
        );
        if cal.is_null() {
            return Weekday::Mon;
        }
        let first = ucal_getAttribute(cal.cast(), UCAL_FIRST_DAY_OF_WEEK);
        ucal_close(cal);
        match first {
            1 => Weekday::Sun,
            3 => Weekday::Tue,
            4 => Weekday::Wed,
            5 => Weekday::Thu,
            6 => Weekday::Fri,
            7 => Weekday::Sat,
            _ => Weekday::Mon,
        }
    }
}
struct Bucket {
    label: String,
    long: String,
    seconds: f64,
}
fn bucket(kind: PeriodKind, unit: &Unit, i: usize, seconds: f64) -> Bucket {
    let date = unit.from;
    let fmt = |s| date.format(s).to_string();
    let (label, long) = match kind {
        Day => {
            let h = unit.hour.unwrap_or_default();
            let hour = (h + 11) % 12 + 1;
            let half = if h < 12 { "AM" } else { "PM" };
            let label = if h.is_multiple_of(6) {
                format!("{hour}{}", if h < 12 { 'a' } else { 'p' })
            } else {
                String::new()
            };
            (label, format!("{hour} {half}"))
        }
        Week => (fmt("%a"), fmt("%A, %b %-d")),
        Month => (
            if i == 0 || date.day().is_multiple_of(5) {
                date.day().to_string()
            } else {
                String::new()
            },
            fmt("%a, %b %-d"),
        ),
        Year | All => (
            if kind == All && (i == 0 || date.month() == 1) {
                fmt("%b '%y")
            } else {
                fmt("%b").chars().take(1).collect()
            },
            fmt("%B %Y"),
        ),
    };
    Bucket {
        label,
        long,
        seconds,
    }
}
struct Rank {
    title: String,
    artist: Option<String>,
    seconds: f64,
    cover: Arc<CoverSource>,
    key: Option<String>,
}
struct Snapshot {
    drawn: Instant,
    kind: PeriodKind,
    subtitle: String,
    has_history: bool,
    tiles: Vec<(String, String)>,
    chart_title: &'static str,
    note: String,
    buckets: Vec<Bucket>,
    current: Option<usize>,
    artists: Vec<Rank>,
    tracks: Vec<Rank>,
    phone: PhoneSummary,
}
fn fallback_art(seed: String) -> Arc<CoverSource> {
    Arc::new(CoverSource {
        seed,
        ..Default::default()
    })
}
fn total(model: &Listening, from: chrono::NaiveDate, to: chrono::NaiveDate) -> f64 {
    model.days().range(from..=to).map(|(_, d)| d.s).sum()
}
impl Snapshot {
    fn read(v: &View<'_>, kind: PeriodKind) -> Self {
        let model = &v.s.listening;
        let now = Local::now().naive_local();
        let week = week_start();
        let period = |k| model.period(k, now, week).expect("current calendar period");
        let tiles = KINDS[..4]
            .iter()
            .map(|&(k, _, _, prev)| {
                let p = period(k);
                let (from, to) = p.previous.unwrap();
                (
                    duration(total(model, p.from, p.to), false),
                    format!("{prev} {}", duration(total(model, from, to), false)),
                )
            })
            .collect();
        let first = model.first_day();
        let subtitle = first.map_or_else(
            || "Time spent listening, counted as you play".into(),
            |day| {
                let mut s = format!("Since {}", day.format("%b %-d, %Y"));
                let streak = model.streak(now.date());
                if streak > 1 {
                    s.push_str(&format!(" · {streak}-day streak"));
                }
                s
            },
        );
        let p = period(kind);
        let range = model.range(p.from, p.to);
        let mut note = format!("{} in all", duration(range.total, false));
        if range.phone > 0. {
            note.push_str(&format!(
                " · {} on your phone",
                duration(range.phone, false)
            ));
        }
        let buckets = p
            .units
            .iter()
            .zip(model.series(&p))
            .enumerate()
            .map(|(i, (u, s))| bucket(kind, u, i, s))
            .collect();
        let models = &v.s.models;
        let artists = range
            .artists
            .into_iter()
            .take(10)
            .map(|(name, seconds)| {
                let key = norm(&name);
                let group = models.catalog.artists.iter().find(|g| g.key == key);
                let cover = group
                    .and_then(|g| g.ids.first())
                    .and_then(|id| models.by_id.get(id))
                    .map(|s| s.cover.clone())
                    .unwrap_or_else(|| fallback_art(format!("artist:{key}")));
                let key = group.filter(|_| !name.is_empty()).map(|_| key);
                Rank {
                    title: if name.is_empty() {
                        "Unknown artist".into()
                    } else {
                        name
                    },
                    artist: None,
                    seconds,
                    cover,
                    key,
                }
            })
            .collect();
        let tracks = range
            .tracks
            .into_iter()
            .take(10)
            .map(|(t, seconds)| {
                let song = models.by_id.get(&t.0);
                let (name, artist) = song.map_or((t.1.as_str(), t.2.as_str()), |s| {
                    (s.title.as_str(), s.artist.as_str())
                });
                Rank {
                    title: name.into(),
                    artist: Some(
                        if artist.is_empty() {
                            "Unknown artist"
                        } else {
                            artist
                        }
                        .into(),
                    ),
                    seconds,
                    key: song.map(|s| s.id.clone()),
                    cover: song
                        .map(|s| s.cover.clone())
                        .unwrap_or_else(|| fallback_art(t.0.clone())),
                }
            })
            .collect();
        let chart_title = match kind {
            Day => "Today, by hour",
            Week => "This week, by day",
            Month => "This month, by day",
            Year => "This year, by month",
            All => "All time, by month",
        };
        Self {
            drawn: Instant::now(),
            kind,
            subtitle,
            has_history: first.is_some(),
            tiles,
            chart_title,
            note,
            buckets,
            current: p.current,
            artists,
            tracks,
            phone: PhoneSummary::read(v),
        }
    }
}
#[derive(Default)]
pub struct StatsView {
    snapshot: Option<Arc<Snapshot>>,
    phone_users: Vec<(String, String)>,
    phone_sync_requested: bool,
    phone_connected: bool,
    reveal_phone: bool,
    phone_visible: bool,
}
impl StatsView {
    /// Exchange account choices and manual sync requests with the phone scheduler.
    pub fn sync_phone(&mut self, accounts: &[(String, String)]) -> bool {
        self.phone_connected = true;
        if self.phone_users != accounts {
            accounts.clone_into(&mut self.phone_users);
        }
        std::mem::take(&mut self.phone_sync_requested)
    }
}
pub fn show(ui: &mut Ui, v: &mut View<'_>) {
    let &(kind, key, name, _) = KINDS
        .iter()
        .find(|(_, k, _, _)| v.s.field("statsKind") == *k)
        .unwrap_or(&KINDS[1]);
    let hover = ui.rect_contains_pointer(ui.max_rect()) || egui::Popup::is_any_open(ui.ctx());
    if v.views
        .stats
        .snapshot
        .as_ref()
        .is_none_or(|s| s.kind != kind || (!hover && s.drawn.elapsed() >= REFRESH))
    {
        v.views.stats.snapshot = Some(Arc::new(Snapshot::read(v, kind)));
    }
    let s = v.views.stats.snapshot.as_ref().unwrap().clone();
    if !hover {
        ui.ctx()
            .request_repaint_after(REFRESH.saturating_sub(s.drawn.elapsed()));
    }
    egui::ScrollArea::vertical()
        .id_salt("listening stats")
        .auto_shrink([false,false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.;
            super::heading(ui, "Stats", &s.subtitle.to_uppercase());
            periods(ui, v, &s);
            if !s.has_history {
                ui.label(egui::RichText::new("Play something: the time you spend listening adds up here, with your top artists and songs").color(MUTED));
                ui.add_space(18.);
                phone(ui, v, &s.phone);
                return;
            }
            let table = v.s.field("statsTable") == true;
            panel().show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                let button = if table { "Chart" } else { "Table" };
                if panel_header(ui, s.chart_title, &s.note, Some(button)) {
                    v.s.set_field("statsTable", (!table).into());
                }
                if table { chart::table(ui, &s); } else { chart::bars(ui, &s, v.tint); }
            });
            ui.add_space(18.);
            let side_by_side = ui.available_width() >= 698.;
            let min_rows = if side_by_side { s.artists.len().max(s.tracks.len()) } else { 0 };
            let mut ranks = |ui: &mut Ui, artists: bool| {
                let (title, rows) = if artists { ("Top artists", &s.artists) } else { ("Top songs", &s.tracks) };
                panel().show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    panel_header(ui, title, &name.to_lowercase(), None);
                    if rows.is_empty() { ui.label("Nothing yet"); }
                    for (rank, item) in rows.iter().enumerate() {
                        if rank_row(ui, v, rank, item, rows[0].seconds) && let Some(id) = &item.key {
                            if artists { v.go(Route::Artist(id.clone())); }
                            else { play_ranked_song(v, rows, id, key, name); }
                        }
                    }
                    ui.add_space(54. * min_rows.saturating_sub(rows.len()) as f32);
                });
            };
            if side_by_side {
                ui.spacing_mut().item_spacing.x = 18.;
                ui.columns(2, |cols| {
                    ranks(&mut cols[0], true);
                    ranks(&mut cols[1], false);
                });
            } else {
                ranks(ui, true);
                ui.add_space(18.);
                ranks(ui, false);
            }
            ui.add_space(18.);
            phone(ui, v, &s.phone);
        });
}

fn play_ranked_song(v: &mut View<'_>, rows: &[Rank], id: &str, period: &str, name: &str) {
    if v.s.state.queue.current() == Some(id) {
        v.s.toggle(v.player);
    } else {
        let ids: Vec<_> = rows.iter().filter_map(|r| r.key.clone()).collect();
        if let Some(idx) = ids.iter().position(|k| k == id) {
            let context = serde_json::json!({
                "kind": "recent", "key": format!("top-{period}"), "name": format!("Top songs, {}",name.to_lowercase())
            });
            v.s.play_ids(v.player, &ids, idx as isize, context);
        }
    }
}

fn periods(ui: &mut Ui, v: &mut View<'_>, snapshot: &Snapshot) {
    let cols = if ui.ctx().content_rect().width() <= 900. {
        2
    } else {
        4
    };
    let width = (ui.available_width() - 12. * (cols - 1) as f32) / cols as f32;
    for row in (0..4).step_by(cols) {
        let (_, rect) = ui.allocate_space(vec2(ui.available_width(), 102.));
        for (col, &(kind, key, title, _)) in KINDS[row..row + cols].iter().enumerate() {
            let r = area(
                rect.left() + col as f32 * (width + 12.),
                rect.top(),
                width,
                102.,
            );
            let resp = ui::hit(ui, r, ("tile", key), title);
            let p = ui.painter();
            let selected = kind == snapshot.kind;
            let border = if selected {
                v.tint.ink(0.60)
            } else if resp.hovered() {
                HAIR3
            } else {
                HAIR2
            };
            p.rect(
                r,
                0,
                if selected { v.tint.ink(0.06) } else { PANEL },
                (1., border),
                StrokeKind::Inside,
            );
            let inner = r.shrink2(vec2(16., 0.));
            let band = |lo, hi| inner.with_min_y(r.top() + lo).with_max_y(r.top() + hi);
            ui::label(p, band(0., 42.), &title.to_uppercase(), "HUD", 11., MUTED);
            ui::label(
                p,
                band(31., 78.),
                &snapshot.tiles[row + col].0,
                "Strong",
                24.,
                TEXT,
            );
            ui::label(
                p,
                band(69., 97.),
                &snapshot.tiles[row + col].1,
                "Mono",
                11.,
                FAINT,
            );
            if resp.clicked() {
                v.s.set_field("statsKind", key.into());
            }
        }
        if row + cols < 4 {
            ui.add_space(12.);
        }
    }
    ui.add_space(22.);
    egui::Frame::new()
        .inner_margin(2)
        .stroke((1., HAIR2))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.;
                for (k, key, title, _) in KINDS {
                    let selected = k == snapshot.kind;
                    let button =
                        egui::Button::new(hud(&title.to_uppercase(), 10.5).color(if selected {
                            TEXT
                        } else {
                            MUTED
                        }))
                        .min_size(vec2(64., 24.))
                        .corner_radius(0)
                        .fill(if selected { v.tint.ink(0.14) } else { BG })
                        .stroke(if selected {
                            egui::Stroke::new(1., v.tint.ink(0.35))
                        } else {
                            egui::Stroke::NONE
                        });
                    if ui.add(button).clicked() {
                        v.s.set_field("statsKind", key.into());
                    }
                }
            });
        });
    ui.add_space(14.);
}
fn panel() -> egui::Frame {
    egui::Frame::new()
        .fill(PANEL)
        .stroke((1., HAIR2))
        .inner_margin(egui::Margin::symmetric(21, 19))
}
fn panel_header(ui: &mut Ui, title: &str, note: &str, button: Option<&str>) -> bool {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.), egui::Sense::hover());
    let t = ui.painter().text(
        r.left_center(),
        Align2::LEFT_CENTER,
        title.to_uppercase(),
        font("HUD", 12.),
        TEXT,
    );
    let end = r.right() - if button.is_some() { 67. } else { 0. };
    let n = ui.painter().text(
        pos2(end, r.center().y),
        Align2::RIGHT_CENTER,
        note,
        font("Mono", 10.5),
        FAINT,
    );
    if n.left() > t.right() + 25. {
        ui.painter()
            .hline(t.right() + 12. ..=n.left() - 10., r.center().y, (1., HAIR));
    }
    ui.add_space(13.);
    button.is_some_and(|b| {
        let rect = area(r.right() - 57., r.top(), 57., 26.);
        let re = ui::hit(ui, rect, b, b);
        if re.hovered() {
            ui.painter().rect_filled(rect, 0, RAISED);
        }
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            b.to_uppercase(),
            font("HUD", 11.),
            SECONDARY,
        );
        re.clicked()
    })
}
fn rank_row(ui: &mut Ui, v: &mut View<'_>, i: usize, item: &Rank, max: f64) -> bool {
    let (r, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), 54.), egui::Sense::click());
    if !ui.is_rect_visible(r) {
        return false;
    }
    let p = ui.painter();
    if response.hovered() && item.key.is_some() {
        p.rect_filled(r, 0, RAISED);
    }
    p.text(
        pos2(r.left() + 25., r.center().y),
        Align2::RIGHT_CENTER,
        (i + 1).to_string(),
        font("Mono", 12.),
        FAINT,
    );
    let art = area(r.left() + 38., r.top() + 7., 40., 40.);
    let round = if item.artist.is_none() {
        CornerRadius::same(20)
    } else {
        CornerRadius::ZERO
    };
    p.rect_filled(art, round, RAISED);
    if let Some(cover) = v
        .covers
        .get(&item.cover, Size::Small, v.mono.then_some(v.tint))
    {
        egui::Image::new(&cover.cover)
            .corner_radius(round)
            .paint_at(ui, art);
    }
    let text = item.key.is_some() || item.artist.is_none();
    let end = p.text(
        pos2(r.right() - 7., r.center().y),
        Align2::RIGHT_CENTER,
        duration(item.seconds, true),
        font("Mono", 12.),
        if text { SECONDARY } else { FAINT },
    );
    let x = art.right() + 12.;
    let w = (end.left() - x - 12.).max(1.);
    let y = if item.artist.is_some() {
        r.top() + 4.
    } else {
        r.top() + 11.
    };
    ui::label(
        p,
        area(x, y, w, 21.),
        &item.title,
        "Strong",
        13.5,
        if text { TEXT } else { MUTED },
    );
    if let Some(artist) = &item.artist {
        ui::label(p, area(x, y + 20., w, 15.), artist, "Mono", 10.5, FAINT);
    }
    let bar = area(
        x,
        if item.artist.is_some() {
            r.bottom() - 9.
        } else {
            y + 25.
        },
        w,
        4.,
    );
    p.rect_filled(bar, 2, RAISED);
    p.rect_filled(
        bar.with_max_x(x + w * (item.seconds / max.max(0.1)) as f32),
        2,
        v.tint.ink(if text { 0.7 } else { 0.35 }),
    );
    if item.key.is_some() {
        response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
    } else {
        if item.artist.is_some() {
            response.on_hover_text("Not in your library any more");
        }
        false
    }
}
fn phone_note(phone: &serde_json::Value) -> String {
    let plays = phone["plays"].as_u64().unwrap_or(0);
    let noun = if plays == 1 { "play" } else { "plays" };
    let mut note = format!(
        "Phone listening comes from Navidrome on this PC: {plays} {noun} counted, as whole songs"
    );
    if let Some(at) = phone["at"].as_i64().filter(|&at| at > 0) {
        let elapsed = Local::now().timestamp_millis().saturating_sub(at).max(0);
        let minutes = (elapsed + 30_000) / 60_000;
        let ago = match minutes {
            0 => "just now".into(),
            1..60 => format!("{minutes} min ago"),
            _ => format!("{} h ago", (minutes + 30) / 60),
        };
        note.push_str(&format!(" · checked {ago}"));
    }
    if phone["error"].as_str().is_some_and(|s| !s.is_empty()) {
        note.push_str(" · couldn't read it last time");
    }
    note.push_str(" ·");
    note
}
struct PhoneSummary {
    found: bool,
    note: String,
    users: Vec<(String, String)>,
}
impl PhoneSummary {
    fn read(v: &View<'_>) -> Self {
        let phone = v.s.field("phone");
        let note = phone_note(phone);
        Self {
            found: phone["found"] == true,
            note,
            users: v.views.stats.phone_users.clone(),
        }
    }
}
fn phone(ui: &mut Ui, v: &mut View<'_>, summary: &PhoneSummary) {
    let enabled = v.s.setting("phoneStats") != false;
    if enabled && !summary.found {
        return;
    }
    let chosen = v.s.setting("phoneUser").as_str().unwrap_or("").to_owned();
    let selected = summary.users.iter().find(|(id, _)| id == &chosen);
    let picker =
        enabled && (summary.users.len() > 1 || (!summary.users.is_empty() && selected.is_none()));
    let group = ui.vertical(|ui| {
        if picker {
            ui.horizontal(|ui| {
                ui.label(mono("Count phone listening for",11.).color(FAINT));
                let name = selected.map_or("Choose your Navidrome account", |(_,name)|name.as_str());
                egui::ComboBox::from_id_salt("phone listener").selected_text(name).show_ui(ui, |ui| {
                    for (id,name) in &summary.users {
                        if ui.selectable_label(*id==chosen,name).clicked() {
                            v.s.set_setting("phoneUser",id.clone().into());
                            v.views.stats.phone_sync_requested = true;
                        }
                    }
                });
            });
            ui.add_space(8.);
            if selected.is_none() {return;}
        }
        let (note, action) = if enabled { (summary.note.as_str(), "stop counting") } else { ("Phone listening isn't counted ·", "count it") };
        ui.horizontal_wrapped(|ui| {
            ui.label(mono(note,11.).color(FAINT)).on_hover_text("Navidrome reports a song after it has played about halfway, so a phone play counts as the whole song.");
            if ui.link(mono(action,11.).color(MUTED)).clicked() {
                v.s.set_setting("phoneStats", (!enabled).into());
                v.views.stats.phone_sync_requested = !enabled;
            }
        });
    });
    v.views.stats.phone_visible = ui.clip_rect().contains_rect(group.response.rect);
    if v.views.stats.reveal_phone {
        group.response.scroll_to_me(Some(egui::Align::Max));
        v.views.stats.reveal_phone = false;
    }
}

#[cfg(test)]
mod locale_tests {
    use super::*;
    #[test]
    fn week_follows_the_display_language() {
        assert_eq!(week_start_for_language("en-GB"), Weekday::Mon);
        assert_eq!(week_start_for_language("en-US"), Weekday::Sun);
        assert_eq!(week_start_for_language("ar-EG"), Weekday::Sat);
    }

    #[test]
    fn phone_status_uses_middle_dots_before_the_check_error_and_link() {
        let phone = serde_json::json!({"plays":1,"at":Local::now().timestamp_millis(),"error":"unavailable"});
        assert!(phone_note(&phone).ends_with(" · checked just now · couldn't read it last time ·"));
    }
}
