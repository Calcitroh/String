use crate::model::{Ids, Models, Song};
use chrono::{Local, NaiveDate};
use egui::Context;
use serde_json::{Value, json};
use std::{
    collections::{HashSet, VecDeque},
    sync::{Arc, Mutex},
    time::Instant,
};
use string_audio::{Config, Event, Host, LoopRange, Player, Status, Track};
use string_core::{
    data::{Document, SpotifyHistory},
    library::Library,
    queue::{AdvancePlan, NextAction, PlaybackOptions},
    state::State,
    stats::{Listening, Track as CountedTrack},
};
use string_extras::spotify::{Matcher, Resolved};

/// Queue controls used by both the live player and the in-memory playback tests.
pub trait Transport {
    fn status(&self) -> Status;
    fn play_track(&self, track: Track, from: f64);
    fn cue(&self, track: Track, from: f64);
    fn stop(&self);
    fn play(&self);
    fn toggle(&self);
    fn rewind_paused(&self);
    fn seek(&self, position: f64);
    fn seek_track(&self, key: u64, position: f64);
}

impl<T: std::borrow::Borrow<Player>> Transport for T {
    fn status(&self) -> Status {
        Player::status(self.borrow())
    }
    fn play_track(&self, track: Track, from: f64) {
        Player::play_track(self.borrow(), track, from)
    }
    fn cue(&self, track: Track, from: f64) {
        Player::cue(self.borrow(), track, from)
    }
    fn stop(&self) {
        Player::stop(self.borrow())
    }
    fn play(&self) {
        Player::play(self.borrow())
    }
    fn toggle(&self) {
        Player::toggle(self.borrow())
    }
    fn rewind_paused(&self) {
        Player::rewind_paused(self.borrow())
    }
    fn seek(&self, position: f64) {
        Player::seek(self.borrow(), position)
    }
    fn seek_track(&self, key: u64, position: f64) {
        Player::seek_track(self.borrow(), key, position)
    }
}

pub type Shared = Arc<Mutex<Session>>;
pub struct Session {
    pub state: State,
    pub listening: Listening,
    pub library: Library,
    pub models: Models,
    pub state_dirty: bool,
    pub listening_dirty: bool,
    pub library_dirty: bool,
    pub notices: VecDeque<String>,
    pub generation: u64,
    pub plays_revision: u64,
    pub(crate) seek_revision: u64,
    pub(crate) seek_position: f64,
    pub(crate) import_listens: Option<Vec<crate::integration::phone::Heard>>,
    active: u64,
    active_failed: bool,
    sequence: u64,
    pending: Option<(u64, AdvancePlan)>,
    superseded: Option<u64>,
    failed: HashSet<String>,
    counted: bool,
    last_tick: Instant,
    was_playing: bool,
    last_key: Option<u64>,
    last_position: f64,
    last_loop_offset: f64,
    active_duration: Option<f64>,
    pub resync: bool,
    /// How many times song files were moved to the Recycle Bin: a scan that started before the last move found them
    /// still there, so its result is set aside and the scan runs again.
    pub removals: u64,
    /// The imported Spotify history (`spotify.json`), or why it couldn't be read. Change both with `set_spotify`.
    pub spotify: Option<Document<SpotifyHistory>>,
    pub spotify_error: Option<String>,
    /// The history matched to the current library: only Stats reads it, never `listening`.
    spotify_view: Option<(Listening, Resolved)>,
    /// Changes whenever `spotify_view` does, so Stats draws again.
    pub spotify_revision: u64,
}
/// What the Stats panel says about the imported Spotify history.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpotifySummary {
    pub first: NaiveDate,
    pub last: NaiveDate,
    pub seconds: f64,
    /// The part of `seconds` that is songs in the library.
    pub matched_share: f64,
    pub plays: u64,
}
impl Session {
    pub fn new(mut state: State, listening: Listening, library: Library) -> Self {
        let models = Models::build(&library, &state);
        let legacy = state.queue.needs_legacy_order();
        if legacy {
            let ids = models.context_ids(state.queue.context(None));
            state.queue.restore_legacy_order(ids.to_vec());
        }
        Self {
            state,
            listening,
            library,
            models,
            state_dirty: legacy,
            listening_dirty: false,
            library_dirty: false,
            notices: VecDeque::new(),
            generation: 0,
            plays_revision: 0,
            seek_revision: 0,
            seek_position: 0.,
            import_listens: None,
            active: 0,
            active_failed: false,
            sequence: 0,
            pending: None,
            superseded: None,
            failed: HashSet::new(),
            counted: false,
            last_tick: Instant::now(),
            was_playing: false,
            last_key: None,
            last_position: 0.,
            last_loop_offset: 0.,
            active_duration: None,
            resync: false,
            removals: 0,
            spotify: None,
            spotify_error: None,
            spotify_view: None,
            spotify_revision: 0,
        }
    }
    pub fn rebuild(&mut self) {
        self.models = Models::build(&self.library, &self.state);
        for id in &self.failed {
            if let Some(s) = self.models.catalog.tracks.get_mut(id) {
                s.unplayable = true;
            }
        }
        self.invalidate_plan();
        self.generation += 1;
        self.resolve_spotify();
    }
    pub fn set_spotify(&mut self, doc: Option<Document<SpotifyHistory>>, error: Option<String>) {
        self.spotify = doc;
        self.spotify_error = error;
        self.resolve_spotify();
    }
    fn resolve_spotify(&mut self) {
        let history = self
            .spotify
            .as_ref()
            .map(|doc| &doc.value.0)
            .filter(|h| h.first_day().is_some());
        if history.is_none() && self.spotify_view.is_none() {
            return;
        }
        // Library order, hidden songs too, under the names String's own plays record, so a matched Spotify play
        // adds to the person's own plays of that song.
        let songs = self.library.tracks.iter().filter_map(|t| {
            let s = self.models.by_id.get(&t.id())?;
            Some(CountedTrack(
                s.id.clone(),
                s.title.clone(),
                s.artist.clone(),
            ))
        });
        self.spotify_view = history.map(|h| Matcher::new(songs).resolve(h));
        self.spotify_revision += 1;
    }
    pub fn spotify_view(&self) -> Option<&Listening> {
        self.spotify_view.as_ref().map(|(view, _)| view)
    }
    pub fn spotify_summary(&self) -> Option<SpotifySummary> {
        let (view, resolved) = self.spotify_view.as_ref()?;
        let mut heard = view
            .days()
            .iter()
            .filter(|(_, d)| d.s > 0.)
            .map(|(&day, _)| day);
        let first = heard.next()?;
        Some(SpotifySummary {
            first,
            last: heard.next_back().unwrap_or(first),
            seconds: resolved.seconds,
            matched_share: if resolved.seconds > 0. {
                resolved.matched_seconds / resolved.seconds
            } else {
                0.
            },
            plays: self
                .spotify
                .as_ref()
                .map_or(0, |doc| doc.value.meta().plays),
        })
    }
    pub fn current(&self) -> Option<&Song> {
        if self.active_failed {
            return None;
        }
        self.models.by_id.get(self.state.queue.current()?)
    }
    pub fn bar_song(&self, status: Status) -> Option<&Song> {
        (status.key == Some(self.active))
            .then(|| self.current())
            .flatten()
    }
    pub fn setting(&self, k: &str) -> &Value {
        self.state
            .fields
            .get("set")
            .map(|v| &v[k])
            .unwrap_or(&Value::Null)
    }
    pub fn field(&self, k: &str) -> &Value {
        self.state.fields.get(k).unwrap_or(&Value::Null)
    }
    pub fn set_setting(&mut self, k: &str, v: Value) {
        let set = self.state.fields.entry("set").or_insert_with(|| json!({}));
        if !set.is_object() {
            *set = json!({})
        }
        set[k] = v;
        self.state_dirty = true;
    }
    pub fn set_field(&mut self, k: &str, v: Value) {
        if k == "plays" {
            self.plays_revision += 1;
        }
        self.state.fields.insert(k.into(), v);
        self.state_dirty = true;
    }
    pub fn volume(&self) -> f32 {
        self.field("vol").as_f64().unwrap_or(0.8).clamp(0., 1.) as f32
    }
    pub fn muted(&self) -> bool {
        self.field("muted").as_bool().unwrap_or(false)
    }
    pub fn audible_volume(&self) -> f32 {
        if self.muted() { 0. } else { self.volume() }
    }
    pub fn loop_for(&self, id: &str) -> Option<LoopRange> {
        let duration = if self.state.queue.current() == Some(id) {
            self.active_duration
        } else {
            None
        }
        .or_else(|| self.models.by_id.get(id).map(|s| s.duration))
        .filter(|d| *d > 0.);
        self.saved_loop(id, duration)
    }
    fn saved_loop(&self, id: &str, duration: Option<f64>) -> Option<LoopRange> {
        let lp = &self.field("loops")[id];
        let (a, b) = (lp["a"].as_f64(), lp["b"].as_f64());
        if a.is_none() && b.is_none() {
            return None;
        }
        let limit = duration.unwrap_or(f64::INFINITY);
        let start = a.unwrap_or(0.).clamp(0., limit);
        let end = b.map(|t| t.clamp(0., limit));
        (end.unwrap_or(limit) - start >= 1.).then_some(LoopRange { start, end })
    }
    fn skip_options(&self) -> PlaybackOptions {
        PlaybackOptions {
            looping: self.current().and_then(|s| self.loop_for(&s.id)).is_some(),
            ..self.advance_options()
        }
    }
    fn advance_options(&self) -> PlaybackOptions {
        // The engine asks for a successor only after ruling out a playable loop.
        // A cached tag length can still disagree with it before the first UI tick.
        PlaybackOptions {
            autoplay: self.setting("autoplay").as_bool().unwrap_or(true),
            looping: false,
        }
    }
    fn key(&mut self) -> u64 {
        self.sequence += 1;
        self.sequence
    }
    fn track(&self, id: &str, key: u64) -> Option<Track> {
        // Only the decoder knows the playable length, including joined files.
        let range = self.saved_loop(id, None);
        // The saved path back to the file's real name (a name holding a lone UTF-16 unit is stored encoded).
        let path = string_core::library::native_path(&self.models.by_id.get(id)?.path);
        Some(Track::new(key, path).with_loop(range))
    }
    pub fn load_current(&mut self, p: &impl Transport, start: bool, pos: f64) {
        self.pending = None;
        self.superseded = None;
        self.active = self.key();
        self.active_failed = false;
        self.active_duration = None;
        self.counted = false;
        self.was_playing = false;
        self.last_tick = Instant::now();
        if let Some(track) = self
            .state
            .queue
            .current()
            .and_then(|id| self.track(id, self.active))
        {
            if start {
                p.play_track(track, pos)
            } else {
                p.cue(track, pos)
            }
        } else {
            p.stop()
        }
    }
    pub fn play_ids(&mut self, p: &impl Transport, ids: &[String], index: isize, ctx: Value) {
        if self.state.play_ids(
            ids,
            index,
            ctx,
            &self.models.catalog,
            Local::now().timestamp_millis(),
        ) {
            self.state_dirty = true;
            self.load_current(p, true, 0.);
        }
    }
    pub fn toggle(&mut self, p: &impl Transport) {
        if self.active != 0 && self.current().is_some() {
            p.toggle()
        } else {
            self.resume(p)
        }
    }
    pub fn resume(&mut self, p: &impl Transport) {
        if self.current().is_some() {
            if p.status().key == Some(self.active) {
                p.play()
            } else {
                self.load_current(p, true, self.state.queue.pos())
            }
        } else if !self.state.queue.items().is_empty() {
            let queue = &mut self.state.queue;
            let catalog = &self.models.catalog;
            // Stay in the saved queue. Its successor also knows the Repeat All list boundaries.
            let opts = PlaybackOptions {
                autoplay: false,
                looping: false,
            };
            let plan = queue.what_next(true, opts, catalog);
            let started = if plan.action() == NextAction::Start {
                queue.commit_next(plan, opts, catalog)
            } else {
                (0..queue.items().len()).any(|i| queue.jump(i, catalog))
            };
            if started {
                self.state_dirty = true;
                self.load_current(p, true, 0.);
            }
        } else if !self.models.songs.is_empty() {
            let ids = self.models.songs.clone();
            self.state.queue.set_shuffle(true, &self.models.catalog);
            self.play_ids(
                p,
                &ids,
                -1,
                json!({"kind":"songs","key":"","name":"All songs"}),
            );
        }
    }
    pub fn previous(&mut self, p: &Player) {
        let at = self.state.queue.at();
        let st = p.status();
        let position = if st.key == Some(self.active) {
            st.position
        } else {
            self.state.queue.pos()
        };
        self.state.queue.previous(position, &self.models.catalog);
        // Repeat All can select this same place as a real previous song after wrapping.
        let wrapped = position <= 3.5
            && self.state.queue.repeat() == string_core::queue::Repeat::All
            && self
                .state
                .queue
                .current()
                .and_then(|id| self.models.catalog.tracks.get(id))
                .is_some_and(|song| !song.hidden && !song.unplayable);
        if self.state.queue.at() == at && !wrapped {
            self.seek(p, 0.);
        } else {
            self.state_dirty = true;
            self.load_current(p, true, 0.)
        }
    }
    pub fn next(&mut self, p: &impl Transport) {
        let st = p.status();
        if self.bar_song(st).is_some() && st.duration.is_some() {
            self.active_duration = st.duration;
        }
        let opts = self.skip_options();
        let plan = self.state.queue.what_next(true, opts, &self.models.catalog);
        if self.state.queue.items().is_empty() {
            return;
        }
        if plan.action() == NextAction::Stop {
            self.record_seek(0.);
            p.rewind_paused();
        } else if plan.action() == NextAction::Restart {
            self.seek(p, 0.);
            if !p.status().playing {
                self.resume(p);
            }
        } else if self.state.commit_next(
            plan,
            opts,
            &self.models.catalog,
            Local::now().timestamp_millis(),
        ) {
            self.state_dirty = true;
            self.load_current(p, true, 0.)
        }
    }
    fn invalidate_plan(&mut self) {
        if let Some((key, _)) = self.pending.take() {
            self.superseded = Some(key);
        }
    }
    pub fn queue_changed(&mut self, p: &Player) {
        self.invalidate_plan();
        self.state_dirty = true;
        p.queue_changed();
    }
    pub fn seek(&mut self, p: &impl Transport, pos: f64) {
        let pos = pos.max(0.).min(p.status().duration.unwrap_or(f64::MAX));
        self.record_seek(pos);
        p.seek(pos);
    }
    pub fn seek_from_bar(&mut self, p: &impl Transport, key: u64, pos: f64) {
        let st = p.status();
        if self.active == key
            && !self.active_failed
            && st.key.is_none_or(|published| published == key)
        {
            let pos = pos.max(0.).min(st.duration.unwrap_or(f64::MAX));
            self.record_seek(pos);
            p.seek_track(key, pos);
        }
    }
    fn record_seek(&mut self, pos: f64) {
        self.seek_revision = self.seek_revision.wrapping_add(1);
        self.seek_position = pos;
        self.state.queue.seek(pos);
        self.state_dirty = true;
        self.last_tick = Instant::now();
        self.last_position = pos;
        self.was_playing = false;
    }
    pub fn toggle_like(&mut self, ids: &[String]) {
        let likes = self
            .state
            .fields
            .entry("liked")
            .or_insert_with(|| json!({}));
        if !likes.is_object() {
            *likes = json!({})
        }
        let obj = likes.as_object_mut().unwrap();
        let remove = ids.iter().all(|id| obj.contains_key(id));
        for id in ids {
            if remove {
                obj.remove(id);
            } else {
                obj.insert(id.clone(), Local::now().timestamp_millis().into());
            }
        }
        self.models.refresh_lists(&self.state);
        self.generation += 1;
        self.state_dirty = true;
    }
    pub fn insert(&mut self, p: &Player, ids: &[String], later: bool) {
        self.state.queue.insert(ids, later, &self.models.catalog);
        self.queue_changed(p);
    }
    pub fn set_loop(&mut self, p: &Player, point: Option<(bool, f64)>) {
        let Some(song) = self.current() else {
            return;
        };
        let id = song.id.clone();
        let st = p.status();
        let duration = st
            .duration
            .filter(|_| st.key == Some(self.active))
            .unwrap_or(song.duration);
        if point.is_some() && duration <= 0. {
            return;
        }
        let previous = self.saved_loop(&id, Some(duration));
        if point.is_none() && previous.is_none() {
            return;
        }
        let lp = point.and_then(|(start, at)| edit_loop(previous, start, at, duration));
        let entry = self
            .state
            .fields
            .entry("loops")
            .or_insert_with(|| json!({}));
        if !entry.is_object() {
            *entry = json!({})
        }
        let obj = entry.as_object_mut().unwrap();
        if let Some(lp) = lp {
            let a = (lp.start > 0.).then_some(lp.start);
            obj.insert(id, json!({"a":a,"b":lp.end}));
            let start = crate::theme::time(lp.start);
            let message = if let Some(end) = lp.end {
                format!("Looping {start}–{}", crate::theme::time(end))
            } else {
                format!("Loop starts at {start}. Right-click where it should end.")
            };
            self.notices.push_back(message);
        } else {
            obj.remove(&id);
            self.notices.push_back("Loop off".into());
        }
        p.set_loop(lp);
        self.queue_changed(p);
    }
    pub fn tick(&mut self, st: Status, force: bool) {
        // An exhausted deck can disappear for a callback before its loop restarts.
        // Keep the last confirmed sample so the restart's rewind is counted once.
        if st.key.is_none() && self.last_key == Some(self.active) && !self.active_failed {
            return;
        }
        let dt = self.last_tick.elapsed().as_secs_f64();
        if !force && dt < 0.5 && self.was_playing == st.playing && self.last_key == st.key {
            return;
        }
        let key_matches = st.key == Some(self.active) && !self.active_failed;
        if key_matches {
            self.active_duration = st.duration;
            let rewound = st.loop_offset - self.last_loop_offset;
            let heard = st.position - self.last_position + rewound;
            if self.was_playing
                && heard > 0.
                && heard <= dt + 0.15
                && (dt <= 30. || rewound > 0.)
                && self.last_key == st.key
                && let Some(s) = self.current()
            {
                let track = CountedTrack(s.id.clone(), s.title.clone(), s.artist.clone());
                let now = Local::now().naive_local();
                // A delayed tick can cover many real jumps. The listening document accepts
                // local samples of up to 30 seconds apiece.
                let mut remaining = heard;
                while remaining > 30. {
                    remaining -= 30.;
                    let at = now - chrono::TimeDelta::milliseconds((remaining * 1000.) as i64);
                    self.heard(track.clone(), 30., at);
                }
                self.heard(track, remaining, now);
            }
            if self.state.queue.pos() != st.position {
                self.state.queue.seek(st.position);
                self.state_dirty = true;
            }
            if st.playing
                && let Some(id) = self.state.queue.current().map(str::to_owned)
            {
                if !self.was_playing || self.last_key != st.key {
                    self.state.queue.confirm_playing(&id);
                    self.state_dirty = true;
                }
                if !self.counted && st.position > 30.0_f64.min(st.duration.unwrap_or(60.) / 2.) {
                    let plays = self
                        .state
                        .fields
                        .entry("plays")
                        .or_insert_with(|| json!({}));
                    if !plays.is_object() {
                        *plays = json!({})
                    }
                    let n = plays[&id][0].as_u64().unwrap_or(0);
                    plays[&id] = json!([n + 1, Local::now().timestamp_millis()]);
                    self.plays_revision += 1;
                    self.counted = true;
                    self.state_dirty = true;
                }
            }
        }
        self.last_tick = Instant::now();
        self.was_playing = key_matches && st.playing;
        self.last_key = st.key;
        self.last_position = st.position;
        self.last_loop_offset = st.loop_offset;
    }
    pub fn add_playlist(&mut self, name: String, ids: &[String]) {
        let now = Local::now().timestamp_millis();
        let list = self
            .state
            .fields
            .entry("playlists")
            .or_insert_with(|| json!([]));
        if let Some(list) = list.as_array_mut() {
            list.push(json!({"id":format!("{:x}",rand::random::<u64>()),"name":name,"created":now,"updated":now,"items":ids.iter().map(|id|json!({"id":id,"at":now})).collect::<Vec<_>>()}));
            self.models.refresh_lists(&self.state);
            self.generation += 1;
            self.state_dirty = true;
        }
    }
    pub fn add_to_playlist(&mut self, key: &str, ids: Ids) {
        if let Some(pl) = self
            .state
            .fields
            .get_mut("playlists")
            .and_then(Value::as_array_mut)
            .and_then(|p| p.iter_mut().find(|p| p["id"] == key))
            && let Some(items) = pl["items"].as_array_mut()
        {
            let now = Local::now().timestamp_millis();
            let mut seen: HashSet<_> = items
                .iter()
                .filter_map(|v| v.as_str().or_else(|| v["id"].as_str()))
                .collect();
            let fresh: Vec<_> = ids
                .iter()
                .filter(|id| seen.insert(id.as_str()))
                .map(|id| json!({"id": id, "at": now}))
                .collect();
            items.extend(fresh);
            pl["updated"] = now.into();
            self.state_dirty = true;
            self.models.refresh_lists(&self.state);
            self.generation += 1;
        }
    }
}
/// A missing endpoint follows the corresponding end of the song. Leave room for the end dip
/// when choosing points, as the original player does.
fn edit_loop(
    previous: Option<LoopRange>,
    start: bool,
    at: f64,
    duration: f64,
) -> Option<LoopRange> {
    if !at.is_finite() || duration < 1. {
        return None;
    }
    let limit = (duration - 0.3).max(0.);
    let at = at.clamp(0., limit);
    let mut a = previous.map_or(0., |lp| lp.start.min(limit));
    let mut b = previous.and_then(|lp| lp.end).map(|b| b.min(limit));
    if start {
        a = at
    } else {
        b = Some(at)
    }
    if let Some(end) = &mut b {
        if *end < a {
            std::mem::swap(&mut a, end);
        }
        if *end - a < 1. {
            *end = (a + 1.).min(duration);
            a = a.min(*end - 1.);
        }
    } else {
        a = a.min((duration - 0.12 - 1.).max(0.));
    }
    (a > 0. || b.is_some()).then_some(LoopRange { start: a, end: b })
}

struct QueueHost {
    session: Shared,
    ctx: Context,
}
impl Host for QueueHost {
    fn next_after(&mut self, current: &Track) -> Option<Track> {
        let mut s = self.session.lock().unwrap();
        if current.key != s.active && !s.pending.as_ref().is_some_and(|(k, _)| *k == current.key) {
            return None;
        }
        let plan = s
            .state
            .queue
            .what_next(false, s.advance_options(), &s.models.catalog);
        let Some(id) = plan.id().map(str::to_owned) else {
            s.invalidate_plan();
            return None;
        };
        let key = if plan.action() == NextAction::Restart {
            s.active
        } else if let Some((key, previous)) = &s.pending
            && previous.id() == Some(&id)
            && previous.action() == plan.action()
        {
            *key
        } else {
            s.invalidate_plan();
            s.key()
        };
        let t = s.track(&id, key)?;
        s.pending = Some((key, plan));
        Some(t)
    }
    fn event(&mut self, event: Event) {
        let mut s = self.session.lock().unwrap();
        match event {
            Event::Started { track } => {
                if s.pending.as_ref().is_some_and(|(k, _)| *k == track.key) {
                    let (_, plan) = s.pending.take().unwrap();
                    let opts = s.advance_options();
                    let s = &mut *s;
                    if !s.state.commit_next(
                        plan,
                        opts,
                        &s.models.catalog,
                        Local::now().timestamp_millis(),
                    ) {
                        s.resync = true;
                    } else {
                        s.active = track.key;
                        s.active_failed = false;
                        s.active_duration = None;
                        s.counted = false;
                        s.last_tick = Instant::now();
                        s.state_dirty = true;
                    }
                } else if s.superseded == Some(track.key) && track.key != s.active {
                    // The old deck started between the edit and the engine's replan.
                    s.resync = true;
                } else if track.key != s.active {
                    return;
                }
            }
            Event::Failed {
                track,
                error,
                selection,
            } => {
                if selection && track.key == s.active {
                    s.active_failed = true;
                    s.was_playing = false;
                }
                let id = string_core::library::track_id(&track.path.to_string_lossy());
                s.failed.insert(id.clone());
                if let Some(row) = s.models.catalog.tracks.get_mut(&id) {
                    row.unplayable = true
                }
                s.notices.push_back(format!(
                    "Cannot play {}: {error}",
                    track.path.file_name().unwrap_or_default().to_string_lossy()
                ));
            }
            Event::Stopped => {
                s.invalidate_plan();
                s.state.queue.seek(0.);
                s.state_dirty = true;
            }
            _ => {}
        }
        drop(s);
        self.ctx.request_repaint();
    }
}
pub fn open(session: &Shared, ctx: &Context, force_mute: bool) -> Player {
    let s = session.lock().unwrap();
    let cfg = Config {
        muted: force_mute || s.muted(),
        volume: s.volume(),
        crossfade: s.setting("fade").as_f64().unwrap_or(0.),
        spectrum: true,
        ..Default::default()
    };
    drop(s);
    let p = Player::new(
        cfg,
        QueueHost {
            session: session.clone(),
            ctx: ctx.clone(),
        },
    );
    let mut s = session.lock().unwrap();
    let pos = s.state.queue.pos();
    if s.current().is_some() {
        s.load_current(&p, false, pos)
    }
    p
}

#[cfg(test)]
mod player_tests {
    use super::*;
    use std::time::Duration;
    use string_audio::OfflinePlayer;

    struct SilentHost;
    impl Host for SilentHost {
        fn next_after(&mut self, _: &Track) -> Option<Track> {
            None
        }
        fn event(&mut self, _: Event) {}
    }
    fn session(paths: &[&str]) -> Session {
        let mut library = Library::default();
        for path in paths {
            library
                .tracks
                .push(serde_json::from_value(json!({"path":path,"duration":200})).unwrap());
        }
        Session::new(Default::default(), Default::default(), library)
    }
    fn muted_player() -> Player {
        Player::new(
            Config {
                muted: true,
                ..Default::default()
            },
            SilentHost,
        )
    }
    fn select(s: &mut Session, ids: &[&str], at: isize) {
        let ids: Vec<_> = ids.iter().map(|id| id.to_string()).collect();
        assert!(s.state.queue.play_ids(
            &ids,
            at,
            json!({"kind":"pl","key":"example"}),
            &s.models.catalog
        ));
    }

    #[test]
    fn loop_keys_normalize_and_report_each_change() {
        let mut s = session(&["a.wav"]);
        select(&mut s, &["a.wav"], 0);
        let p = muted_player();
        s.set_loop(&p, Some((true, 0.)));
        assert!(s.loop_for("a.wav").is_none());
        for (start, at, a, b) in [
            (true, 10., 10., None),
            (false, 5., 5., Some(10.)),
            (true, 30., 10., Some(30.)),
            (false, 10.3, 10., Some(11.)),
        ] {
            s.notices.clear();
            s.set_loop(&p, Some((start, at)));
            assert_eq!(s.loop_for("a.wav"), Some(LoopRange { start: a, end: b }));
            assert_eq!(s.notices.len(), 1);
        }
        s.set_loop(&p, None);
        s.set_loop(&p, Some((true, 400.)));
        assert!((s.loop_for("a.wav").unwrap().start - 198.88).abs() < 1e-8);
        s.set_loop(&p, Some((false, 400.)));
        let lp = s.loop_for("a.wav").unwrap();
        assert!(lp.end.unwrap() <= 200. && lp.end.unwrap() - lp.start >= 1.);
        s.set_loop(&p, None);
        assert_eq!(s.notices.back().unwrap(), "Loop off");
    }

    #[test]
    fn short_or_out_of_song_saved_loops_are_inactive() {
        let mut s = session(&["a.wav"]);
        select(&mut s, &["a.wav"], 0);
        for entry in [
            json!({"a":4,"b":4.9}),
            json!({"a":199.5}),
            json!({"a":201,"b":220}),
        ] {
            s.set_field("loops", json!({"a.wav":entry}));
            assert!(s.loop_for("a.wav").is_none());
            assert!(!s.skip_options().looping);
        }
    }

    #[test]
    fn play_without_current_preserves_the_list_and_shuffles_only_an_empty_queue() {
        let mut s = session(&["a.wav", "b.wav", "c.wav"]);
        let p = muted_player();
        s.resume(&p);
        assert!(s.state.queue.shuffled());
        assert_eq!(s.state.queue.items().len(), 3);
        for (at, missing, next) in [(0, "c.wav", "b.wav"), (1, "b.wav", "c.wav")] {
            let mut s = session(&["a.wav", "b.wav", "c.wav"]);
            select(&mut s, &["c.wav", "b.wav"], at);
            let saved = serde_json::to_vec(&s.state).unwrap();
            s.library.tracks.retain(|t| t.path != missing);
            let mut restored = Session::new(
                serde_json::from_slice(&saved).unwrap(),
                s.listening,
                s.library,
            );
            restored.resume(&p);
            assert_eq!(restored.state.queue.current(), Some(next));
            assert_eq!(restored.state.queue.items().len(), 2);
            assert_eq!(restored.state.queue.context(None)["key"], "example");
        }
    }

    #[test]
    fn next_at_a_stopping_edge_rewinds_the_selected_song() {
        let p = muted_player();
        for mode in ["autoplay off", "repeat one", "loop"] {
            let mut s = session(&["a.wav"]);
            select(&mut s, &["a.wav"], 0);
            match mode {
                "autoplay off" => s.set_setting("autoplay", false.into()),
                "repeat one" => s.state.queue.set_repeat(string_core::queue::Repeat::One),
                _ => s.set_field("loops", json!({"a.wav":{"a":2,"b":4}})),
            }
            s.state.queue.seek(51.);
            s.next(&p);
            assert_eq!(s.state.queue.pos(), 0., "{mode}");
            assert_eq!(s.state.queue.current(), Some("a.wav"));
        }
    }

    /// The owner's call (live try): Repeat One and a seek-bar loop repeat a song that ends by itself, but pressing
    /// Next on an album's last song goes on to the next album, as it does with Repeat off. (1.x stopped at 0:00.)
    #[test]
    fn next_on_an_albums_last_song_goes_on_to_the_next_album_with_a_loop_on() {
        for mode in ["repeat one", "seek-bar loop"] {
            let mut library = Library::default();
            for (path, album, n) in [("a1.wav", "Alpha", 1), ("a2.wav", "Alpha", 2), ("b1.wav", "Beta", 1), ("b2.wav", "Beta", 2)] {
                library.tracks.push(
                    serde_json::from_value(json!({
                        "path": path, "duration": 200, "title": path, "artist": "Band", "album": album, "track": n
                    }))
                    .unwrap(),
                );
            }
            let mut s = Session::new(Default::default(), Default::default(), library);
            let alpha = s
                .models
                .catalog
                .albums
                .iter()
                .find(|g| g.ids.iter().any(|id| id == "a1.wav"))
                .unwrap()
                .key
                .clone();
            let ctx = json!({"kind":"album","key":alpha});
            let ids = s.models.context_ids(&ctx);
            assert_eq!(&ids[..], ["a1.wav", "a2.wav"]);
            assert!(s.state.queue.play_ids(&ids, 1, ctx, &s.models.catalog));
            match mode {
                "repeat one" => s.state.queue.set_repeat(string_core::queue::Repeat::One),
                _ => s.set_field("loops", json!({"a2.wav":{"a":2,"b":40}})),
            }
            let p = muted_player();
            s.next(&p);
            assert_eq!(s.state.queue.current(), Some("b1.wav"), "{mode}: Next on Alpha's last song");
            assert_eq!(s.state.queue.context(None)["kind"], "album", "{mode}");
            if mode == "repeat one" {
                // a song that ends by itself still repeats
                assert_eq!(s.state.queue.repeat(), string_core::queue::Repeat::One);
                let natural = s.state.queue.what_next(false, s.advance_options(), &s.models.catalog);
                assert_eq!(natural.action(), NextAction::Restart);
            }
        }
    }

    #[test]
    fn failed_selection_stops_showing_a_title_and_recovery_shows_the_confirmed_song() {
        let mut s = session(&["a.wav", "b.wav", "c.wav"]);
        select(&mut s, &["a.wav", "b.wav", "c.wav"], 0);
        s.active = s.key();
        let first = s.track("a.wav", s.active).unwrap();
        let shared = Arc::new(Mutex::new(s));
        let mut host = QueueHost {
            session: shared.clone(),
            ctx: Context::default(),
        };
        host.event(Event::Failed {
            track: first.clone(),
            error: "missing".into(),
            selection: true,
        });
        assert!(shared.lock().unwrap().current().is_none());
        let second = host.next_after(&first).unwrap();
        host.event(Event::Failed {
            track: second.clone(),
            error: "missing".into(),
            selection: true,
        });
        let third = host.next_after(&second).unwrap();
        host.event(Event::Started { track: third });
        assert_eq!(shared.lock().unwrap().current().unwrap().id, "c.wav");
    }

    #[test]
    fn previous_after_the_threshold_keeps_the_play_and_its_count() {
        let mut s = session(&["a.wav", "b.wav"]);
        select(&mut s, &["a.wav", "b.wav"], 1);
        s.state.queue.seek(35.);
        s.counted = true;
        let p = muted_player();
        let key = s.active;
        s.previous(&p);
        assert_eq!(s.state.queue.current(), Some("b.wav"));
        assert_eq!(s.state.queue.pos(), 0.);
        assert_eq!(s.active, key);
        assert!(s.counted);
        assert!(!p.status().playing);
        select(&mut s, &["b.wav"], 0);
        s.state.queue.set_repeat(string_core::queue::Repeat::All);
        s.previous(&p);
        assert_ne!(
            s.active, key,
            "Repeat All selects the only song again near its start"
        );
    }

    #[test]
    fn five_missing_files_stop_with_no_song_in_the_bar() {
        let paths = ["1.wav", "2.wav", "3.wav", "4.wav", "5.wav", "6.wav"];
        let mut s = session(&paths);
        select(&mut s, &paths, 0);
        let first = s.track(paths[0], s.active).unwrap();
        let shared = Arc::new(Mutex::new(s));
        let host = QueueHost {
            session: shared.clone(),
            ctx: Context::default(),
        };
        let mut player = OfflinePlayer::new(48000, Config::default(), host);
        player.play_track(first, 0.);
        player.render_secs(0.1);
        let s = shared.lock().unwrap();
        assert_eq!(s.failed.len(), 5);
        assert!(!player.status().playing);
        assert!(s.current().is_none());
    }

    fn write_song(path: &std::path::Path, seconds: u32) {
        let mut wav = b"RIFF".to_vec();
        let bytes = 8000_u32 * seconds * 2;
        wav.extend((36 + bytes).to_le_bytes());
        wav.extend(b"WAVEfmt \x10\0\0\0\x01\0\x01\0");
        wav.extend(8000_u32.to_le_bytes());
        wav.extend(16000_u32.to_le_bytes());
        wav.extend(b"\x02\0\x10\0data");
        wav.extend(bytes.to_le_bytes());
        wav.resize(44 + bytes as usize, 0);
        std::fs::write(path, wav).unwrap();
    }

    struct TestTransport(std::cell::RefCell<OfflinePlayer>);
    impl Transport for TestTransport {
        fn status(&self) -> Status {
            self.0.borrow().status()
        }
        fn play_track(&self, track: Track, from: f64) {
            self.0.borrow_mut().play_track(track, from)
        }
        fn cue(&self, track: Track, from: f64) {
            self.0.borrow_mut().cue(track, from)
        }
        fn stop(&self) {
            self.0.borrow_mut().stop()
        }
        fn play(&self) {
            self.0.borrow_mut().play()
        }
        fn toggle(&self) {
            self.0.borrow_mut().toggle()
        }
        fn rewind_paused(&self) {
            self.0.borrow_mut().rewind_paused()
        }
        fn seek(&self, position: f64) {
            self.0.borrow_mut().seek(position)
        }
        fn seek_track(&self, key: u64, position: f64) {
            self.0.borrow_mut().seek_track(key, position)
        }
    }

    #[test]
    fn review_failed_startup_cue_keeps_the_saved_bookmark() {
        let mut s = session(&["missing.wav"]);
        select(&mut s, &["missing.wav"], 0);
        s.state.queue.seek(742.25);
        let track = s.track("missing.wav", s.active).unwrap();
        let shared = Arc::new(Mutex::new(s));
        let host = QueueHost {
            session: shared.clone(),
            ctx: Context::default(),
        };
        let mut p = OfflinePlayer::new(8000, Config::default(), host);
        p.cue(track, 742.25);
        p.render(480);
        let mut s = shared.lock().unwrap();
        s.tick(p.status(), true);
        assert!(s.current().is_none());
        let saved: State = serde_json::from_value(serde_json::to_value(&s.state).unwrap()).unwrap();
        assert_eq!(saved.queue.current(), Some("missing.wav"));
        assert_eq!(saved.queue.pos(), 742.25);
        assert!(!s.state_dirty);
    }

    #[test]
    fn review_listening_tick_between_loop_decks_keeps_the_last_heard_frame() {
        let mut s = session(&["loop.wav"]);
        select(&mut s, &["loop.wav"], 0);
        let playing = Status {
            key: Some(s.active),
            duration: Some(4.),
            position: 3.6,
            playing: true,
            ..Default::default()
        };
        s.tick(playing, true);
        // The mixer released the exhausted deck before publishing its replacement.
        s.tick(Status::default(), true);
        s.last_tick = Instant::now() - Duration::from_secs(1);
        s.tick(
            Status {
                position: 1.1,
                loop_offset: 3.,
                ..playing
            },
            true,
        );
        let heard: f64 = s.listening.days().values().map(|day| day.s).sum();
        assert!((heard - 0.5).abs() < 0.0001, "heard {heard}");
    }

    #[test]
    fn review_active_loop_uses_the_decoded_length() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("longer.wav");
        write_song(&path, 8);
        let name = path.to_str().unwrap();
        let id = string_core::library::track_id(name);
        let mut s = session(&[name]);
        s.models.by_id.get_mut(&id).unwrap().duration = 2.;
        select(&mut s, &[&id], 0);
        s.set_field("loops", json!({&id:{"a":4,"b":6}}));
        let mut p = OfflinePlayer::new(8000, Config::default(), SilentHost);
        p.play_track(s.track(&id, s.active).unwrap(), 5.);
        p.render_secs(1.5);
        assert_eq!(p.status().duration, Some(8.));
        assert!(
            (p.status().position - 4.5).abs() < 0.001,
            "{:?}",
            p.status()
        );
        assert_eq!(s.field("loops")[&id], json!({"a":4,"b":6}));
        // Next can arrive before the UI tick has copied the decoder's duration. A loop doesn't hold a skip back:
        // with nothing else to go on to, Next starts the song again, as with Repeat off.
        let p = TestTransport(p.into());
        s.next(&p);
        p.0.borrow_mut().render_secs(0.5);
        assert!(
            p.status().playing && p.status().position < 1.,
            "{:?}",
            p.status()
        );
    }

    #[test]
    fn review_loop_beyond_decoded_eof_allows_autoplay_before_the_first_ui_tick() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("shorter.wav");
        let next = dir.path().join("next.wav");
        write_song(&first, 8);
        write_song(&next, 8);
        let paths = [first.to_str().unwrap(), next.to_str().unwrap()];
        let ids = paths.map(string_core::library::track_id);
        let mut s = session(&paths);
        select(&mut s, &[&ids[0]], 0);
        s.set_field("loops", json!({&ids[0]:{"a":7.5,"b":12}}));
        let track = s.track(&ids[0], s.active).unwrap();
        let shared = Arc::new(Mutex::new(s));
        let host = QueueHost {
            session: shared.clone(),
            ctx: Context::default(),
        };
        let mut p = OfflinePlayer::new(8000, Config::default(), host);
        p.play_track(track, 7.);
        p.render_secs(1.5);
        let s = shared.lock().unwrap();
        assert_eq!(s.state.queue.current(), Some(ids[1].as_str()));
        assert!(p.status().playing && !s.resync);
    }

    #[test]
    fn review_next_stops_on_the_exact_first_frame() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("edge.wav");
        write_song(&path, 5);
        let name = path.to_str().unwrap();
        let id = string_core::library::track_id(name);
        let mut s = session(&[name]);
        select(&mut s, &[&id], 0);
        s.set_setting("autoplay", false.into());
        let p = TestTransport(OfflinePlayer::new(8000, Config::default(), SilentHost).into());
        s.load_current(&p, true, 1.);
        p.0.borrow_mut().render_secs(0.5);
        assert!(p.status().playing);
        s.next(&p);
        p.0.borrow_mut().render_secs(0.5);
        assert!(!p.status().playing);
        assert_eq!(p.status().key, Some(s.active));
        assert_eq!(p.status().position, 0.);
        s.resume(&p);
        p.0.borrow_mut().render(1);
        assert_eq!(p.status().position, 1. / 8000.);
    }

    /// Next inside an album moves one song on, whatever loop is on (reported live: it went back into the album).
    #[test]
    fn next_in_an_album_moves_one_song_with_any_loop_on() {
        for (mode, shuffle) in ["repeat one", "repeat all", "seek-bar loop"]
            .into_iter()
            .flat_map(|m| [(m, false), (m, true)])
        {
            let dir = tempfile::tempdir().unwrap();
            let paths: Vec<_> = (1..=3)
                .map(|n| {
                    let path = dir.path().join(format!("0{n} song.wav"));
                    write_song(&path, 8);
                    path.to_str().unwrap().to_owned()
                })
                .collect();
            let names: Vec<&str> = paths.iter().map(String::as_str).collect();
            let ids: Vec<String> = names.iter().map(|p| string_core::library::track_id(p)).collect();
            let mut s = session(&names);
            let album = s.models.catalog.albums.first().map(|g| g.key.clone()).unwrap_or_default();
            assert!(s.state.queue.play_ids(
                &ids,
                0,
                json!({"kind":"album","key":album}),
                &s.models.catalog
            ));
            if shuffle {
                s.state.queue.set_shuffle(true, &s.models.catalog);
            }
            let first = s.state.queue.current().unwrap().to_owned();
            match mode {
                "repeat one" => s.state.queue.set_repeat(string_core::queue::Repeat::One),
                "repeat all" => s.state.queue.set_repeat(string_core::queue::Repeat::All),
                _ => s.set_field("loops", json!({&first:{"a":1,"b":3}})),
            }
            let p = TestTransport(OfflinePlayer::new(8000, Config::default(), SilentHost).into());
            s.load_current(&p, true, 0.);
            p.0.borrow_mut().render_secs(1.5);
            let mut heard = vec![first];
            for press in 1..=2 {
                s.next(&p);
                p.0.borrow_mut().render_secs(0.5);
                let now = s.state.queue.current().unwrap().to_owned();
                assert!(
                    !heard.contains(&now),
                    "{mode}, shuffle {shuffle}: Next #{press} went back to a song already heard ({heard:?} then {now})"
                );
                assert!(p.status().playing, "{mode}, shuffle {shuffle}: Next #{press} paused");
                heard.push(now);
            }
            assert_eq!(heard.len(), ids.len());
        }
    }

    /// Commands queued like the real player's channel, run on the engine once the session lock is released.
    struct Deferred<'a> {
        player: &'a std::cell::RefCell<OfflinePlayer>,
        cmds: std::cell::RefCell<Vec<Box<dyn FnOnce(&mut OfflinePlayer) + 'a>>>,
    }
    impl<'a> Deferred<'a> {
        fn push(&self, f: impl FnOnce(&mut OfflinePlayer) + 'a) {
            self.cmds.borrow_mut().push(Box::new(f));
        }
        fn apply(&self) {
            for f in self.cmds.take() {
                f(&mut self.player.borrow_mut());
            }
        }
    }
    impl Transport for Deferred<'_> {
        fn status(&self) -> Status {
            self.player.borrow().status()
        }
        fn play_track(&self, track: Track, from: f64) {
            self.push(move |p| p.play_track(track, from))
        }
        fn cue(&self, track: Track, from: f64) {
            self.push(move |p| p.cue(track, from))
        }
        fn stop(&self) {
            self.push(|p| p.stop())
        }
        fn play(&self) {
            self.push(|p| p.play())
        }
        fn toggle(&self) {
            self.push(|p| p.toggle())
        }
        fn rewind_paused(&self) {
            self.push(|p| p.rewind_paused())
        }
        fn seek(&self, position: f64) {
            self.push(move |p| p.seek(position))
        }
        fn seek_track(&self, key: u64, position: f64) {
            self.push(move |p| p.seek_track(key, position))
        }
    }

    /// The live report: in an album with a loop on, Next restarted or paused the same song. This drives the real
    /// engine host, which has already prepared the loop's "same song again" when Next arrives.
    #[test]
    fn next_with_a_prepared_repeat_moves_on_in_an_album() {
        for mode in ["repeat one", "repeat all", "seek-bar loop"] {
            for shuffle in [false, true] {
                let dir = tempfile::tempdir().unwrap();
                let paths: Vec<_> = (1..=3)
                    .map(|n| {
                        let path = dir.path().join(format!("0{n} song.wav"));
                        write_song(&path, 8);
                        path.to_str().unwrap().to_owned()
                    })
                    .collect();
                let names: Vec<&str> = paths.iter().map(String::as_str).collect();
                let ids: Vec<String> =
                    names.iter().map(|p| string_core::library::track_id(p)).collect();
                let mut s = session(&names);
                let album = s.models.catalog.albums.first().map(|g| g.key.clone()).unwrap_or_default();
                assert!(s.state.queue.play_ids(
                    &ids,
                    0,
                    json!({"kind":"album","key":album}),
                    &s.models.catalog
                ));
                if shuffle {
                    s.state.queue.set_shuffle(true, &s.models.catalog);
                }
                let first = s.state.queue.current().unwrap().to_owned();
                match mode {
                    "repeat one" => s.state.queue.set_repeat(string_core::queue::Repeat::One),
                    "repeat all" => s.state.queue.set_repeat(string_core::queue::Repeat::All),
                    _ => s.set_field("loops", json!({&first:{"a":1,"b":7}})),
                }
                let shared = Arc::new(Mutex::new(s));
                let host = QueueHost {
                    session: shared.clone(),
                    ctx: Context::default(),
                };
                let player = std::cell::RefCell::new(OfflinePlayer::new(
                    8000,
                    Config {
                        muted: true,
                        ..Default::default()
                    },
                    host,
                ));
                let t = Deferred {
                    player: &player,
                    cmds: Default::default(),
                };
                shared.lock().unwrap().load_current(&t, true, 0.);
                t.apply();
                // Long enough for the engine to have asked for and prepared what follows.
                player.borrow_mut().render_secs(5.);
                let mut heard = vec![first];
                for press in 1..=2 {
                    shared.lock().unwrap().next(&t);
                    t.apply();
                    player.borrow_mut().render_secs(1.);
                    let s = shared.lock().unwrap();
                    let now = s.state.queue.current().unwrap().to_owned();
                    let st = player.borrow().status();
                    let label = format!("{mode}, shuffle {shuffle}, Next #{press}");
                    assert!(!heard.contains(&now), "{label}: the queue went back to {now} ({heard:?})");
                    assert!(st.playing, "{label}: paused ({st:?})");
                    assert_eq!(st.key, Some(s.active), "{label}: the engine plays another track ({st:?})");
                    assert!(st.position < 1.5, "{label}: not from the start ({st:?})");
                    heard.push(now);
                }
            }
        }
    }

    /// The same with the real-time, muted player and its threads: the skip button mid-song and near the end.
    /// It needs a working audio output (a muted player still waits for one), so it runs only when asked for.
    #[test]
    #[ignore = "needs an audio output device: cargo test -- --ignored"]
    fn next_button_in_an_album_moves_on_in_real_time() {
        for (mode, press_at) in [
            ("repeat one", 1.5),
            ("repeat one", 6.5),
            ("seek-bar loop", 3.),
            ("repeat all", 6.5),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let paths: Vec<_> = (1..=3)
                .map(|n| {
                    let path = dir.path().join(format!("0{n} song.wav"));
                    write_song(&path, 8);
                    path.to_str().unwrap().to_owned()
                })
                .collect();
            let names: Vec<&str> = paths.iter().map(String::as_str).collect();
            let ids: Vec<String> = names.iter().map(|p| string_core::library::track_id(p)).collect();
            let mut s = session(&names);
            let album = s.models.catalog.albums.first().map(|g| g.key.clone()).unwrap_or_default();
            assert!(s.state.queue.play_ids(
                &ids,
                0,
                json!({"kind":"album","key":album}),
                &s.models.catalog
            ));
            match mode {
                "repeat one" => s.state.queue.set_repeat(string_core::queue::Repeat::One),
                "repeat all" => s.state.queue.set_repeat(string_core::queue::Repeat::All),
                _ => s.set_field("loops", json!({&ids[0]:{"a":1,"b":7}})),
            }
            let shared = Arc::new(Mutex::new(s));
            let host = QueueHost {
                session: shared.clone(),
                ctx: Context::default(),
            };
            let p = Player::new(
                Config {
                    muted: true,
                    ..Default::default()
                },
                host,
            );
            shared.lock().unwrap().load_current(&p, true, 0.);
            let wait_for = |what: &dyn Fn(&Status) -> bool, secs: f64| {
                let end = Instant::now() + Duration::from_secs_f64(secs);
                while Instant::now() < end && !what(&p.status()) {
                    std::thread::sleep(Duration::from_millis(10));
                }
                what(&p.status())
            };
            assert!(
                wait_for(&|st| st.playing && st.position >= press_at, press_at + 4.),
                "{mode}: never reached {press_at} s ({:?})",
                p.status()
            );
            shared.lock().unwrap().next(&p);
            std::thread::sleep(Duration::from_millis(700));
            let st = p.status();
            let s = shared.lock().unwrap();
            let label = format!("{mode}, Next at {press_at} s");
            assert_eq!(s.state.queue.current(), Some(ids[1].as_str()), "{label}: the queue ({st:?})");
            assert!(st.playing, "{label}: paused ({st:?})");
            assert_eq!(st.key, Some(s.active), "{label}: the engine plays another track ({st:?})");
            assert!(st.position < 1.5, "{label}: not from the start of the next song ({st:?})");
        }
    }

    #[test]
    fn review_repeat_one_preload_failure_keeps_the_playing_song() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("repeat.wav");
        write_song(&path, 8);
        let name = path.to_str().unwrap();
        let id = string_core::library::track_id(name);
        let mut s = session(&[name]);
        select(&mut s, &[&id], 0);
        s.state.queue.set_repeat(string_core::queue::Repeat::One);
        let track = s.track(&id, s.active).unwrap();
        let shared = Arc::new(Mutex::new(s));
        let host = QueueHost {
            session: shared.clone(),
            ctx: Context::default(),
        };
        let mut p = OfflinePlayer::new(
            8000,
            Config {
                ask_ahead: 4.,
                ..Default::default()
            },
            host,
        );
        p.play_track(track, 0.);
        p.render_secs(1.);
        std::fs::rename(&path, dir.path().join("moved.wav")).unwrap();
        p.render_secs(4.);
        let mut s = shared.lock().unwrap();
        assert!(!s.notices.is_empty(), "the repeat failed to open");
        assert!(p.status().playing);
        assert_eq!(s.bar_song(p.status()).map(|song| &song.id), Some(&id));
        s.tick(p.status(), true);
        drop(s);
        p.render(1);
        let mut s = shared.lock().unwrap();
        s.tick(p.status(), true);
        assert!(s.listening.days().values().any(|day| day.s > 0.));
    }

    #[test]
    fn review_seek_release_during_an_unpublished_selection_cannot_move_the_new_track() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.wav");
        let second = dir.path().join("second.wav");
        write_song(&first, 5);
        write_song(&second, 5);
        let name = first.to_str().unwrap();
        let id = string_core::library::track_id(name);
        let mut s = session(&[name]);
        select(&mut s, &[&id], 0);
        let p = TestTransport(OfflinePlayer::new(8000, Config::default(), SilentHost).into());
        s.load_current(&p, true, 0.);
        p.0.borrow_mut().render_secs(0.1);
        let pressed = s.active;
        // The engine has selected the successor while the UI still sees the previous callback.
        p.play_track(Track::new(pressed + 1, second), 0.);
        assert_eq!(p.status().key, Some(pressed));
        s.seek_from_bar(&p, pressed, 3.);
        p.0.borrow_mut().render_secs(0.5);
        assert_eq!(p.status().key, Some(pressed + 1));
        assert!(p.status().position <= 0.5, "{:?}", p.status());
    }

    #[test]
    fn review_toggle_cancels_a_selection_before_output_returns() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.wav");
        let b = dir.path().join("b.wav");
        write_song(&a, 5);
        write_song(&b, 5);
        let paths = [a.to_str().unwrap(), b.to_str().unwrap()];
        let ids = paths.map(string_core::library::track_id);
        let mut s = session(&paths);
        select(&mut s, &[&ids[0], &ids[1]], 0);
        let p = TestTransport(OfflinePlayer::new(8000, Config::default(), SilentHost).into());
        s.load_current(&p, true, 0.);
        p.0.borrow_mut().render_secs(0.1);
        let retired = p.status();
        s.next(&p);
        let selected = s.active;
        assert!(retired.playing && s.bar_song(retired).is_none());
        // No callbacks from the retired device between selecting B and cancelling it.
        s.toggle(&p);
        p.0.borrow_mut().render_secs(0.5);
        assert!(!p.status().playing, "{:?}", p.status());
        assert_eq!(p.status().key, Some(selected));
    }

    #[test]
    fn one_second_loops_count_every_rendered_second_between_infrequent_ticks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("loop.wav");
        write_song(&path, 4);
        let name = path.to_str().unwrap();
        let id = string_core::library::track_id(name);
        let mut s = session(&[name]);
        select(&mut s, &[&id], 0);
        s.set_field("loops", json!({&id:{"a":1,"b":2}}));
        let mut p = OfflinePlayer::new(8000, Config::default(), SilentHost);
        p.play_track(s.track(&id, s.active).unwrap(), 1.);
        p.render_secs(0.1);
        s.tick(p.status(), true);
        for seconds in [0.5, 1., 2.25, 1.25, 32.] {
            p.render_secs(seconds);
            s.last_tick = Instant::now() - Duration::from_secs_f64(seconds);
            s.tick(p.status(), true);
        }
        let heard: f64 = s.listening.days().values().map(|d| d.s).sum();
        assert!((heard - 37.).abs() < 0.003, "heard {heard} of 37 seconds");
    }
}
