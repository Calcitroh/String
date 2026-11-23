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
    sequence: u64,
    pending: Option<(u64, AdvancePlan)>,
    superseded: Option<u64>,
    failed: HashSet<String>,
    counted: bool,
    last_tick: Instant,
    was_playing: bool,
    last_key: Option<u64>,
    last_position: f64,
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
            sequence: 0,
            pending: None,
            superseded: None,
            failed: HashSet::new(),
            counted: false,
            last_tick: Instant::now(),
            was_playing: false,
            last_key: None,
            last_position: 0.,
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
        self.models.by_id.get(self.state.queue.current()?)
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
    pub fn loop_for(&self, id: &str) -> Option<LoopRange> {
        let lp = &self.field("loops")[id];
        (lp["a"].is_number() || lp["b"].is_number()).then(|| LoopRange {
            start: lp["a"].as_f64().unwrap_or(0.),
            end: lp["b"].as_f64(),
        })
    }
    fn opts(&self) -> PlaybackOptions {
        PlaybackOptions {
            autoplay: self.setting("autoplay").as_bool().unwrap_or(true),
            looping: self
                .state
                .queue
                .current()
                .and_then(|id| self.loop_for(id))
                .is_some(),
        }
    }
    fn key(&mut self) -> u64 {
        self.sequence += 1;
        self.sequence
    }
    fn track(&self, id: &str, key: u64) -> Option<Track> {
        Some(Track::new(key, &self.models.by_id.get(id)?.path).with_loop(self.loop_for(id)))
    }
    pub fn load_current(&mut self, p: &Player, start: bool, pos: f64) {
        self.pending = None;
        self.superseded = None;
        self.active = self.key();
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
        if p.status().playing {
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
        } else {
            let ids = self.models.songs.clone();
            self.play_ids(
                p,
                &ids,
                0,
                json!({"kind":"songs","key":"","name":"All songs"}),
            );
        }
    }
    pub fn previous(&mut self, p: &Player) {
        if self
            .state
            .queue
            .previous(p.status().position, &self.models.catalog)
            .is_some()
        {
            self.state_dirty = true;
            self.load_current(p, true, 0.)
        }
    }
    pub fn next(&mut self, p: &Player) {
        let opts = self.opts();
        let plan = self.state.queue.what_next(true, opts, &self.models.catalog);
        if plan.action() != NextAction::Stop
            && self.state.commit_next(
                plan,
                opts,
                &self.models.catalog,
                Local::now().timestamp_millis(),
            )
        {
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
        let Some(id) = self.state.queue.current().map(str::to_owned) else {
            return;
        };
        let mut lp = self.loop_for(&id).unwrap_or(LoopRange {
            start: 0.,
            end: None,
        });
        if let Some((start, at)) = point {
            if start {
                lp.start = at;
                if lp.end.is_some_and(|b| b < at + 0.5) {
                    lp.end = None
                }
            } else if at >= lp.start + 0.5 {
                lp.end = Some(at)
            } else {
                self.notices
                    .push_back("The loop end must follow its start.".into());
                return;
            }
        }
        let entry = self
            .state
            .fields
            .entry("loops")
            .or_insert_with(|| json!({}));
        if !entry.is_object() {
            *entry = json!({})
        }
        let obj = entry.as_object_mut().unwrap();
        if point.is_some() {
            obj.insert(id, json!({"a":lp.start,"b":lp.end}));
            p.set_loop(Some(lp));
        } else {
            obj.remove(&id);
            p.set_loop(None)
        }
        self.queue_changed(p);
    }
    pub fn tick(&mut self, st: Status, force: bool) {
        let dt = self.last_tick.elapsed().as_secs_f64();
        if !force && dt < 0.5 && self.was_playing == st.playing && self.last_key == st.key {
            return;
        }
        let key_matches = st.key == Some(self.active);
        if key_matches {
            let heard = st.position - self.last_position;
            if self.was_playing
                && heard > 0.
                && heard <= dt + 0.15
                && dt <= 30.
                && self.last_key == st.key
                && let Some(s) = self.current()
            {
                let track = CountedTrack(s.id.clone(), s.title.clone(), s.artist.clone());
                self.heard(track, heard, Local::now().naive_local());
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
