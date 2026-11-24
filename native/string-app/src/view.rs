use crate::{
    model::Ids,
    playback::Session,
    route::{History, Route},
    scan::Scanner,
    theme::Tint,
    ui::{Toast, covers::CoverCache, songs::Selection},
    viz::Viz,
};
use serde_json::{Value, json};
use string_audio::{Player, Status};
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Overlay {
    Record,
    Lyrics,
}
#[derive(Default)]
pub struct Views {
    pub lyrics: Option<crate::screens::lyrics::LyricsView>,
    pub stats: crate::screens::stats::StatsView,
    pub history: History,
    pub overlay: Option<Overlay>,
    pub queue: bool,
    pub query: String,
    pub search_focus: bool,
    pub selection: Selection,
    pub artist_popular_selection: Selection,
    pub toast: Toast,
    /// Dropped files on their way to Stats or the library.
    pub drops: crate::integration::spotify::Drops,
    pub adding_playlist: bool,
    pub playlist_name: String,
    pub root_input: String,
    pub filter: String,
    pub reveal_id: Option<String>,
    pub cache: Option<ListCache>,
    pub close_failed: bool,
}
pub struct ListCache {
    pub route: Route,
    pub generation: u64,
    pub query: String,
    pub filter: String,
    pub ids: Ids,
}
pub struct View<'a> {
    pub(crate) adds: &'a crate::add_work::Input,
    pub platform: &'a mut crate::platform::Platform,
    pub s: &'a mut Session,
    pub player: &'a Player,
    pub st: Status,
    pub views: &'a mut Views,
    pub covers: &'a mut CoverCache,
    pub viz: &'a Viz,
    pub scan: &'a mut Scanner,
    pub tint: Tint,
    pub mono: bool,
    pub force_mute: bool,
}
impl View<'_> {
    pub fn go(&mut self, r: Route) {
        self.views.history.go(r);
        self.route_changed();
    }
    pub fn route_changed(&mut self) {
        self.views.overlay = None;
        self.views.queue = false;
        self.views.selection = Default::default();
        self.views.artist_popular_selection = Default::default();
        self.views.filter.clear();
        self.views.cache = None;
        self.s
            .set_field("route", self.views.history.current.path().into());
    }
    pub fn context(&self) -> Value {
        let (kind, key, name) = match &self.views.history.current {
            Route::Liked => ("liked", "", "Liked songs"),
            Route::Album(k) => (
                "album",
                k.as_str(),
                self.s
                    .models
                    .catalog
                    .albums
                    .iter()
                    .find(|g| &g.key == k)
                    .map_or("Album", |g| &g.name),
            ),
            Route::Artist(k) => (
                "artist",
                k.as_str(),
                self.s
                    .models
                    .catalog
                    .artists
                    .iter()
                    .find(|g| &g.key == k)
                    .map_or("Artist", |g| &g.name),
            ),
            Route::Playlist(k) => (
                "pl",
                k.as_str(),
                self.s
                    .models
                    .playlists
                    .iter()
                    .find(|p| &p.id == k)
                    .map_or("Playlist", |p| &p.name),
            ),
            Route::Search => ("search", self.views.query.as_str(), "Search"),
            _ => ("songs", "", "All songs"),
        };
        json!({"kind":kind,"key":key,"name":name})
    }
    pub fn record(&mut self, mode: Overlay) {
        self.views.overlay = if self.views.overlay == Some(mode) {
            None
        } else {
            Some(mode)
        };
        self.views.queue = false;
    }
    pub fn jump_current(&mut self) {
        if let Some(id) = self.s.state.queue.current().map(str::to_owned) {
            self.go(Route::Songs);
            self.views.reveal_id = Some(id);
        }
    }
    pub fn jump(&mut self, i: usize) {
        if self.s.state.queue.jump(i, &self.s.models.catalog) {
            self.s.state_dirty = true;
            self.s.load_current(self.player, true, 0.);
        }
    }
    pub fn like_current(&mut self) {
        if let Some(id) = self.s.state.queue.current().map(str::to_owned) {
            self.s.toggle_like(&[id]);
        }
    }
    pub fn volume(&mut self, n: f32) {
        let n = n.clamp(0., 1.);
        self.s.set_field("vol", n.into());
        if n > 0. {
            self.s.set_field("muted", false.into());
        }
        self.player.set_volume(n);
        self.player.set_muted(self.force_mute || self.s.muted());
    }
    pub fn mute(&mut self) {
        let value = !self.s.muted();
        self.s.set_field("muted", value.into());
        self.player.set_muted(value || self.force_mute);
    }
    pub fn shuffle(&mut self) {
        let on = !self.s.state.queue.shuffled();
        self.s.state.queue.set_shuffle(on, &self.s.models.catalog);
        self.s.queue_changed(self.player);
    }
    pub fn repeat(&mut self) {
        self.s.state.queue.cycle_repeat();
        self.s.queue_changed(self.player);
    }
    pub fn add_path(&mut self, path: std::path::PathBuf) {
        // Checked on the add worker, then added with the pickers' rules (add_work::Adds::poll).
        if !self.adds.window_path(path) {
            self.views.toast.set("String could not take more songs.");
        }
    }
}
