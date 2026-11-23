//! The Library page: music folders, playback, look, Discord, updates and the library's numbers.
use crate::{
    route::Route,
    services::{self, Discord, DiscordStatus, Event, Task, UpdateInfo, UpdateProgress},
    theme::*,
    ui::{
        self,
        panel::{self, Btn, Kind},
    },
    view::View,
};
use chrono::TimeZone;
use crossbeam_channel::{Receiver, unbounded};
use egui::{Align, Context, Frame, Label, Layout, Margin, RichText, Sense, Ui, vec2};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{LazyLock, Mutex, MutexGuard},
    time::{Duration, Instant},
};

const SONGS: &str =
    "Songs|*.mp3;*.flac;*.m4a;*.m4b;*.aac;*.ogg;*.oga;*.opus;*.wav;*.weba;*.webm|All files|*.*";
const AUDIO: &[&str] = &[
    "mp3", "flac", "m4a", "m4b", "aac", "ogg", "oga", "opus", "wav", "weba", "webm",
];
type Answer<T> = Task<Result<T, String>>;
#[derive(Default)]
pub struct Updates {
    pub info: Option<UpdateInfo>,
    pub msg: String,
    pub busy: bool,
    checking: Option<Answer<UpdateInfo>>,
    install: Option<(Answer<bool>, Receiver<UpdateProgress>)>,
    started: Option<Instant>,
    ask: bool,
}
#[derive(Default)]
pub struct State {
    pub updates: Updates,
    picking: Option<(bool, Answer<Vec<PathBuf>>)>,
    models: Option<Result<Vec<String>, String>>,
    models_task: Option<Answer<Vec<String>>>,
    discord_id: Option<String>,
    id_due: Option<Instant>,
    pub how_open: Option<bool>,
    /// Scroll this row of panels into view once.
    pub reveal_row: Option<usize>,
    /// Scroll the Look part of Playback to the top for this many frames (the rows settle their heights meanwhile).
    pub reveal_look: u8,
    numbers: Option<(u64, Numbers)>,
}
#[derive(Default)]
struct Numbers {
    roots: Vec<usize>,
    artists: usize,
    formats: String,
}
static STATE: LazyLock<Mutex<State>> = LazyLock::new(Default::default);
pub fn state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap()
}
/// 12,345 the way the page has always written it.
pub fn num(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}
pub fn plural(n: usize, word: &str) -> String {
    format!("{} {word}{}", num(n as u64), if n == 1 { "" } else { "s" })
}
fn long(seconds: f64) -> String {
    let s = seconds.round().max(0.) as u64;
    let (h, m) = (s / 3600, s % 3600 / 60);
    if h > 0 {
        format!("{h} hr {m} min")
    } else if m > 0 {
        format!("{m} min {} sec", s % 60)
    } else {
        format!("{s} sec")
    }
}
fn folder_prefix(path: &str) -> String {
    format!("{}\\", path.to_lowercase().trim_end_matches(['\\', '/']))
}
fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| p.as_str().map(str::to_owned))
        .collect()
}
/// Change one setting and tell whoever follows it.
fn set(v: &mut View<'_>, key: &str, value: Value) {
    v.s.set_setting(key, value);
    services::get().event(Event::Setting(key));
}
/// New music folders: a parent replaces its subfolders, and a folder already covered is skipped.
pub fn add_roots(v: &mut View<'_>, list: &[PathBuf]) {
    let mut roots = strings(v.s.field("roots"));
    let mut added = 0;
    for p in list {
        let p = p.to_string_lossy().into_owned();
        let key = p.to_lowercase();
        if roots
            .iter()
            .any(|r| r.to_lowercase() == key || key.starts_with(&folder_prefix(r)))
        {
            continue;
        }
        let inside = folder_prefix(&p);
        roots.retain(|r| !r.to_lowercase().starts_with(&inside));
        roots.push(p);
        added += 1;
    }
    if added == 0 {
        if !list.is_empty() {
            v.views.toast.set("Already in your library");
        }
        return;
    }
    v.s.set_field("roots", roots.into());
    v.views.toast.set(if added == 1 {
        "Folder added: reading its songs".to_owned()
    } else {
        format!("{added} folders added: reading their songs")
    });
    v.scan.requested = true;
}
/// Songs added one by one.
pub fn add_songs(v: &mut View<'_>, list: &[PathBuf]) {
    let mut files = strings(v.s.field("files"));
    let before = files.len();
    for p in list {
        let ext = p
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        let p = p.to_string_lossy().into_owned();
        let id = p.to_lowercase();
        if AUDIO.contains(&ext.as_str())
            && !files.iter().any(|f| f.to_lowercase() == id)
            && !v.s.models.by_id.contains_key(&id)
        {
            files.push(p);
        }
    }
    let fresh = files.len() - before;
    if fresh == 0 {
        if !list.is_empty() {
            v.views.toast.set("Those are already in your library");
        }
        return;
    }
    v.s.set_field("files", files.into());
    v.views
        .toast
        .set(format!("{} added", plural(fresh, "song")));
    v.scan.requested = true;
}
impl State {
    fn numbers(&mut self, v: &View<'_>) -> &Numbers {
        let roots = strings(v.s.field("roots"));
        if self
            .numbers
            .as_ref()
            .is_none_or(|(g, n)| *g != v.s.generation || n.roots.len() != roots.len())
        {
            let prefixes: Vec<_> = roots.iter().map(|r| folder_prefix(r)).collect();
            let mut counts = vec![0; roots.len()];
            for t in &v.s.library.tracks {
                let id = t.id();
                for (n, p) in counts.iter_mut().zip(&prefixes) {
                    *n += usize::from(id.starts_with(p));
                }
            }
            let mut formats: HashMap<&str, usize> = HashMap::new();
            for id in v.s.models.songs.iter() {
                let f = v.s.models.by_id[id].format.as_str();
                *formats
                    .entry(if f.is_empty() { "?" } else { f })
                    .or_default() += 1;
            }
            let mut formats: Vec<_> = formats.into_iter().collect();
            formats.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
            let formats = formats
                .iter()
                .map(|(k, n)| format!("{k} {n}"))
                .collect::<Vec<_>>()
                .join(" · ");
            let artists =
                v.s.models
                    .catalog
                    .artists
                    .iter()
                    .filter(|a| !a.key.is_empty())
                    .count();
            self.numbers = Some((
                v.s.generation,
                Numbers {
                    roots: counts,
                    artists,
                    formats,
                },
            ));
        }
        &self.numbers.as_ref().unwrap().1
    }
    /// Take in what the worker threads answered.
    fn poll(&mut self, ctx: &Context, v: &mut View<'_>) {
        if let Some((folders, task)) = &self.picking
            && let Some(answer) = task.poll()
        {
            let folders = *folders;
            self.picking = None;
            match answer {
                Ok(list) if folders => add_roots(v, &list),
                Ok(list) => add_songs(v, &list),
                Err(e) => v.views.toast.set(e),
            }
        }
        if let Some(answer) = self.models_task.as_ref().and_then(Task::poll) {
            self.models_task = None;
            self.models = Some(answer);
        }
        if self.id_due.is_some_and(|due| Instant::now() >= due) {
            self.id_due = None;
            let id: String = self
                .discord_id
                .as_deref()
                .unwrap_or("")
                .split_whitespace()
                .collect();
            set(v, "discordId", id.into());
        }
        let u = &mut self.updates;
        // Until a check has answered, what is known can still change (the app may connect its updater late).
        if u.checking.is_none() && !u.busy && u.info.as_ref().is_none_or(|i| i.latest.is_empty()) {
            u.info = Some(services::get().update_info());
        }
        if let Some(answer) = u.checking.as_ref().and_then(Task::poll) {
            u.checking = None;
            match answer {
                Ok(info) => {
                    let mut seen = v.s.field("update").clone();
                    if !seen.is_object() {
                        seen = json!({});
                    }
                    seen["at"] = chrono::Local::now().timestamp_millis().into();
                    v.s.set_field("update", seen);
                    if info.installed && info.published && !info.newer {
                        v.views.toast.set("String is up to date");
                    }
                    u.info = Some(info);
                }
                Err(e) => {
                    u.msg = format!("Couldn’t check: {e}");
                    v.views.toast.set(u.msg.clone());
                }
            }
        }
        if let Some((task, progress)) = &u.install {
            for step in progress.try_iter() {
                u.msg = match step {
                    UpdateProgress::Receiving { got, total } => format!(
                        "Downloading {}%",
                        (got as f64 / total.max(1) as f64 * 100.).round().min(100.)
                    ),
                    UpdateProgress::Verifying => "Checking the download…".into(),
                };
            }
            if let Some(answer) = task.poll() {
                u.install = None;
                match answer {
                    Ok(true) => {
                        u.msg = "Installing: String closes and opens again in a moment".into();
                        u.started = Some(Instant::now());
                        ctx.request_repaint_after(Duration::from_secs(91));
                    }
                    Ok(false) => u.msg = "Downloaded and checked".into(),
                    Err(e) => {
                        u.busy = false;
                        u.msg = format!("Not installed: {e}");
                        v.views.toast.set(u.msg.clone());
                    }
                }
            }
        }
        // Still here a while later: the installer gave up, so the person can try again.
        if u.busy
            && u.started
                .is_some_and(|t| t.elapsed() > Duration::from_secs(90))
        {
            u.busy = false;
            u.started = None;
            u.msg = "The update didn’t finish. If its window said String was still open, click Install again.".into();
        }
    }
}
impl Updates {
    pub fn check(&mut self, ctx: &Context) {
        if self.checking.is_none() && !self.busy {
            self.msg.clear();
            self.checking = Some(Task::run(ctx, |s| {
                s.update_check().map_err(|e| e.to_string())
            }));
        }
    }
    fn install(&mut self, ctx: &Context) {
        let Some(version) = self
            .info
            .as_ref()
            .filter(|i| i.newer)
            .map(|i| i.latest.clone())
        else {
            return;
        };
        let (tx, rx) = unbounded();
        self.busy = true;
        self.msg = "Downloading…".into();
        let wake = ctx.clone();
        self.install = Some((
            Task::run(ctx, move |s| {
                s.update_install(&version, &mut |step| {
                    let _ = tx.send(step);
                    wake.request_repaint();
                })
                .map_err(|e| e.to_string())
            }),
            rx,
        ));
    }
}
pub fn show(ui: &mut Ui, v: &mut View<'_>) {
    panel::page_title(ui, "Library", "Folders, playback and look", |_| {});
    let mut st = state();
    let st = &mut *st;
    st.poll(ui.ctx(), v);
    if st.updates.ask {
        let latest = st
            .updates
            .info
            .as_ref()
            .map_or(String::new(), |i| i.latest.clone());
        if let Some(yes) = panel::ask(
            ui.ctx(),
            &format!("Install String {latest}?"),
            "String downloads it, checks it, then closes and opens again, updated. Your library, playlists and settings stay as they are.",
            "Install",
            false,
            v.tint,
        ) {
            st.updates.ask = false;
            if yes {
                st.updates.install(ui.ctx());
            }
        }
    }
    let panels: [fn(&mut Ui, &mut View<'_>, &mut State); 5] =
        [folders, playback, discord, updates, numbers];
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            let w = ui.available_width() - 12.;
            let cols = (((w + 18.) / 338.) as usize).clamp(1, panels.len());
            let cw = ((w - 18. * (cols - 1) as f32) / cols as f32).floor();
            for (i, row) in panels.chunks(cols).enumerate() {
                let id = ui.scope_id().with(("row height", i));
                let even: f32 = ui.data(|d| d.get_temp(id)).unwrap_or(0.);
                let mut tallest = 0.0_f32;
                let whole = ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 18.;
                    for draw in row {
                        ui.allocate_ui_with_layout(
                            vec2(cw, 0.),
                            Layout::top_down(Align::Min),
                            |ui| {
                                ui.set_width(cw);
                                let (_, natural) = panel::panel(ui, even, |ui| draw(ui, v, st));
                                tallest = tallest.max(natural);
                            },
                        );
                    }
                });
                if st.reveal_row == Some(i) && even > 0. {
                    st.reveal_row = None;
                    ui.scroll_to_rect(whole.response.rect, Some(Align::Min));
                }
                if tallest != even {
                    ui.data_mut(|d| d.insert_temp(id, tallest));
                    ui.ctx().request_repaint();
                }
                ui.add_space(12.);
            }
        });
}
fn small_icon(ui: &mut Ui, v: &View<'_>, name: &str, title: &str) -> bool {
    let (r, _) = ui.allocate_exact_size(vec2(30., 30.), Sense::hover());
    ui::icon_btn(ui, r, name, title, false, v.tint).clicked()
}
fn folders(ui: &mut Ui, v: &mut View<'_>, st: &mut State) {
    let tint = v.tint;
    let svc = services::get();
    panel::panel_h(ui, "Music folders", "read in place, never changed", tint);
    let roots = strings(v.s.field("roots"));
    let counts = st.numbers(v).roots.clone();
    let mut remove = None;
    if roots.is_empty() {
        panel::empty_note(ui, "No folders yet");
    }
    for (i, root) in roots.iter().enumerate() {
        ui.push_id(i, |ui| {
            Frame::new()
                .fill(panel::black(0.3))
                .stroke((1., HAIR))
                .inner_margin(Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.set_width(ui.available_width());
                        let (r, _) = ui.allocate_exact_size(vec2(16., 30.), Sense::hover());
                        ui::glyph::draw(
                            ui.painter(),
                            egui::Rect::from_center_size(r.center(), vec2(16., 16.)),
                            "folder",
                            tint.ink(1.),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = 4.;
                            if small_icon(ui, v, "x", "Stop using this folder") {
                                remove = Some(root.clone());
                            }
                            if small_icon(ui, v, "external", "Show in Explorer") {
                                let path = PathBuf::from(root);
                                services::fire(ui.ctx(), move |s| s.reveal(&path));
                            }
                            ui.add_space(4.);
                            ui.label(
                                mono(&plural(counts.get(i).copied().unwrap_or(0), "song"), 10.5)
                                    .color(FAINT),
                            );
                            ui.add_space(4.);
                            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                ui.add(Label::new(mono(root, 12.).color(SECONDARY)).wrap());
                            });
                        });
                    });
                });
        });
    }
    ui.add_space(8.);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 10.;
        if svc.pickers() {
            if Btn::new("Add folder", tint)
                .icon("plus")
                .kind(Kind::Primary)
                .sm()
                .enabled(st.picking.is_none())
                .show(ui)
                .clicked()
            {
                st.picking = Some((
                    true,
                    Task::run(ui.ctx(), |s| {
                        s.pick_folders("Choose folders with your songs")
                            .map_err(|e| e.to_string())
                    }),
                ));
            }
        } else {
            let w = ui.available_width();
            let typed = panel::field(
                ui,
                &mut v.views.root_input,
                "A music folder's or a song's path",
                "Mono",
                w,
            );
            let enter = typed.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let add = Btn::new("Add folder", tint)
                .icon("plus")
                .kind(Kind::Primary)
                .sm()
                .show(ui)
                .clicked();
            if (add || enter) && !v.views.root_input.trim().is_empty() {
                let path = PathBuf::from(
                    std::mem::take(&mut v.views.root_input)
                        .trim()
                        .trim_matches('"'),
                );
                if path.is_dir() {
                    add_roots(v, &[path]);
                } else if path.is_file() {
                    add_songs(v, &[path]);
                } else {
                    v.views.toast.set("This path does not exist.");
                }
            }
        }
        if Btn::new("Rescan", tint)
            .icon("reload")
            .sm()
            .show(ui)
            .clicked()
        {
            v.scan.requested = true;
            v.views.toast.set("Checking your folders");
        }
    });
    if let Some(status) = v.scan.status() {
        ui.add_space(2.);
        ui.label(mono(&status, 10.5).color(FAINT));
    }
    if !svc.pickers() {
        ui.add_space(2.);
        ui.label(mono("Or drop folders and songs into the window.", 10.5).color(FAINT));
    }
    if let Some(root) = remove {
        let left: Vec<_> = roots.into_iter().filter(|r| *r != root).collect();
        v.s.set_field("roots", left.into());
        v.scan.requested = true;
    }
    ui.add_space(16.);
    let files = v.s.field("files").as_array().map_or(0, Vec::len);
    panel::panel_h(
        ui,
        "Single songs",
        &format!("{} added one by one", plural(files, "song")),
        tint,
    );
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.;
        if svc.pickers()
            && Btn::new("Add songs", tint)
                .icon("music")
                .sm()
                .enabled(st.picking.is_none())
                .show(ui)
                .clicked()
        {
            st.picking = Some((
                false,
                Task::run(ui.ctx(), |s| {
                    s.pick_files("Choose songs", SONGS, None, true)
                        .map_err(|e| e.to_string())
                }),
            ));
        }
        if files > 0
            && Btn::new("Forget them", tint)
                .kind(Kind::Ghost)
                .sm()
                .show(ui)
                .clicked()
        {
            v.s.set_field("files", json!([]));
            v.scan.requested = true;
        }
    });
    if !svc.pickers() && files == 0 {
        ui.label(mono("A song's path goes in the box above too.", 10.5).color(FAINT));
    }
    ui.add_space(16.);
    panel::panel_h(ui, "Duplicates", "the same song twice", tint);
    if Btn::new("Find duplicates", tint)
        .icon("search")
        .sm()
        .show(ui)
        .clicked()
    {
        v.go(Route::Dupes);
    }
    super::dupes::hidden_note(ui, v);
}
fn playback(ui: &mut Ui, v: &mut View<'_>, st: &mut State) {
    let tint = v.tint;
    panel::panel_h(ui, "Playback", "", tint);
    let fade = v.s.setting("fade").as_f64().unwrap_or(0.).clamp(0., 12.);
    let picked = panel::row(ui, "Crossfade between songs", |ui| {
        let text = if fade > 0. {
            format!("{fade:.0} s")
        } else {
            "Off".into()
        };
        ui.allocate_ui_with_layout(vec2(30., 18.), Layout::left_to_right(Align::Center), |ui| {
            ui.label(mono(&text, 12.).color(MUTED));
        });
        let (r, _) = ui.allocate_exact_size(vec2(120., 18.), Sense::hover());
        ui::rail(ui, r, "Crossfade", fade as f32 / 12., tint).1
    });
    if let Some(f) = picked {
        let secs = (f * 12.).round() as f64;
        if secs != fade {
            v.player.set_crossfade(secs);
            v.s.set_setting("fade", json!(secs as u32));
        }
    }
    panel::note(ui, "Off plays songs back to back with no gap.");
    let mut autoplay = v.s.setting("autoplay").as_bool().unwrap_or(true);
    if panel::switch(ui, "Autoplay when a list ends", &mut autoplay, tint) {
        v.s.set_setting("autoplay", autoplay.into());
        v.s.queue_changed(v.player);
    }
    panel::note(
        ui,
        "Skips to the next album or songs from your library instead of pausing.",
    );
    let mut online = v.s.setting("lyricsOnline").as_bool().unwrap_or(false);
    if panel::switch(ui, "Find lyrics online (LRCLIB)", &mut online, tint) {
        set(v, "lyricsOnline", online.into());
    }
    let lang =
        v.s.setting("trLang")
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or("English")
            .to_owned();
    let mut pick = None;
    panel::row(ui, "Translate lyrics into", |ui| {
        egui::ComboBox::from_id_salt("translate into")
            .width(150.)
            .wrap_mode(egui::TextWrapMode::Truncate)
            .selected_text(&lang)
            .show_ui(ui, |ui| {
                for l in string_lyrics::LANGUAGES {
                    if ui.selectable_label(*l == lang, *l).clicked() {
                        pick = Some(*l);
                    }
                }
            });
    });
    if let Some(l) = pick {
        set(v, "trLang", l.into());
    }
    let model = v.s.setting("trModel").as_str().unwrap_or("").to_owned();
    let mut pick = None;
    panel::row(ui, "Translation model", |ui| {
        let best = match &st.models {
            Some(Ok(list)) => format!(
                "Best downloaded ({})",
                string_lyrics::pick_model(list).unwrap_or("none yet")
            ),
            Some(Err(e)) => format!("Ollama: {e}"),
            None if model.is_empty() => "Best downloaded (automatic)".into(),
            None => "Best downloaded".into(),
        };
        egui::ComboBox::from_id_salt("translation model")
            .width(150.)
            .wrap_mode(egui::TextWrapMode::Truncate)
            .selected_text(if model.is_empty() { &best } else { &model })
            .show_ui(ui, |ui| {
                // The list is asked for when it opens, so a visit to this page never starts Ollama.
                if st.models.is_none() && st.models_task.is_none() {
                    st.models_task = Some(Task::run(ui.ctx(), |s| {
                        s.translation_models().map_err(|e| e.to_string())
                    }));
                }
                if ui.selectable_label(model.is_empty(), &best).clicked() {
                    pick = Some(String::new());
                }
                let listed = st.models.as_ref().and_then(|m| m.as_ref().ok());
                for m in listed.into_iter().flatten() {
                    if ui.selectable_label(*m == model, m).clicked() {
                        pick = Some(m.clone());
                    }
                }
                if !model.is_empty() && listed.is_none_or(|l| !l.contains(&model)) {
                    let _ = ui.selectable_label(true, &model);
                }
            });
    });
    if let Some(m) = pick {
        set(v, "trModel", m.into());
    }
    panel::note(
        ui,
        "Translations run on this PC with Ollama: lyrics never leave your computer. Bigger models translate better and take longer.",
    );
    panel::note(
        ui,
        "For songs without lyrics of their own: sends the title, artist, album and length to lrclib.net, a free open lyrics library.",
    );
    let mut phone = v.s.setting("phoneStats").as_bool().unwrap_or(true);
    if panel::switch(ui, "Count phone listening", &mut phone, tint) {
        set(v, "phoneStats", phone.into());
    }
    panel::note(
        ui,
        "Songs played on your phone through Navidrome on this PC count in Stats, as whole songs.",
    );
    ui.add_space(10.);
    if st.reveal_look > 0 {
        st.reveal_look -= 1;
        ui.scroll_to_cursor(Some(Align::Min));
    }
    panel::panel_h(ui, "Look", "", tint);
    let now = v.s.setting("tint").as_str().unwrap_or("mono").to_owned();
    // the person's look.css can set its own tint: shown beside String's three while it's the one drawn
    let custom = matches!(tint, Tint::Custom(_));
    let tints: &[(&str, &str)] = if custom {
        &[("mono", "Mono"), ("arc", "Arc"), ("ember", "Ember"), ("look", "Look")]
    } else {
        &[("mono", "Mono"), ("arc", "Arc"), ("ember", "Ember")]
    };
    let picked = panel::row(ui, "Tint", |ui| {
        panel::seg(
            ui,
            tints,
            if custom {
                "look"
            } else if matches!(now.as_str(), "arc" | "ember") {
                &now
            } else {
                "mono"
            },
            tint,
        )
    });
    if let Some(k) = picked
        && k != "look"
    {
        v.s.set_setting("tint", k.into());
    }
    if custom {
        panel::note(ui, "Your look.css sets the tint while “Use my look.css” is on.");
    }
    let picked = panel::row(ui, "Cover art", |ui| {
        panel::seg(
            ui,
            &[("mono", "Tinted"), ("colour", "Colour")],
            if v.mono { "mono" } else { "colour" },
            tint,
        )
    });
    if let Some(k) = picked {
        v.s.set_setting("artMono", (k == "mono").into());
    }
    let mut viz = v.s.setting("viz").as_bool().unwrap_or(true);
    if panel::switch(ui, "Spectrum in the player bar", &mut viz, tint) {
        v.s.set_setting("viz", viz.into());
    }
    // your own look: <data>\look\look.css, the same file and setting as 1.x (see theme::look)
    let open = panel::row(ui, "Your own look", |ui| {
        Btn::new("Open folder", tint).icon("folder").sm().show(ui)
    });
    if open.clicked() {
        match crate::theme::look::open_folder() {
            Ok((css, made)) => {
                if made {
                    v.views
                        .toast
                        .set("Your look folder is ready: look.css has examples to start from");
                }
                services::fire(ui.ctx(), move |s| s.reveal(&css));
            }
            Err(e) => v.views.toast.set(e.to_string()),
        }
    }
    let mut look = v.s.setting("look").as_bool().unwrap_or(true);
    if panel::switch(ui, "Use my look.css", &mut look, tint) {
        v.s.set_setting("look", look.into());
    }
    panel::note(
        ui,
        "A look.css in your look folder, with pictures beside it, can change the tint, the logo and the picture behind the pages. Updates never touch it, and String shows each saved change straight away. The first time, the folder gets a look.css full of examples. Its colours and fonts work in String 1.x only, for now.",
    );
}
fn valid_id(id: &str) -> bool {
    (15..=22).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_digit())
}
fn discord(ui: &mut Ui, v: &mut View<'_>, st: &mut State) {
    let tint = v.tint;
    panel::panel_h(ui, "Discord", "“Listening to” on your profile", tint);
    let mut on = v.s.setting("discord").as_bool().unwrap_or(false);
    if panel::switch(ui, "Show what I'm playing on Discord", &mut on, tint) {
        set(v, "discord", on.into());
    }
    let saved = v.s.setting("discordId").as_str().unwrap_or("").to_owned();
    Frame::new()
        .inner_margin(Margin::symmetric(10, 6))
        .show(ui, |ui| {
            panel::lbl(ui, "Application ID");
            ui.add_space(1.);
            let w = ui.available_width();
            let text = st.discord_id.get_or_insert_with(|| saved.clone());
            if panel::field(
                ui,
                text,
                "a long number from Discord's developer site",
                "Mono",
                w,
            )
            .changed()
            {
                // A whole ID is saved at once; anything else waits until the typing pauses.
                let whole = valid_id(&text.split_whitespace().collect::<String>());
                let wait = if whole || text.trim().is_empty() {
                    0
                } else {
                    500
                };
                st.id_due = Some(Instant::now() + Duration::from_millis(wait));
                ui.ctx()
                    .request_repaint_after(Duration::from_millis(wait + 20));
            }
        });
    let shows = v.s.setting("dcShow").as_str().unwrap_or("song").to_owned();
    let picked = panel::row_below(ui, "Your status line shows", |ui| {
        panel::seg(
            ui,
            &[("song", "Song"), ("artist", "Artist"), ("app", "App name")],
            if matches!(shows.as_str(), "artist" | "app") {
                &shows
            } else {
                "song"
            },
            tint,
        )
    });
    if let Some(k) = picked {
        set(v, "dcShow", k.into());
    }
    let mut covers = v.s.setting("dcCovers").as_bool().unwrap_or(false);
    if panel::switch(ui, "Covers from Apple's iTunes search", &mut covers, tint) {
        set(v, "dcCovers", covers.into());
    }
    let fine = |ui: &mut Ui, text: &str| {
        Frame::new()
            .inner_margin(Margin {
                left: 10,
                right: 10,
                top: 2,
                bottom: 8,
            })
            .show(ui, |ui| {
                ui.add(Label::new(mono(text, 11.).color(FAINT)).wrap());
            });
    };
    fine(
        ui,
        "Discord only shows pictures that are on the web, so your own cover files can't appear there. With this on, the song and artist name go to Apple's iTunes search to find the cover; off, there's no picture.",
    );
    let d = services::get().discord().unwrap_or_else(|| Discord {
        status: if !on {
            DiscordStatus::Off
        } else if saved.is_empty() {
            DiscordStatus::NoId
        } else if !valid_id(&saved) {
            DiscordStatus::BadId
        } else {
            DiscordStatus::Starting
        },
        ..Default::default()
    });
    let text = match d.status {
        DiscordStatus::Off => "Off".to_owned(),
        DiscordStatus::NoId => "Paste your application ID to start".into(),
        DiscordStatus::BadId => {
            "That doesn't look like an application ID: it's a long number".into()
        }
        DiscordStatus::Starting => "Starting…".into(),
        DiscordStatus::Waiting => "Waiting for Discord to open on this PC".into(),
        DiscordStatus::Connected if d.showing.is_empty() => {
            format!("Connected as {} · nothing showing while paused", d.user)
        }
        DiscordStatus::Connected => {
            format!("Connected as {} · showing “{}”", d.user, d.showing)
        }
        DiscordStatus::Refused => {
            "Discord doesn't know that application ID: check it on the developer site".into()
        }
        DiscordStatus::Stopped => "The helper stopped: trying again shortly".into(),
        DiscordStatus::Error => format!("Couldn't start the helper: {}", d.error),
    };
    Frame::new()
        .outer_margin(Margin::symmetric(10, 2))
        .inner_margin(Margin::symmetric(12, 10))
        .fill(panel::black(0.35))
        .stroke((1., HAIR2))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(9., 16.), Sense::hover());
                let dot = match d.status {
                    DiscordStatus::Connected => tint.ink(1.),
                    DiscordStatus::Waiting | DiscordStatus::Starting => tint.ink(0.45),
                    _ => GHOST,
                };
                ui.painter().circle_filled(r.center(), 3.5, dot);
                ui.add(Label::new(mono(&text, 12.).color(SECONDARY)).wrap());
            });
            if !d.error.is_empty()
                && !matches!(d.status, DiscordStatus::Error | DiscordStatus::Refused)
            {
                ui.add(Label::new(mono(&d.error, 11.).color(FAINT)).wrap());
            }
        });
    ui.add_space(6.);
    let open = st.how_open.get_or_insert(saved.is_empty());
    let head = panel::line(
        ui,
        &format!(
            "{} GETTING AN APPLICATION ID (ONCE)",
            if *open { "▾" } else { "▸" }
        ),
        "HUD",
        10.5,
        0.16,
    );
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 22.), Sense::click());
    ui.painter().galley(
        egui::pos2(r.left() + 10., r.center().y - head.size().y * 0.5),
        head,
        if resp.hovered() { TEXT } else { MUTED },
    );
    if resp
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
    {
        *open = !*open;
    }
    if !*open {
        return;
    }
    let plain = |s: &str| RichText::new(s).font(font("UI", 12.5)).color(SECONDARY);
    let bold = |s: &str| RichText::new(s).font(font("Strong", 12.5)).color(TEXT);
    let step = |ui: &mut Ui, n: usize, parts: &mut dyn FnMut(&mut Ui)| {
        ui.horizontal_top(|ui| {
            ui.add_space(12.);
            ui.label(plain(&format!("{n}.")));
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(0., 3.);
                parts(ui);
            });
        });
    };
    step(ui, 1, &mut |ui| {
        ui.label(plain("Open "));
        let link = ui.add(
            Label::new(plain("Discord's developer site").underline())
                .selectable(false)
                .sense(Sense::click()),
        );
        if link
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
        {
            services::fire(ui.ctx(), |s| {
                s.open_link("https://discord.com/developers/applications")
            });
        }
        ui.label(plain(" and sign in with your Discord account."));
    });
    step(ui, 2, &mut |ui| {
        ui.label(plain("Press "));
        ui.label(bold("New Application"));
        ui.label(plain(
            ". Its name is what shows after “Listening to” on your profile, so call it ",
        ));
        ui.label(bold("String"));
        ui.label(plain(
            " (or anything you like). You can give it an icon there too.",
        ));
    });
    step(ui, 3, &mut |ui| {
        ui.label(plain("On its "));
        ui.label(bold("General Information"));
        ui.label(plain(" page, copy the "));
        ui.label(bold("Application ID"));
        ui.label(plain(", paste it above and turn the switch on."));
    });
    step(ui, 4, &mut |ui| {
        ui.label(plain("In Discord, "));
        ui.label(bold("Settings → Activity Privacy"));
        ui.label(plain(": sharing your activity has to be on."));
    });
    fine(
        ui,
        "Discord has to be open on this PC (the app, not the website). Your status clears when you pause or close String.",
    );
}
fn updates(ui: &mut Ui, v: &mut View<'_>, st: &mut State) {
    let tint = v.tint;
    let u = &mut st.updates;
    let Some(info) = u.info.clone() else { return };
    panel::panel_h(ui, "Updates", &format!("String {}", info.current), tint);
    if !info.installed {
        panel::note(
            ui,
            "This copy of String was built from its source code, so it doesn’t update itself: build it again for a newer one.",
        );
    } else if !info.published {
        panel::note(ui, "This copy of String has nowhere to get updates from.");
    } else {
        let mut daily = v.s.setting("updates").as_bool().unwrap_or(true);
        if panel::switch(ui, "Check for updates once a day", &mut daily, tint) {
            set(v, "updates", daily.into());
        }
        if info.newer {
            Frame::new()
                .outer_margin(Margin::symmetric(0, 4))
                .inner_margin(Margin::symmetric(14, 12))
                .fill(panel::white(0.02))
                .stroke((1., HAIR2))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let head = panel::line(
                        ui,
                        &format!("STRING {} IS OUT", info.latest.to_uppercase()),
                        "HUD",
                        12.,
                        0.12,
                    );
                    let (r, _) = ui.allocate_exact_size(head.size(), Sense::hover());
                    ui.painter().galley(r.min, head, TEXT);
                    if !info.notes.is_empty() {
                        ui.add_space(4.);
                        egui::ScrollArea::vertical()
                            .id_salt("release notes")
                            .max_height(160.)
                            .show(ui, |ui| {
                                ui.add(
                                    Label::new(
                                        RichText::new(&info.notes)
                                            .font(font("UI", 12.))
                                            .color(MUTED),
                                    )
                                    .wrap(),
                                );
                            });
                    }
                    ui.add_space(6.);
                    let label = if u.busy {
                        u.msg.clone()
                    } else {
                        format!("Install {}", info.latest)
                    };
                    if Btn::new(&label, tint)
                        .icon("down")
                        .kind(Kind::Primary)
                        .sm()
                        .enabled(!u.busy)
                        .show(ui)
                        .clicked()
                    {
                        u.ask = true;
                    }
                });
        }
        ui.add_space(4.);
        ui.horizontal(|ui| {
            let checking = u.checking.is_some();
            if Btn::new(if checking { "Checking…" } else { "Check now" }, tint)
                .icon("reload")
                .sm()
                .enabled(!checking && !u.busy)
                .show(ui)
                .clicked()
            {
                u.check(ui.ctx());
            }
            if !info.latest.is_empty() && !info.newer {
                ui.label(mono("UP TO DATE", 10.5).color(FAINT));
            }
        });
        ui.add_space(4.);
        panel::note(
            ui,
            "Checking asks GitHub, where String is published, for its newest release: nothing about you or your music goes with it. Updates are signed, so String only installs ones from its publisher.",
        );
    }
    if !u.msg.is_empty() && !u.busy {
        panel::note(ui, &u.msg);
    }
}
fn numbers(ui: &mut Ui, v: &mut View<'_>, st: &mut State) {
    panel::panel_h(ui, "Numbers", "", v.tint);
    let n = st.numbers(v);
    let (artists, formats) = (n.artists, n.formats.clone());
    let plays: u64 =
        v.s.field("plays")
            .as_object()
            .into_iter()
            .flatten()
            .map(|(_, p)| p[0].as_u64().unwrap_or(0))
            .sum();
    let liked = v.s.field("liked").as_object().map_or(0, |m| m.len());
    let checked =
        v.s.library
            .saved
            .and_then(|ms| chrono::Local.timestamp_millis_opt(ms).single())
            .map_or(String::new(), |t| {
                format!("checked {}", t.format("%-I:%M %p"))
            });
    let cells = [
        (
            "Songs",
            num(v.s.models.songs.len() as u64),
            long(v.s.models.duration),
            25.,
        ),
        (
            "Albums",
            num(v.s.models.catalog.albums.len() as u64),
            plural(artists, "artist"),
            25.,
        ),
        ("Plays", num(plays), plural(liked, "liked song"), 25.),
        (
            "Formats",
            if formats.is_empty() {
                "–".into()
            } else {
                formats
            },
            checked,
            15.,
        ),
    ];
    let w = ui.available_width();
    let cols = (((w + 10.) / 130.) as usize).clamp(1, 4);
    let cw = ((w - 10. * (cols - 1) as f32) / cols as f32).floor();
    for row in cells.chunks(cols) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.;
            for (label, value, sub, size) in row {
                panel::kpi(ui, cw, label, value, sub, *size);
            }
        });
        ui.add_space(4.);
    }
    ui.add_space(8.);
    ui.add(
        Label::new(
            mono(
                "Keys: Space play/pause · Ctrl+←/→ previous/next · ←/→ seek · Ctrl+↑/↓ volume · M mute · S shuffle · R repeat · L like · Q queue · F now playing · Y lyrics · [ ] loop start/end · \\ stop the loop · / search",
                10.5,
            )
            .color(FAINT),
        )
        .wrap(),
    );
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn numbers_read_the_way_the_page_writes_them() {
        assert_eq!(num(0), "0");
        assert_eq!(num(999), "999");
        assert_eq!(num(1000), "1,000");
        assert_eq!(num(12345678), "12,345,678");
        assert_eq!(plural(1, "song"), "1 song");
        assert_eq!(plural(2400, "song"), "2,400 songs");
        assert_eq!(long(9.4), "9 sec");
        assert_eq!(long(131.), "2 min 11 sec");
        assert_eq!(long(7260.), "2 hr 1 min");
    }
    #[test]
    fn a_folder_only_covers_what_is_inside_it() {
        let p = folder_prefix("C:\\Music\\");
        assert!("c:\\music\\a.mp3".starts_with(&p));
        assert!(!"c:\\music two\\a.mp3".starts_with(&p));
    }
    #[test]
    fn an_application_id_is_a_long_number() {
        assert!(valid_id("123456789012345678"));
        assert!(!valid_id("12345"));
        assert!(!valid_id("12345678901234567x"));
    }
}
/// Smoke cases for this page.
pub mod smoke {
    use super::*;
    pub fn open(v: &mut View<'_>) {
        crate::ui::edit_info::close();
        v.go(Route::Settings);
    }
    pub fn ready(v: &mut View<'_>) -> bool {
        v.views.history.current == Route::Settings
    }
    /// The Updates panel offering a release, and Numbers beside it.
    pub fn updates(v: &mut View<'_>) {
        v.go(Route::Settings);
        let mut st = state();
        st.updates.info = Some(UpdateInfo {
            current: env!("CARGO_PKG_VERSION").into(),
            installed: true,
            published: true,
            latest: "9.9.9".into(),
            notes: "Sample notes for a release".into(),
            newer: true,
        });
        st.how_open = Some(false);
        st.reveal_row = Some(1);
    }
    pub fn updates_ready(_: &mut View<'_>) -> bool {
        state().reveal_row.is_none()
    }
    pub fn tidy() {
        let mut st = state();
        st.updates = Default::default();
        st.how_open = None;
    }
    static LOOK_AT: Mutex<Option<Instant>> = Mutex::new(None);
    /// Settings → Look, with whatever look the test profile has (a planted `look\look.css` shows its tint, logo and
    /// background here and on every page before).
    pub fn look(v: &mut View<'_>) {
        tidy();
        v.go(Route::Settings);
        state().reveal_look = 6;
        *LOOK_AT.lock().unwrap() = Some(Instant::now());
    }
    /// Once the look matches the switch, its pictures are on screen and the scroll has come to rest.
    pub fn look_ready(v: &mut View<'_>) -> bool {
        let on = v.s.setting("look").as_bool().unwrap_or(true);
        v.views.history.current == Route::Settings
            && state().reveal_look == 0
            && LOOK_AT.lock().unwrap().is_some_and(|t| t.elapsed() > Duration::from_millis(700))
            && crate::theme::look::settled(on)
    }
}
