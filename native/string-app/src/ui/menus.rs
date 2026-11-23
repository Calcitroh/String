//! The song menu, the list menu, the small dialogs they open and the actions behind their entries.
//!
//! Open a menu from any screen:
//!
//! ```ignore
//! if response.secondary_clicked() {
//!     menus::track_menu(selected_ids, pointer_pos, Origin::list(list_ids, row, v.context()));
//! }
//! menus::ctx_menu(json!({"kind":"album","key":key}), pos, None);
//! ```
//!
//! `overlay` draws whatever is open once per frame, after the screens.
use crate::{
    model::{Ids, Models},
    playback::Session,
    route::Route,
    theme::*,
    ui::{self, area, glyph},
    view::View,
};
use egui::{
    Align, Align2, Area, Color32, Context, Frame, Id, Key, Layout, Modifiers, Order, Painter, Pos2,
    Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui,
    epaint::Shadow,
    pos2,
    text::{CCursor, CCursorRange, LayoutJob},
    vec2,
};
use serde_json::{Value, json};
use std::{
    cmp::Reverse,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
};
use string_audio::Player;

/* ---------- hooks other parts fill in ---------- */

/// Work the menus hand to other parts of the app. An entry whose hook is empty is left out,
/// except Show in Explorer, which falls back to opening Explorer itself.
pub struct Hooks {
    /// Open the Edit info dialog for this song id.
    pub edit_info: Option<fn(&mut View<'_>, &str)>,
    /// Show this file or folder in Explorer.
    pub reveal: Option<fn(&mut View<'_>, &Path)>,
}
static HOOKS: Mutex<Hooks> = Mutex::new(Hooks {
    edit_info: None,
    reveal: None,
});
/// `menus::hooks().edit_info = Some(open_edit_info);`
pub fn hooks() -> MutexGuard<'static, Hooks> {
    HOOKS.lock().unwrap()
}

/* ---------- state ---------- */

/// Where the songs of a song menu were picked from.
#[derive(Clone, Default)]
pub struct Origin {
    /// The list shown and the row Play starts at (the first selected row for several songs).
    pub list: Option<(Ids, usize)>,
    /// What the queue says it plays from: `{"kind","key","name"}`.
    pub ctx: Option<Value>,
    /// A playlist id and the places of the picked songs in its items, for "Remove from this playlist".
    pub playlist: Option<(String, Vec<usize>)>,
}
impl Origin {
    /// A song picked outside any list: Play starts just these songs.
    pub fn none() -> Self {
        Self::default()
    }
    pub fn list(ids: Ids, row: usize, ctx: Value) -> Self {
        Self {
            list: Some((ids, row)),
            ctx: Some(ctx),
            playlist: None,
        }
    }
}
#[derive(Clone)]
enum Kind {
    Track { ids: Ids, origin: Origin },
    List { ctx: Value, visible: Option<Ids> },
    Picker { ids: Ids },
}
struct Menu {
    kind: Kind,
    at: Pos2,
    sub: bool,
    rect: Rect,
    fresh: bool,
}
enum Purpose {
    NewPlaylist { ids: Vec<String>, open: bool },
    Rename(String),
    Delete(String),
}
struct Dialog {
    title: String,
    text: String,
    value: Option<String>,
    ok: &'static str,
    danger: bool,
    purpose: Purpose,
    /// Frames left to focus the field and select its text.
    fresh: u8,
    answer: Option<bool>,
}
struct State {
    menu: Option<Menu>,
    dialog: Option<Dialog>,
    drop_demo: bool,
}
static STATE: Mutex<State> = Mutex::new(State {
    menu: None,
    dialog: None,
    drop_demo: false,
});
fn state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap()
}

/* ---------- opening ---------- */

fn open(kind: Kind, at: Pos2) {
    state().menu = Some(Menu {
        kind,
        at,
        sub: false,
        rect: Rect::NOTHING,
        fresh: true,
    });
}
/// The menu for one or more songs, at a screen position.
pub fn track_menu(ids: impl Into<Ids>, at: Pos2, origin: Origin) {
    let ids = ids.into();
    if !ids.is_empty() {
        open(Kind::Track { ids, origin }, at);
    }
}
/// The menu for an album, artist, playlist or other list: `ctx` is `{"kind","key"}`.
/// `visible` is the filtered list on screen, when there is one.
pub fn ctx_menu(ctx: Value, at: Pos2, visible: Option<Ids>) {
    open(Kind::List { ctx, visible }, at);
}
/// Just "Add to playlist" for these songs.
pub fn playlist_picker(ids: impl Into<Ids>, at: Pos2) {
    open(Kind::Picker { ids: ids.into() }, at);
}
/// Where a right-click on this widget landed: `if let Some(at) = menu_pos(&response) { track_menu(.., at, ..) }`.
pub fn menu_pos(res: &Response) -> Option<Pos2> {
    res.secondary_clicked()
        .then(|| res.interact_pointer_pos().unwrap_or(res.rect.center()))
}
pub fn close_menu() {
    state().menu = None;
}
pub fn is_open() -> bool {
    let s = state();
    s.menu.is_some() || s.dialog.is_some()
}
/// Ask for a name, then make the playlist with these songs; `open` also goes to its page.
pub fn new_playlist(v: &View<'_>, ids: Vec<String>, open: bool) {
    state().dialog = Some(Dialog {
        title: "New playlist".into(),
        text: String::new(),
        value: Some(format!("Playlist {}", v.s.models.playlists.len() + 1)),
        ok: "Create",
        danger: false,
        purpose: Purpose::NewPlaylist { ids, open },
        fresh: 4,
        answer: None,
    });
}
pub fn rename_playlist(v: &View<'_>, key: &str) {
    if let Some(p) = v.s.models.playlists.iter().find(|p| p.id == key) {
        state().dialog = Some(Dialog {
            title: "Rename playlist".into(),
            text: String::new(),
            value: Some(p.name.clone()),
            ok: "Rename",
            danger: false,
            purpose: Purpose::Rename(key.into()),
            fresh: 4,
            answer: None,
        });
    }
}
pub fn delete_playlist(v: &View<'_>, key: &str) {
    if let Some(p) = v.s.models.playlists.iter().find(|p| p.id == key) {
        state().dialog = Some(Dialog {
            title: "Delete playlist".into(),
            text: format!("Delete “{}”? The songs stay in your library.", p.name),
            value: None,
            ok: "Delete",
            danger: true,
            purpose: Purpose::Delete(key.into()),
            fresh: 4,
            answer: None,
        });
    }
}

/* ---------- words ---------- */

pub fn plural(n: usize, word: &str) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    format!("{out} {word}{}", if n == 1 { "" } else { "s" })
}
/// "2 hr 5 min", "1 min 47 sec" or "22 sec".
pub fn long(seconds: f64) -> String {
    let s = seconds.max(0.).round() as u64;
    let (h, m) = (s / 3600, s % 3600 / 60);
    if h > 0 {
        format!("{h} hr {m} min")
    } else if m > 0 {
        format!("{m} min {} sec", s % 60)
    } else {
        format!("{s} sec")
    }
}
/// "Today", "Yesterday", "3 days ago", then the date.
pub fn when(ms: i64) -> String {
    use chrono::Datelike;
    let Some(t) = chrono::DateTime::from_timestamp_millis(ms).filter(|_| ms > 0) else {
        return String::new();
    };
    let day = t.with_timezone(&chrono::Local).date_naive();
    let today = chrono::Local::now().date_naive();
    match (today - day).num_days() {
        i64::MIN..1 => "Today".into(),
        1 => "Yesterday".into(),
        d @ 2..7 => format!("{d} days ago"),
        _ if day.year() == today.year() => day.format("%b %-d").to_string(),
        _ => day.format("%b %-d, %Y").to_string(),
    }
}
pub fn is_liked(s: &Session, id: &str) -> bool {
    match &s.field("liked")[id] {
        Value::Null | Value::Bool(false) => false,
        Value::Number(n) => n.as_f64() != Some(0.),
        _ => true,
    }
}
pub fn same_ctx(a: &Value, b: &Value) -> bool {
    a["kind"].is_string()
        && a["kind"] == b["kind"]
        && a["key"].as_str().unwrap_or("") == b["key"].as_str().unwrap_or("")
}
/// The list's name as it is now (a renamed playlist shows its new name).
pub fn ctx_name(m: &Models, c: &Value) -> String {
    let key = c["key"].as_str().unwrap_or("");
    let stored = c["name"].as_str().unwrap_or("");
    let group = |groups: &[string_core::queue::Group]| {
        groups
            .iter()
            .find(|g| g.key == key)
            .map_or(stored, |g| &g.name)
            .to_owned()
    };
    match c["kind"].as_str() {
        Some("album") => group(&m.catalog.albums),
        Some("artist") => group(&m.catalog.artists),
        Some("pl") => m
            .playlists
            .iter()
            .find(|p| p.id == key)
            .map_or(stored, |p| &p.name)
            .to_owned(),
        Some("liked") => "Liked songs".into(),
        Some("songs") => "All songs".into(),
        Some("search") => format!("“{key}”"),
        _ => stored.into(),
    }
}
/// The page a list lives on, when it has one.
pub fn ctx_route(c: &Value) -> Option<Route> {
    let key = || c["key"].as_str().unwrap_or("").to_owned();
    match c["kind"].as_str()? {
        "album" => Some(Route::Album(key())),
        "artist" => Some(Route::Artist(key())),
        "pl" => Some(Route::Playlist(key())),
        "liked" => Some(Route::Liked),
        "songs" => Some(Route::Songs),
        _ => None,
    }
}
fn named(m: &Models, c: &Value) -> Value {
    json!({"kind": c["kind"], "key": c["key"].as_str().unwrap_or(""), "name": ctx_name(m, c)})
}
fn title_of(m: &Models, id: &str) -> String {
    m.by_id.get(id).map(|s| s.title.clone()).unwrap_or_default()
}

/* ---------- playlists and the queue in the session ---------- */

impl Session {
    /// Queue songs by hand, next or after the others queued by hand. Returns the ones taken.
    pub fn enqueue(&mut self, p: &Player, ids: &[String], later: bool) -> Vec<String> {
        let added = self.state.queue.insert(ids, later, &self.models.catalog);
        if !added.is_empty() {
            self.queue_changed(p);
        }
        added
    }
    fn playlist_items(&mut self, key: &str) -> Option<&mut Value> {
        let pl = self
            .state
            .fields
            .get_mut("playlists")?
            .as_array_mut()?
            .iter_mut()
            .find(|p| p["id"] == key && p.is_object())?;
        if !pl["items"].is_array() {
            pl["items"] = json!([]);
        }
        Some(pl)
    }
    fn lists_changed(&mut self) {
        self.models.refresh_lists(&self.state);
        self.generation += 1;
        self.state_dirty = true;
    }
    /// Each playlist entry in order: its place in the items and its song id and added time.
    pub fn playlist_entries(&self, key: &str) -> Vec<(usize, String, i64)> {
        self.field("playlists")
            .as_array()
            .into_iter()
            .flatten()
            .find(|p| p["id"] == key)
            .and_then(|p| p["items"].as_array())
            .into_iter()
            .flatten()
            .enumerate()
            .filter_map(|(i, v)| {
                let id = v.as_str().or(v["id"].as_str())?;
                let at = v["at"].as_f64().unwrap_or(0.) as i64;
                Some((i, id.to_owned(), at))
            })
            .collect()
    }
    pub fn new_playlist(&mut self, name: &str, ids: &[String]) -> String {
        let now = chrono::Local::now().timestamp_millis();
        let id = format!("{:x}", rand::random::<u64>());
        let lists = self
            .state
            .fields
            .entry("playlists")
            .or_insert_with(|| json!([]));
        if !lists.is_array() {
            *lists = json!([]);
        }
        let items: Vec<_> = ids.iter().map(|id| json!({"id": id, "at": now})).collect();
        lists
            .as_array_mut()
            .unwrap()
            .push(json!({"id": id, "name": name, "items": items, "created": now, "updated": now}));
        self.lists_changed();
        id
    }
    /// Add the songs the playlist doesn't have yet. Returns those.
    pub fn playlist_add(&mut self, key: &str, ids: &[String]) -> Vec<String> {
        let now = chrono::Local::now().timestamp_millis();
        let Some(pl) = self.playlist_items(key) else {
            return vec![];
        };
        let items = pl["items"].as_array_mut().unwrap();
        let mut fresh = vec![];
        for id in ids {
            let has = |v: &Value| v.as_str() == Some(id) || v["id"] == *id;
            if !items.iter().any(has) && !fresh.contains(id) {
                fresh.push(id.clone());
            }
        }
        if fresh.is_empty() {
            return fresh;
        }
        items.extend(fresh.iter().map(|id| json!({"id": id, "at": now})));
        pl["updated"] = now.into();
        self.lists_changed();
        fresh
    }
    pub fn playlist_rename(&mut self, key: &str, name: &str) {
        if let Some(pl) = self
            .state
            .fields
            .get_mut("playlists")
            .and_then(Value::as_array_mut)
            .and_then(|a| a.iter_mut().find(|p| p["id"] == key))
        {
            pl["name"] = name.into();
            pl["updated"] = chrono::Local::now().timestamp_millis().into();
            self.lists_changed();
        }
    }
    /// Returns the deleted playlist's name.
    pub fn playlist_delete(&mut self, key: &str) -> Option<String> {
        let lists = self.state.fields.get_mut("playlists")?.as_array_mut()?;
        let i = lists.iter().position(|p| p["id"] == key)?;
        let gone = lists.remove(i);
        self.lists_changed();
        Some(gone["name"].as_str().unwrap_or("").to_owned())
    }
    /// Take out the entries at these places in the playlist's items.
    pub fn playlist_remove(&mut self, key: &str, places: &[usize]) {
        let now = chrono::Local::now().timestamp_millis();
        let Some(pl) = self.playlist_items(key) else {
            return;
        };
        let items = pl["items"].as_array_mut().unwrap();
        let before = items.len();
        let mut i = 0;
        items.retain(|_| {
            i += 1;
            !places.contains(&(i - 1))
        });
        if items.len() != before {
            pl["updated"] = now.into();
            self.lists_changed();
        }
    }
    /// Move the entries at these places so they sit before the entry now at `before`.
    pub fn playlist_move(&mut self, key: &str, places: &[usize], before: usize) {
        let now = chrono::Local::now().timestamp_millis();
        let Some(pl) = self.playlist_items(key) else {
            return;
        };
        let items = pl["items"].as_array_mut().unwrap();
        let mut places: Vec<_> = places
            .iter()
            .copied()
            .filter(|&i| i < items.len())
            .collect();
        places.sort_unstable();
        places.dedup();
        if places.is_empty() {
            return;
        }
        let at = before.min(items.len()) - places.iter().filter(|&&i| i < before).count();
        let mut moving = Vec::with_capacity(places.len());
        for &i in places.iter().rev() {
            moving.push(items.remove(i));
        }
        moving.reverse();
        let at = at.min(items.len());
        items.splice(at..at, moving);
        pl["updated"] = now.into();
        self.lists_changed();
    }
    /// The title becomes the artist and the artist the title. Returns the new pair.
    pub fn swap_names(&mut self, p: &Player, id: &str) -> Option<(String, String)> {
        let song = self.models.by_id.get(id)?;
        let (title, artist) = (song.artist.clone(), song.title.clone());
        if title.is_empty() {
            return None;
        }
        let edits = self
            .state
            .fields
            .entry("edits")
            .or_insert_with(|| json!({}));
        if !edits.is_object() {
            *edits = json!({});
        }
        let entry = &mut edits[id];
        if !entry.is_object() {
            *entry = json!({});
        }
        entry["title"] = title.clone().into();
        entry["artist"] = artist.clone().into();
        entry.as_object_mut().unwrap().remove("auto");
        self.rebuild();
        self.queue_changed(p);
        Some((title, artist))
    }
}

/* ---------- actions, each with the line it says ---------- */

pub fn play_next(v: &mut View<'_>, ids: &[String]) {
    v.s.enqueue(v.player, ids, false);
    v.views.toast.set(if let [id] = ids {
        format!("{} plays next", title_of(&v.s.models, id))
    } else {
        format!("{} play next", plural(ids.len(), "song"))
    });
}
/// Add to the queue and say what was really added.
pub fn queue_tracks(v: &mut View<'_>, ids: &[String]) {
    let added = v.s.enqueue(v.player, ids, true);
    let name = match &added[..] {
        [] => return,
        [id] => title_of(&v.s.models, id),
        _ => String::new(),
    };
    v.views.toast.set(format!(
        "{} added to the queue",
        if name.is_empty() {
            plural(added.len(), "song")
        } else {
            name
        }
    ));
}
pub fn add_to_playlist(v: &mut View<'_>, key: &str, ids: &[String]) {
    let Some(name) =
        v.s.models
            .playlists
            .iter()
            .find(|p| p.id == key)
            .map(|p| p.name.clone())
    else {
        return;
    };
    let fresh = v.s.playlist_add(key, ids);
    v.views.toast.set(match &fresh[..] {
        [] => format!("Already in {name}"),
        [id] => format!("{} added to {name}", title_of(&v.s.models, id)),
        _ => format!("{} added to {name}", plural(fresh.len(), "song")),
    });
}
pub fn toggle_like(v: &mut View<'_>, ids: &[String]) {
    let on = !ids.iter().all(|id| is_liked(v.s, id));
    v.s.toggle_like(ids);
    v.views.toast.set(if on {
        format!("Added to Liked ({})", ids.len())
    } else {
        "Removed from Liked".into()
    });
}
/// The big play button of a list: pause or resume when it is what plays, else start it.
pub fn play_ctx(v: &mut View<'_>, ctx: &Value, visible: Option<&Ids>) {
    if visible.is_none() && same_ctx(v.s.state.queue.context(None), ctx) && v.s.current().is_some()
    {
        v.s.toggle(v.player);
        return;
    }
    let ids = visible
        .cloned()
        .unwrap_or_else(|| v.s.models.context_ids(ctx));
    if ids.is_empty() {
        v.views.toast.set("Nothing to play there yet");
        return;
    }
    let at = if v.s.state.queue.shuffled() { -1 } else { 0 };
    let ctx = named(&v.s.models, ctx);
    v.s.play_ids(v.player, &ids, at, ctx);
}
pub fn shuffle_ctx(v: &mut View<'_>, ctx: &Value, visible: Option<&Ids>) {
    let ids = visible
        .cloned()
        .unwrap_or_else(|| v.s.models.context_ids(ctx));
    if ids.is_empty() {
        return;
    }
    if !v.s.state.queue.shuffled() {
        v.shuffle();
    }
    let ctx = named(&v.s.models, ctx);
    v.s.play_ids(v.player, &ids, -1, ctx);
}
pub fn reveal(v: &mut View<'_>, path: &Path) {
    let hook = hooks().reveal;
    if let Some(f) = hook {
        f(v, path);
    } else if !v.force_mute {
        use std::os::windows::process::CommandExt;
        let shown = std::process::Command::new("explorer.exe")
            .raw_arg(format!("/select,\"{}\"", path.display()))
            .spawn();
        if let Err(e) = shown {
            v.views.toast.set(format!("Explorer did not open: {e}"));
        }
    }
}

/* ---------- the menu ---------- */

#[derive(Clone)]
enum Cmd {
    Play,
    Shuffle,
    PlayNext,
    Queue,
    Picker,
    Back,
    NewPlaylist,
    AddTo(String),
    Like,
    RemoveFromPlaylist,
    Go(Route),
    EditInfo(String),
    Swap(String),
    Reveal(PathBuf),
    Rename(String),
    Delete(String),
}
enum Entry {
    Head(String),
    Sep,
    Item {
        label: String,
        icon: &'static str,
        key: &'static str,
        cmd: Cmd,
    },
}
impl Entry {
    fn height(&self) -> f32 {
        match self {
            Self::Head(_) => 26.,
            Self::Sep => 11.,
            Self::Item { .. } => 34.,
        }
    }
}
fn item(label: &str, icon: &'static str, cmd: Cmd) -> Entry {
    Entry::Item {
        label: label.into(),
        icon,
        key: "",
        cmd,
    }
}
fn picker(v: &View<'_>, out: &mut Vec<Entry>) {
    out.push(item("New playlist…", "plus", Cmd::NewPlaylist));
    let mut lists: Vec<_> =
        v.s.field("playlists")
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| {
                Some((
                    p["updated"].as_f64().unwrap_or(0.) as i64,
                    p["id"].as_str()?,
                    p["name"].as_str()?,
                ))
            })
            .collect();
    if !lists.is_empty() {
        out.push(Entry::Sep);
    }
    lists.sort_by_key(|l| Reverse(l.0));
    for (_, id, name) in lists {
        out.push(item(name, "list", Cmd::AddTo(id.into())));
    }
}
fn list_ids(v: &View<'_>, ctx: &Value, visible: &Option<Ids>) -> Ids {
    visible
        .clone()
        .unwrap_or_else(|| v.s.models.context_ids(ctx))
}
fn entries(v: &View<'_>, m: &Menu) -> Vec<Entry> {
    let mut out = vec![];
    if m.sub {
        out.push(item("Back", "back", Cmd::Back));
        out.push(Entry::Sep);
        picker(v, &mut out);
        return out;
    }
    let sub = Entry::Item {
        label: "Add to playlist".into(),
        icon: "plus",
        key: "›",
        cmd: Cmd::Picker,
    };
    match &m.kind {
        Kind::Picker { .. } => {
            out.push(Entry::Head("Add to playlist".into()));
            picker(v, &mut out);
        }
        Kind::Track { ids, origin } => {
            let one = ids.len() == 1;
            let song = v.s.models.by_id.get(&ids[0]).filter(|_| one);
            out.push(Entry::Head(
                song.map_or_else(|| plural(ids.len(), "song"), |s| s.title.clone()),
            ));
            out.push(item("Play", "playf", Cmd::Play));
            out.push(item("Play next", "playnext", Cmd::PlayNext));
            out.push(item("Add to queue", "queue", Cmd::Queue));
            out.push(Entry::Sep);
            out.push(sub);
            out.push(Entry::Item {
                label: if ids.iter().all(|id| is_liked(v.s, id)) {
                    "Remove from Liked"
                } else {
                    "Add to Liked"
                }
                .into(),
                icon: "heart",
                key: "L",
                cmd: Cmd::Like,
            });
            if origin.playlist.is_some() {
                out.push(item(
                    "Remove from this playlist",
                    "trash",
                    Cmd::RemoveFromPlaylist,
                ));
            }
            if let Some(s) = song {
                out.push(Entry::Sep);
                if !s.album_key.is_empty() {
                    out.push(item(
                        "Go to album",
                        "disc",
                        Cmd::Go(Route::Album(s.album_key.clone())),
                    ));
                }
                if !s.artist.is_empty() {
                    out.push(item(
                        "Go to artist",
                        "user",
                        Cmd::Go(Route::Artist(s.artist_key.clone())),
                    ));
                }
                if hooks().edit_info.is_some() {
                    out.push(item("Edit info", "pencil", Cmd::EditInfo(s.id.clone())));
                }
                if !s.artist.is_empty() {
                    out.push(item(
                        "Swap title and artist",
                        "swap",
                        Cmd::Swap(s.id.clone()),
                    ));
                }
                out.push(item(
                    "Show in Explorer",
                    "folder",
                    Cmd::Reveal(s.path.clone().into()),
                ));
            }
        }
        Kind::List { ctx, visible } => {
            out.push(Entry::Head(ctx_name(&v.s.models, ctx)));
            out.push(item("Play", "playf", Cmd::Play));
            out.push(item("Shuffle", "shuffle", Cmd::Shuffle));
            out.push(item("Play next", "playnext", Cmd::PlayNext));
            out.push(item("Add to queue", "queue", Cmd::Queue));
            out.push(Entry::Sep);
            out.push(sub);
            let key = ctx["key"].as_str().unwrap_or("");
            if ctx["kind"] == "pl" && v.s.models.playlists.iter().any(|p| p.id == key) {
                out.push(Entry::Sep);
                out.push(item("Rename", "pencil", Cmd::Rename(key.into())));
                out.push(item("Delete playlist", "trash", Cmd::Delete(key.into())));
            }
            if matches!(ctx["kind"].as_str(), Some("album" | "artist"))
                && let Some(dir) = list_ids(v, ctx, visible)
                    .first()
                    .and_then(|id| v.s.models.by_id.get(id))
                    .and_then(|s| Path::new(&s.path).parent())
            {
                out.push(Entry::Sep);
                out.push(item("Show in Explorer", "folder", Cmd::Reveal(dir.into())));
            }
        }
    }
    out
}
fn run(v: &mut View<'_>, kind: Kind, cmd: Cmd) {
    let ids = match &kind {
        Kind::Track { ids, .. } | Kind::Picker { ids } => ids.clone(),
        Kind::List { ctx, visible } => list_ids(v, ctx, visible),
    };
    match cmd {
        Cmd::Play => match &kind {
            Kind::List { ctx, visible } => play_ctx(v, ctx, visible.as_ref()),
            Kind::Track { origin, .. } => {
                let ctx = origin
                    .ctx
                    .as_ref()
                    .map_or(Value::Null, |c| named(&v.s.models, c));
                if let Some((list, row)) = &origin.list
                    && let Some(id) = list.get(*row)
                {
                    if v.s.state.queue.current() == Some(id.as_str())
                        && same_ctx(v.s.state.queue.context(None), &ctx)
                    {
                        v.s.toggle(v.player);
                    } else {
                        v.s.play_ids(v.player, list, *row as isize, ctx);
                    }
                } else {
                    v.s.play_ids(v.player, &ids, 0, ctx);
                }
            }
            Kind::Picker { .. } => {}
        },
        Cmd::Shuffle => {
            if let Kind::List { ctx, visible } = &kind {
                shuffle_ctx(v, ctx, visible.as_ref());
            }
        }
        Cmd::PlayNext => {
            if !ids.is_empty() {
                play_next(v, &ids);
            }
        }
        Cmd::Queue => queue_tracks(v, &ids),
        Cmd::NewPlaylist => new_playlist(v, ids.to_vec(), false),
        Cmd::AddTo(key) => add_to_playlist(v, &key, &ids),
        Cmd::Like => toggle_like(v, &ids),
        Cmd::RemoveFromPlaylist => {
            if let Kind::Track { origin, .. } = &kind
                && let Some((key, places)) = &origin.playlist
            {
                v.s.playlist_remove(key, places);
                v.views.selection = Default::default();
            }
        }
        Cmd::Go(route) => v.go(route),
        Cmd::EditInfo(id) => {
            let hook = hooks().edit_info;
            if let Some(f) = hook {
                f(v, &id);
            }
        }
        Cmd::Swap(id) => {
            if let Some((title, artist)) = v.s.swap_names(v.player, &id) {
                v.views.toast.set(format!("Now “{title}” by {artist}"));
            }
        }
        Cmd::Reveal(path) => reveal(v, &path),
        Cmd::Rename(key) => rename_playlist(v, &key),
        Cmd::Delete(key) => delete_playlist(v, &key),
        Cmd::Picker | Cmd::Back => {}
    }
}
fn show_menu(ctx: &Context, v: &mut View<'_>) {
    let Some(mut menu) = state().menu.take() else {
        return;
    };
    let pressed_outside = ctx.input(|i| {
        i.pointer.any_pressed()
            && i.pointer
                .interact_pos()
                .is_some_and(|p| !menu.rect.contains(p))
    });
    let left = ctx.input(|i| {
        i.raw
            .events
            .iter()
            .any(|e| matches!(e, egui::Event::WindowFocused(false)))
    });
    if (pressed_outside && !menu.fresh) || left {
        return;
    }
    menu.fresh = false;
    let list = entries(v, &menu);
    let measure = ctx.layer_painter(egui::LayerId::background());
    let text_w = |t: &str, f: egui::FontId| measure.layout_no_wrap(t.into(), f, TEXT).size().x;
    let wide = list
        .iter()
        .map(|e| match e {
            Entry::Item { label, key, .. } => {
                62. + text_w(label, font("UI", 13.)) + if key.is_empty() { 0. } else { 34. }
            }
            _ => 0.,
        })
        .fold(220_f32, f32::max)
        .min(320.);
    let screen = ctx.content_rect();
    let inner_h = list
        .iter()
        .map(Entry::height)
        .sum::<f32>()
        .min(screen.height() - 26.);
    let size = vec2(wide, inner_h + 14.);
    let min = pos2(
        menu.at.x.min(screen.right() - size.x - 6.).max(6.),
        menu.at.y.min(screen.bottom() - size.y - 6.).max(6.),
    );
    menu.rect = Rect::from_min_size(min, size);
    let hi = v.tint;
    let mut picked = None;
    Area::new(Id::unique("string menu"))
        .order(Order::Tooltip)
        .fixed_pos(min)
        .show(ctx, |ui| {
            Frame::new()
                .fill(Color32::from_rgb(8, 8, 10))
                .stroke((1., HAIR3))
                .inner_margin(6)
                .shadow(Shadow {
                    offset: [0, 18],
                    blur: 48,
                    spread: 0,
                    color: Color32::from_black_alpha(170),
                })
                .show(ui, |ui| {
                    ui.set_width(wide - 14.);
                    ui.spacing_mut().item_spacing = vec2(0., 0.);
                    egui::ScrollArea::vertical()
                        .max_height(inner_h)
                        .show(ui, |ui| {
                            for (i, e) in list.iter().enumerate() {
                                let w = ui.available_width();
                                let (r, res) =
                                    ui.allocate_exact_size(vec2(w, e.height()), Sense::click());
                                let p = ui.painter();
                                match e {
                                    Entry::Head(text) => {
                                        caps(
                                            &p.with_clip_rect(r),
                                            r.left_center() + vec2(12., 0.),
                                            Align2::LEFT_CENTER,
                                            text,
                                            "HUD",
                                            10.,
                                            0.18,
                                            FAINT,
                                        );
                                    }
                                    Entry::Sep => {
                                        p.hline(r.x_range().shrink(6.), r.center().y, (1., HAIR2));
                                    }
                                    Entry::Item {
                                        label, icon, key, ..
                                    } => {
                                        let hot = res.hovered();
                                        if hot {
                                            p.rect_filled(r, 0, hi.ink(0.1));
                                        }
                                        glyph::draw(
                                            p,
                                            area(r.left() + 12., r.center().y - 7.5, 15., 15.),
                                            icon,
                                            MUTED,
                                        );
                                        let end =
                                            r.right() - if key.is_empty() { 12. } else { 40. };
                                        ui::label(
                                            p,
                                            Rect::from_min_max(
                                                pos2(r.left() + 38., r.top()),
                                                pos2(end, r.bottom()),
                                            ),
                                            label,
                                            "UI",
                                            13.,
                                            if hot { TEXT } else { SECONDARY },
                                        );
                                        if !key.is_empty() {
                                            p.text(
                                                r.right_center() - vec2(12., 0.),
                                                Align2::RIGHT_CENTER,
                                                key,
                                                font("Mono", if *key == "›" { 13. } else { 10.5 }),
                                                FAINT,
                                            );
                                        }
                                        if res.clicked() {
                                            picked = Some(i);
                                        }
                                    }
                                }
                            }
                        });
                });
        });
    let cmd = picked.and_then(|i| match &list[i] {
        Entry::Item { cmd, .. } => Some(cmd.clone()),
        _ => None,
    });
    match cmd {
        Some(Cmd::Picker) => menu.sub = true,
        Some(Cmd::Back) => menu.sub = false,
        Some(cmd) => {
            run(v, menu.kind, cmd);
            return;
        }
        None => {}
    }
    let mut s = state();
    if s.menu.is_none() {
        s.menu = Some(menu);
    }
}

/* ---------- the dialog ---------- */

#[derive(Clone, Copy, PartialEq)]
enum Look {
    Ghost,
    Primary,
    Danger,
}
fn dialog_btn(ui: &mut Ui, text: &str, look: Look, hi: Tint) -> Response {
    let ink = Color32::from_rgb(5, 5, 6);
    let mut job = LayoutJob::simple_singleline(text.to_uppercase(), font("HUD", 11.5), TEXT);
    job.sections[0].format.extra_letter_spacing = 1.6;
    let galley = ui.painter().layout_job(job);
    let (r, res) = ui.allocate_exact_size(vec2(galley.size().x + 34., 36.), Sense::click());
    let p = ui.painter();
    let hot = res.hovered() || res.has_focus();
    let color = match look {
        Look::Ghost => {
            if hot {
                p.rect(
                    r,
                    0,
                    Color32::from_white_alpha(10),
                    (1., HAIR2),
                    StrokeKind::Inside,
                );
            }
            if hot { TEXT } else { MUTED }
        }
        Look::Primary => {
            let (a, b) = (r.min, r.max);
            p.add(Shape::convex_polygon(
                vec![
                    pos2(a.x + 8., a.y),
                    pos2(b.x, a.y),
                    pos2(b.x, b.y - 8.),
                    pos2(b.x - 8., b.y),
                    pos2(a.x, b.y),
                    pos2(a.x, a.y + 8.),
                ],
                if hot { Color32::WHITE } else { hi.ink(1.) },
                Stroke::NONE,
            ));
            ink
        }
        Look::Danger => {
            if hot {
                p.rect_filled(r, 0, Color32::WHITE);
                ink
            } else {
                let stripes = p.with_clip_rect(r);
                let mut x = r.left() - r.height();
                while x < r.right() {
                    stripes.line_segment(
                        [pos2(x, r.bottom()), pos2(x + r.height(), r.top())],
                        (4.2, Color32::from_white_alpha(18)),
                    );
                    x += 17.;
                }
                p.rect_stroke(
                    r,
                    0,
                    (1., Color32::from_white_alpha(128)),
                    StrokeKind::Inside,
                );
                TEXT
            }
        }
    };
    p.galley_with_override_text_color(
        Align2::CENTER_CENTER
            .anchor_size(r.center(), galley.size())
            .min,
        galley,
        color,
    );
    res
}
fn show_dialog(ctx: &Context, v: &mut View<'_>) {
    let Some(mut d) = state().dialog.take() else {
        return;
    };
    let hi = v.tint;
    let width = 412_f32.min(ctx.content_rect().width() * 0.92 - 48.);
    let mut answer = d.answer.take();
    let modal = egui::Modal::new(Id::unique("string ask"))
        .backdrop_color(Color32::from_black_alpha(158))
        .frame(
            Frame::new()
                .fill(Color32::from_rgb(8, 8, 10))
                .stroke((1., HAIR3))
                .inner_margin(egui::Margin::symmetric(24, 22))
                .shadow(Shadow {
                    offset: [0, 24],
                    blur: 70,
                    spread: 0,
                    color: Color32::from_black_alpha(178),
                }),
        )
        .show(ctx, |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing = vec2(8., 0.);
            let (r, _) = ui.allocate_exact_size(vec2(width, 13.), Sense::hover());
            caps(
                ui.painter(),
                r.left_center(),
                Align2::LEFT_CENTER,
                &d.title,
                "HUD",
                13.,
                0.24,
                hi.ink(1.),
            );
            ui.add_space(16.);
            if let Some(value) = &mut d.value {
                let (r, _) = ui.allocate_exact_size(vec2(width, 40.), Sense::hover());
                let id = Id::unique("string ask field");
                let focused = ui.memory(|m| m.has_focus(id));
                ui.painter().rect(
                    r,
                    0,
                    Color32::from_black_alpha(128),
                    (1., if focused { hi.ink(0.6) } else { HAIR2 }),
                    StrokeKind::Inside,
                );
                let field = ui.put(
                    r.shrink2(vec2(12., 6.)),
                    egui::TextEdit::singleline(value)
                        .id(id)
                        .font(font("UI", 13.5))
                        .char_limit(120)
                        .desired_width(f32::INFINITY)
                        .frame(Frame::NONE),
                );
                if d.fresh > 0 {
                    d.fresh -= 1;
                    field.request_focus();
                    if let Some(mut st) = egui::TextEdit::load_state(ui.ctx(), id) {
                        st.cursor.set_char_range(Some(CCursorRange::two(
                            CCursor::new(0),
                            CCursor::new(value.chars().count()),
                        )));
                        egui::TextEdit::store_state(ui.ctx(), id, st);
                        d.fresh = 0;
                    }
                    ui.ctx().request_repaint();
                }
            } else {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(&d.text)
                            .font(font("UI", 13.5))
                            .color(SECONDARY),
                    )
                    .wrap(),
                );
            }
            ui.add_space(20.);
            ui.allocate_ui_with_layout(
                vec2(width, 36.),
                Layout::right_to_left(Align::Center),
                |ui| {
                    ui.spacing_mut().item_spacing.x = 10.;
                    let look = if d.danger {
                        Look::Danger
                    } else {
                        Look::Primary
                    };
                    if dialog_btn(ui, d.ok, look, hi).clicked() {
                        answer = Some(true);
                    }
                    if dialog_btn(ui, "Cancel", Look::Ghost, hi).clicked() {
                        answer = Some(false);
                    }
                },
            );
        });
    if answer.is_none() && modal.should_close() {
        answer = Some(false);
    }
    match answer {
        None => {
            let mut s = state();
            if s.dialog.is_none() {
                s.dialog = Some(d);
            }
        }
        Some(false) => {}
        Some(true) => {
            let text = d.value.as_deref().unwrap_or("").trim().to_owned();
            match d.purpose {
                _ if d.value.is_some() && text.is_empty() => {}
                Purpose::NewPlaylist { ids, open } => {
                    let id = v.s.new_playlist(&text, &ids);
                    if !ids.is_empty() {
                        v.views
                            .toast
                            .set(format!("{} added to {text}", plural(ids.len(), "song")));
                    }
                    if open {
                        v.go(Route::Playlist(id));
                    }
                }
                Purpose::Rename(key) => v.s.playlist_rename(&key, &text),
                Purpose::Delete(key) => {
                    if let Some(name) = v.s.playlist_delete(&key) {
                        v.views.toast.set(format!("Deleted {name}"));
                        if v.views.history.current == Route::Playlist(key) {
                            v.go(Route::Home);
                        }
                    }
                }
            }
        }
    }
}

/* ---------- drawing helpers shared with the drawer and the playlist page ---------- */

/// Upper-case HUD lettering with wide spacing (`em` of the size). Returns where it was drawn.
#[allow(clippy::too_many_arguments)]
pub fn caps(
    p: &Painter,
    pos: Pos2,
    anchor: Align2,
    text: &str,
    face: &str,
    size: f32,
    em: f32,
    color: Color32,
) -> Rect {
    let mut job = LayoutJob::simple_singleline(text.to_uppercase(), font(face, size), color);
    job.sections[0].format.extra_letter_spacing = size * em;
    let galley = p.layout_job(job);
    let rect = anchor.anchor_size(pos, galley.size());
    p.galley(rect.min, galley, color);
    rect
}
/// The dashed note shown where a list is empty. Returns the height used.
pub fn empty_note(ui: &mut Ui, text: &str) -> f32 {
    let w = ui.available_width();
    let mut job = LayoutJob::simple(
        text.to_uppercase(),
        font("Mono", 11.),
        FAINT,
        (w - 36.).max(40.),
    );
    job.halign = Align::Center;
    job.sections[0].format.extra_letter_spacing = 0.9;
    job.sections[0].format.line_height = Some(17.6);
    let galley = ui.painter().layout_job(job);
    let h = galley.size().y + 36.;
    let (r, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let p = ui.painter();
    let c = [
        r.left_top(),
        r.right_top(),
        r.right_bottom(),
        r.left_bottom(),
        r.left_top(),
    ];
    p.extend(Shape::dashed_line(&c, (1., HAIR2), 4., 3.));
    p.galley(pos2(r.center().x, r.top() + 18.), galley, FAINT);
    h
}
/// The four little level bars beside the song that plays.
pub fn eq_bars(p: &Painter, r: Rect, playing: bool, time: f64, hi: Tint) {
    let base = pos2(r.center().x - 9., r.center().y + 7.);
    for (i, (period, shift)) in [(1., 0.), (0.8, 0.3), (1.15, 0.6), (0.9, 0.15)]
        .into_iter()
        .enumerate()
    {
        let phase = if playing {
            time / period + shift
        } else {
            shift
        };
        let level = 0.25 + 0.75 * (0.5 - 0.5 * (phase * std::f64::consts::TAU).cos()) as f32;
        let x = base.x + i as f32 * 5.;
        p.rect_filled(
            Rect::from_min_max(pos2(x, base.y - 14. * level), pos2(x + 3., base.y)),
            0,
            hi.ink(1.),
        );
    }
}

/* ---------- once per frame ---------- */

/// Called first in the key handling: true while a dialog or a menu has the keyboard.
pub fn takes_keys(ctx: &Context) -> bool {
    let mut s = state();
    if let Some(d) = &mut s.dialog {
        // Taken here so the page underneath never sees the key.
        ctx.input_mut(|i| {
            if i.consume_key(Modifiers::NONE, Key::Enter) {
                d.answer = Some(true);
            } else if i.consume_key(Modifiers::NONE, Key::Escape) {
                d.answer = Some(false);
            }
        });
        return true;
    }
    if s.menu.is_some() && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
        s.menu = None;
        return true;
    }
    false
}
const AUDIO: [&str; 11] = [
    "mp3", "flac", "m4a", "m4b", "aac", "ogg", "oga", "opus", "wav", "weba", "webm",
];
fn drops(ctx: &Context, v: &mut View<'_>) {
    let hi = v.tint;
    let (hovering, dropped) = ctx.input(|i| {
        (
            !i.raw.hovered_files.is_empty(),
            i.raw
                .dropped_files
                .iter()
                .map(|d| d.path().to_owned())
                .collect::<Vec<_>>(),
        )
    });
    if hovering || state().drop_demo {
        let r = ctx.content_rect().shrink(10.);
        let p = ctx.layer_painter(egui::LayerId::new(Order::Tooltip, Id::unique("dropzone")));
        p.rect_filled(r, 0, Color32::from_black_alpha(184));
        let c = [
            r.left_top(),
            r.right_top(),
            r.right_bottom(),
            r.left_bottom(),
            r.left_top(),
        ];
        p.extend(Shape::dashed_line(&c, (1., hi.ink(0.7)), 5., 4.));
        caps(
            &p,
            r.center(),
            Align2::CENTER_CENTER,
            "Drop folders or songs to add them",
            "HUD",
            14.,
            0.3,
            hi.ink(1.),
        );
    }
    if dropped.is_empty() {
        return;
    }
    let mut added = 0;
    for path in dropped {
        let song = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| AUDIO.contains(&e.to_ascii_lowercase().as_str()));
        if path.is_dir() || (path.is_file() && song) {
            v.add_path(path);
            added += 1;
        }
    }
    if added == 0 {
        v.views.toast.set("Those aren't songs String can play");
    }
}
/// The label that follows the pointer while songs are dragged.
pub fn ghost(ctx: &Context, at: Pos2, text: String, hi: Tint) {
    let p = ctx.layer_painter(egui::LayerId::new(Order::Tooltip, Id::unique("drag ghost")));
    let galley = p.layout_no_wrap(text, font("Strong", 12.), Color32::WHITE);
    let r = Rect::from_min_size(at + vec2(12., 14.), galley.size() + vec2(24., 16.));
    p.rect(r, 0, Color32::BLACK, (1., hi.ink(0.6)), StrokeKind::Inside);
    p.galley(r.min + vec2(12., 8.), galley, Color32::WHITE);
}
pub fn drag_text(m: &Models, ids: &[String]) -> String {
    match ids {
        [id] => title_of(m, id),
        _ => plural(ids.len(), "song"),
    }
}
fn drag_ghost(ctx: &Context, v: &View<'_>) {
    if let Some(ids) = egui::DragAndDrop::payload::<Ids>(ctx)
        && let Some(at) = ctx.pointer_latest_pos()
    {
        ghost(ctx, at, drag_text(&v.s.models, &ids), v.tint);
        ctx.request_repaint();
    }
}
/// Draw the open menu and dialog, the drag label and the file drop zone. Call after the screens.
pub fn overlay(ui: &mut Ui, v: &mut View<'_>) {
    let ctx = ui.ctx().clone();
    if std::mem::take(&mut v.views.adding_playlist) {
        // The Now Playing "+" asks for a playlist holding the current song.
        let ids =
            v.s.state
                .queue
                .current()
                .filter(|_| v.views.overlay.is_some())
                .map(str::to_owned)
                .into_iter()
                .collect();
        new_playlist(v, ids, false);
    }
    drops(&ctx, v);
    drag_ghost(&ctx, v);
    show_menu(&ctx, v);
    show_dialog(&ctx, v);
}

/* ---------- smoke cases ---------- */

pub mod smoke {
    use super::*;
    use crate::ui::covers::Size;
    fn covers_ready(v: &mut View<'_>) -> bool {
        // Only the covers a screen can show at once: the cache rightly keeps far fewer than a big library.
        let ids = v.s.models.songs.clone();
        ids.iter().take(24).all(|id| {
            v.covers
                .get(
                    &v.s.models.by_id[id].cover,
                    Size::Small,
                    v.mono.then_some(v.tint),
                )
                .is_some()
        })
    }
    /// Two playlists, a liked song and a queue with two songs added by hand.
    fn seed(v: &mut View<'_>) {
        let songs = v.s.models.songs.clone();
        if songs.is_empty() || !v.s.models.playlists.is_empty() {
            return;
        }
        let pick = |n: usize| songs[n % songs.len()].clone();
        v.s.new_playlist("Smoke mix", &[pick(5), pick(0), pick(2), pick(6)]);
        let many: Vec<_> = songs.iter().take(7).cloned().collect();
        v.s.new_playlist("Long drive", &many);
        v.s.new_playlist("Empty for now", &[]);
        v.s.toggle_like(&[pick(5)]);
    }
    fn first_list(v: &View<'_>) -> Option<String> {
        v.s.models.playlists.first().map(|p| p.id.clone())
    }
    pub fn queue(v: &mut View<'_>) {
        *state() = State {
            menu: None,
            dialog: None,
            drop_demo: false,
        };
        seed(v);
        v.go(Route::Songs);
        let songs = v.s.models.songs.clone();
        if songs.is_empty() {
            return;
        }
        v.s.play_ids(
            v.player,
            &songs,
            1,
            json!({"kind":"songs","key":"","name":"All songs"}),
        );
        v.s.state.queue.clear_manual();
        v.s.enqueue(v.player, &[songs[0].clone()], true);
        v.s.enqueue(v.player, &[songs[songs.len() - 1].clone()], true);
        crate::ui::queue_drawer::set_tab(false);
        v.views.queue = true;
    }
    pub fn recent(v: &mut View<'_>) {
        crate::ui::queue_drawer::set_tab(true);
        v.views.queue = true;
    }
    pub fn menu(v: &mut View<'_>) {
        crate::ui::queue_drawer::set_tab(false);
        v.go(Route::Songs);
        let ids = v.s.models.songs.clone();
        if let Some(id) = ids.get(2) {
            v.views.selection.choose(2, false, false);
            track_menu(
                vec![id.clone()],
                pos2(566., 300.),
                Origin::list(ids.clone(), 2, v.context()),
            );
        }
    }
    pub fn playlist(v: &mut View<'_>) {
        close_menu();
        if let Some(id) = first_list(v) {
            v.go(Route::Playlist(id));
            v.views.selection.choose(1, false, false);
        }
    }
    pub fn dropping(v: &mut View<'_>) {
        let picked: Ids =
            v.s.models
                .playlists
                .first()
                .map(|p| p.ids.iter().skip(1).take(2).cloned().collect())
                .unwrap_or_default();
        if let Some(id) = v.s.models.playlists.get(1).map(|p| p.id.clone()) {
            crate::ui::side_playlists::demo_drop(Some((id, picked)));
        }
        v.views.selection.indices.extend([1, 2]);
    }
    pub fn list_menu(v: &mut View<'_>) {
        crate::ui::side_playlists::demo_drop(None);
        if let Some(id) = first_list(v) {
            ctx_menu(json!({"kind":"pl","key":id}), pos2(60., 380.), None);
        }
    }
    pub fn dialog(v: &mut View<'_>) {
        close_menu();
        if let Some(id) = first_list(v) {
            delete_playlist(v, &id);
        }
    }
    /// The playlist filtered to two songs, with the menu for both.
    pub fn filtered(v: &mut View<'_>) {
        state().dialog = None;
        let Some(id) = first_list(v) else {
            return;
        };
        // Filter by a word from the playlist's own first song, so any test library works.
        let entries = v.s.playlist_entries(&id);
        let word = entries
            .iter()
            .find_map(|e| v.s.models.by_id.get(&e.1))
            .and_then(|s| s.title.split_whitespace().last().map(str::to_lowercase))
            .unwrap_or_else(|| "song".into());
        v.views.filter = word.clone();
        let shown: Vec<_> = entries
            .into_iter()
            .filter(|e| {
                v.s.models
                    .by_id
                    .get(&e.1)
                    .is_some_and(|s| s.title.to_lowercase().contains(&word))
            })
            .collect();
        let ids: Ids = shown.iter().map(|e| e.1.clone()).collect();
        track_menu(
            ids.clone(),
            pos2(700., 500.),
            Origin {
                list: Some((ids, 0)),
                ctx: Some(json!({"kind":"pl","key":id})),
                playlist: Some((id, shown.iter().map(|e| e.0).collect())),
            },
        );
    }
    pub fn picker(v: &mut View<'_>) {
        v.views.filter.clear();
        let ids = v.s.models.songs.clone();
        playlist_picker(ids, pos2(700., 420.));
    }
    pub fn dropzone(v: &mut View<'_>) {
        let mut s = state();
        s.menu = None;
        s.dialog = None;
        s.drop_demo = true;
        drop(s);
        v.go(Route::Songs);
    }
    /// Leave nothing open for the cases that follow.
    pub fn reset(_: &mut View<'_>) {
        let mut s = state();
        s.menu = None;
        s.dialog = None;
        s.drop_demo = false;
    }
    pub fn ready(v: &mut View<'_>) -> bool {
        !v.scan.running() && v.scan.completed > 0 && covers_ready(v)
    }
    pub fn queue_ready(v: &mut View<'_>) -> bool {
        ready(v)
            && v.st.playing
            && v.st.position > 0.3
            && v.s.state.queue.upcoming().iter().any(|x| x.manual)
    }
    pub fn recent_ready(v: &mut View<'_>) -> bool {
        ready(v) && v.views.queue
    }
    pub fn menu_ready(v: &mut View<'_>) -> bool {
        ready(v) && state().menu.is_some()
    }
    pub fn playlist_ready(v: &mut View<'_>) -> bool {
        let Route::Playlist(key) = v.views.history.current.clone() else {
            return false;
        };
        let ids = v.s.models.context_ids(&json!({"kind":"pl","key":key}));
        let tint = v.mono.then_some(v.tint);
        let lead = crate::ui::side_playlists::cover_picks(&v.s.models, &ids)
            .into_iter()
            .next();
        ready(v)
            && lead.is_none_or(|c| {
                v.covers.get(&c, Size::Medium, tint).is_some()
                    && v.covers.get(&c, Size::Large, tint).is_some()
            })
    }
    pub fn dialog_ready(v: &mut View<'_>) -> bool {
        ready(v) && state().dialog.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use string_core::{library::Library, state::State, stats::Listening};
    fn order(s: &Session, key: &str) -> String {
        s.playlist_entries(key)
            .iter()
            .map(|e| e.1.as_str())
            .collect()
    }
    #[test]
    fn playlists_add_once_move_remove_rename_and_delete() {
        let mut s = Session::new(State::default(), Listening::default(), Library::default());
        let ids: Vec<String> = "abcde".chars().map(String::from).collect();
        let key = s.new_playlist("Mix", &ids);
        assert_eq!(
            s.playlist_add(&key, &["c".into(), "f".into(), "f".into()]),
            ["f"]
        );
        assert_eq!(order(&s, &key), "abcdef");
        // b and d go before f; then a goes to the end; then e to the front.
        s.playlist_move(&key, &[1, 3], 5);
        assert_eq!(order(&s, &key), "acebdf");
        s.playlist_move(&key, &[0], 99);
        assert_eq!(order(&s, &key), "cebdfa");
        s.playlist_move(&key, &[1], 0);
        assert_eq!(order(&s, &key), "ecbdfa");
        s.playlist_remove(&key, &[0, 5, 9]);
        assert_eq!(order(&s, &key), "cbdf");
        s.playlist_rename(&key, "Late");
        assert_eq!(s.field("playlists")[0]["name"], "Late");
        assert_eq!(s.playlist_delete(&key).as_deref(), Some("Late"));
        assert!(s.playlist_entries(&key).is_empty() && s.state_dirty);
        assert!(s.playlist_add(&key, &ids).is_empty());
    }
    #[test]
    fn old_playlists_with_bare_ids_still_read_and_grow() {
        let mut s = Session::new(State::default(), Listening::default(), Library::default());
        s.set_field(
            "playlists",
            json!([{"id":"p","name":"Old","items":["a",{"id":"b","at":5}]}]),
        );
        assert_eq!(s.playlist_add("p", &["a".into(), "c".into()]), ["c"]);
        assert_eq!(order(&s, "p"), "abc");
        assert_eq!(s.playlist_entries("p")[1].2, 5);
    }
    #[test]
    fn words_match_the_page() {
        assert_eq!(plural(1, "song"), "1 song");
        assert_eq!(plural(12345, "song"), "12,345 songs");
        assert_eq!(long(22.4), "22 sec");
        assert_eq!(long(107.), "1 min 47 sec");
        assert_eq!(long(7500.), "2 hr 5 min");
        assert_eq!(when(0), "");
        assert_eq!(when(chrono::Local::now().timestamp_millis()), "Today");
        let c = json!({"kind":"search","key":"sun"});
        assert_eq!(ctx_name(&Models::default(), &c), "“sun”");
        assert!(ctx_route(&c).is_none());
        assert_eq!(
            ctx_route(&json!({"kind":"pl","key":"x"})),
            Some(Route::Playlist("x".into()))
        );
        assert!(same_ctx(
            &json!({"kind":"liked"}),
            &json!({"kind":"liked","key":""})
        ));
        assert!(!same_ctx(&Value::Null, &Value::Null));
    }
}
