use crate::model::{Ids, Models, Song};
use chrono::Local;
use egui::Context;
use serde_json::{Value, json};
use std::{
    collections::{HashSet, VecDeque},
    sync::{Arc, Mutex},
    time::Instant,
};
use string_audio::{Config, Event, Host, LoopRange, Player, Status, Track};
use string_core::{
    library::Library,
    queue::{AdvancePlan, NextAction, PlaybackOptions},
    state::State,
    stats::{Listening, Track as CountedTrack},
};

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
    fn opts(&self) -> PlaybackOptions {
        PlaybackOptions {
            autoplay: self.setting("autoplay").as_bool().unwrap_or(true),
            looping: self.current().and_then(|s| self.loop_for(&s.id)).is_some(),
        }
    }
    fn key(&mut self) -> u64 {
        self.sequence += 1;
        self.sequence
    }
    fn track(&self, id: &str, key: u64) -> Option<Track> {
        Some(Track::new(key, string_core::library::native_path(&self.models.by_id.get(id)?.path)).with_loop(self.loop_for(id)))
    }
    pub fn load_current(&mut self, p: &Player, start: bool, pos: f64) {
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
    pub fn play_ids(&mut self, p: &Player, ids: &[String], index: isize, ctx: Value) {
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
    pub fn toggle(&mut self, p: &Player) {
        let st = p.status();
        if st.playing && self.bar_song(st).is_some() {
            p.pause()
        } else {
            self.resume(p)
        }
    }
    pub fn resume(&mut self, p: &Player) {
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
    pub fn next(&mut self, p: &Player) {
        let opts = self.opts();
        let plan = self.state.queue.what_next(true, opts, &self.models.catalog);
        if self.state.queue.items().is_empty() {
            return;
        }
        if plan.action() == NextAction::Stop {
            p.pause();
            self.seek(p, 0.);
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
    pub fn seek(&mut self, p: &Player, pos: f64) {
        self.seek_revision = self.seek_revision.wrapping_add(1);
        let pos = pos.max(0.).min(p.status().duration.unwrap_or(f64::MAX));
        self.seek_position = pos;
        self.state.queue.seek(pos);
        self.state_dirty = true;
        self.last_tick = Instant::now();
        self.last_position = pos;
        self.was_playing = false;
        p.seek(pos);
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
            for id in ids.iter() {
                if !items
                    .iter()
                    .any(|v| v["id"] == *id || v.as_str() == Some(id))
                {
                    items.push(json!({"id":id,"at":now}));
                }
            }
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
        let plan = s.state.queue.what_next(false, s.opts(), &s.models.catalog);
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
                    let opts = s.opts();
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
            Event::Failed { track, error } => {
                if track.key == s.active {
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
            assert!(s.track("a.wav", 1).unwrap().loop_range.is_none());
            assert!(!s.opts().looping);
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
        });
        assert!(shared.lock().unwrap().current().is_none());
        let second = host.next_after(&first).unwrap();
        host.event(Event::Failed {
            track: second.clone(),
            error: "missing".into(),
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

    #[test]
    fn one_second_loops_count_every_rendered_second_between_infrequent_ticks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("loop.wav");
        let mut wav = b"RIFF".to_vec();
        let bytes = 8000_u32 * 4 * 2;
        wav.extend((36 + bytes).to_le_bytes());
        wav.extend(b"WAVEfmt \x10\0\0\0\x01\0\x01\0");
        wav.extend(8000_u32.to_le_bytes());
        wav.extend(16000_u32.to_le_bytes());
        wav.extend(b"\x02\0\x10\0data");
        wav.extend(bytes.to_le_bytes());
        wav.resize(44 + bytes as usize, 0);
        std::fs::write(&path, wav).unwrap();
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
