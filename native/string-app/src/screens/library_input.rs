use crate::{route::Route, view::View};
use egui::{Context, Event, Id, Key, Modifiers, PointerButton, Pos2, RawInput, pos2};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

type Frames = Arc<Mutex<VecDeque<Vec<Event>>>>;
struct Input(Frames);
impl egui::Plugin for Input {
    fn debug_name(&self) -> &'static str {
        "Library smoke input"
    }
    fn input_hook(&mut self, _: &Context, raw: &mut RawInput) {
        if let Some(mut events) = self.0.lock().unwrap().pop_front() {
            raw.focused = true;
            raw.events.append(&mut events);
        }
    }
}

#[derive(Default)]
pub struct Probe {
    frames: Frames,
    step: usize,
    term: String,
    original_sort: serde_json::Value,
    original_album_sort: serde_json::Value,
    selected_track: Option<String>,
}
impl Probe {
    fn send(&self, frames: impl IntoIterator<Item = Vec<Event>>) {
        self.frames.lock().unwrap().extend(frames);
    }
    fn click(&self, p: Pos2) {
        let button = |pressed| Event::PointerButton {
            pos: p,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        self.send([
            vec![Event::PointerMoved(p)],
            vec![],
            vec![button(true)],
            vec![button(false)],
        ]);
    }
    fn key(&self, key: Key) {
        let event = |pressed| Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        self.send([vec![event(true)], vec![event(false)]]);
    }
    pub fn tick(&mut self, ctx: &Context, v: &mut View<'_>) -> Result<bool, String> {
        if !self.frames.lock().unwrap().is_empty() {
            return Ok(false);
        }
        let width = ctx.viewport_rect().width();
        let click_response = |id| {
            ctx.read_response(id)
                .map(|r| self.click(r.rect.center()))
                .ok_or_else(|| format!("Library smoke could not find widget in step {}", self.step))
        };
        match self.step {
            0 => {
                ctx.add_plugin(Input(self.frames.clone()));
                self.original_sort = v.s.field("sort").clone();
                self.original_album_sort = v.s.field("albumSort").clone();
                self.term = v.s.models.by_id[&v.s.models.songs[0]].title.clone();
                v.player.pause();
                v.go(Route::Songs);
            }
            1 => click_response(Id::unique(("library filter", "Filter songs")))?,
            2 => self.send([vec![Event::Text(self.term.clone())]]),
            3 => {
                if v.views.filter != self.term || !ctx.text_edit_focused() {
                    return Err("Song filter lost its input or focus during redraw".into());
                }
                self.send([vec![Event::Text(" unmatched".into())]]);
            }
            4 => {
                if v.views.filter != format!("{} unmatched", self.term) {
                    return Err("Song filter lost the second edit".into());
                }
                self.key(Key::Escape);
            }
            5 => {
                if !v.views.filter.is_empty() {
                    return Err("Escape did not clear the filter".into());
                }
                self.click(pos2(width - 102., 199.));
            }
            6 => {
                if v.s.field("sort")["key"] != "duration" {
                    return Err("Time column did not sort Songs".into());
                }
                self.click(pos2(width - 102., 199.));
            }
            7 => {
                if self.original_sort["key"] != "duration" && v.s.field("sort")["dir"] != -1 {
                    return Err("Second Time click did not reverse the sort".into());
                }
                v.go(Route::Albums);
            }
            8 => self.click(pos2(width - 60., 87.)),
            9 => {
                if v.s.field("albumSort") != "year" {
                    return Err("Year did not change the album order".into());
                }
                if v.s.models.catalog.albums.is_empty() {
                    self.step = 13;
                    return Ok(false);
                }
                self.click(Self::first_card_play(width));
            }
            10 => {
                if !v.st.playing {
                    return Ok(false);
                }
                if v.views.history.current != Route::Albums
                    || v.s.state.queue.context(None)["kind"] != "album"
                {
                    return Err("Card play opened the page or lost its album context".into());
                }
                if v.s.models.catalog.albums.first().map(|g| g.key.as_str())
                    != v.s.state.queue.context(None)["key"].as_str()
                {
                    return Err("The first album card disagrees with the playback order".into());
                }
                self.selected_track = v.s.state.queue.current().map(str::to_owned);
                self.click(Self::first_card_play(width));
            }
            11 => {
                if v.st.playing {
                    return Ok(false);
                }
                if v.s.state.queue.current() != self.selected_track.as_deref() {
                    return Err("Pause changed the active card's track".into());
                }
                self.click(Self::first_card_play(width));
            }
            12 => {
                if !v.st.playing {
                    return Ok(false);
                }
                if v.s.state.queue.current() != self.selected_track.as_deref() {
                    return Err("Resume restarted another song".into());
                }
            }
            13 => click_response(Id::unique("global search"))?,
            14 => self.send([vec![Event::Text(self.term.clone())]]),
            15 => {
                if v.views.history.current != Route::Search || v.views.query != self.term {
                    return Err("Typing in the search field did not open results".into());
                }
                self.key(Key::Enter);
            }
            16 => {
                if v.s.state.queue.context(None)["kind"] != "search"
                    || v.s.state.queue.current()
                        != v.s.models.search(&self.term).first().map(String::as_str)
                {
                    return Err("Enter did not play the first search result".into());
                }
                super::albums::set_sort(v, self.original_album_sort.clone());
                v.s.set_field("sort", self.original_sort.clone());
                v.s.models.sort_songs(&v.s.state);
                v.s.generation += 1;
                self.send([vec![Event::PointerGone]]);
                return Ok(true);
            }
            _ => unreachable!(),
        }
        self.step += 1;
        Ok(false)
    }
    fn first_card_play(width: f32) -> Pos2 {
        let grid = width - 236. - 58.;
        let columns = ((grid + 18.) / 186.).floor().max(1.);
        let cell = (grid - (columns - 1.) * 18.) / columns;
        pos2(265. + cell - 44., 123. + cell - 44.)
    }
    pub fn step(&self) -> usize {
        self.step
    }
}
