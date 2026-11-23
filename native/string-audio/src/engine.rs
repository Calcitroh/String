//! The engine: the player's logic, on its own thread (or driven by hand in tests). It opens
//! songs into the decks' feeders, tells the mixer what to do with them, asks the host what plays
//! next when a song nears its end, and makes it ready: chained on after the current song on the
//! same deck (same sample rate: exactly seamless), or on the other deck for a crossfade or a
//! gapless hand-over between rates. It keeps a loop's start ready on the other deck, and turns
//! what the mixer reports into the host's events.

use std::collections::HashMap;
use std::sync::Arc;

use crate::deck::{Feeder, UNKNOWN};
use crate::mixer::{Cmd, Evt, FLAG_NO_OUTPUT, FLAG_PLAYING, FLAG_SONG, Handover, Link, Position};
use crate::source::Source;
use crate::types::{Command, Config, Event, Host, LoopRange, Status, Track};

/// Songs that fail in a row before the engine stops trying the next one.
const MAX_FAILURES: u32 = 5;
/// A crossfade happens only between songs longer than twice it plus this (as the JS player).
const CROSSFADE_MARGIN: f64 = 4.0;
/// A chained song closer than this to being heard plays even if the queue changed.
const CHAIN_LOCK_SECS: f64 = 0.25;

struct Song {
    track: Track,
    /// The source's sample rate and playable length in its frames.
    rate: u32,
    frames: Option<u64>,
    /// The last explicit load or seek, in case the output is lost before it renders it.
    request: Option<RequestedPosition>,
}

struct RequestedPosition {
    deck: usize,
    era: u32,
    seconds: f64,
}

impl Song {
    fn secs(&self) -> Option<f64> {
        self.frames.map(|f| f as f64 / self.rate as f64)
    }
}

#[derive(Clone)]
enum Next {
    /// The host's answer, waiting to be made ready.
    Want(Track),
    /// On the current song's deck, straight after it.
    Chained { handle: u32 },
    /// Loaded and armed on the other deck.
    Armed { handle: u32 },
}

pub(crate) struct Engine {
    host: Box<dyn Host>,
    config: Config,
    feeders: [Feeder; 2],
    link: Link,
    position: Arc<Position>,
    rate: u32,
    songs: HashMap<u32, Song>,
    last_handle: u32,
    /// Decks the mixer has let go of (nothing plays or waits on them).
    free: [bool; 2],
    /// Ignore callbacks from before a deck was reloaded or sought on the engine thread.
    assignments: [u32; 2],
    /// The deck and song the engine takes as current.
    cur: Option<usize>,
    current: Option<u32>,
    playing: bool,
    asked: bool,
    next: Option<Next>,
    /// The deck holding the loop's start, armed to jump.
    loop_deck: Option<usize>,
    failures: u32,
    busy: bool,
}

impl Engine {
    pub fn new(
        host: Box<dyn Host>,
        config: Config,
        feeders: [Feeder; 2],
        link: Link,
        position: Arc<Position>,
    ) -> Engine {
        let rate = feeders[0].rate();
        let mut e = Engine {
            host,
            config,
            feeders,
            link,
            position,
            rate,
            songs: HashMap::new(),
            last_handle: 0,
            free: [true, true],
            assignments: [0; 2],
            cur: None,
            current: None,
            playing: false,
            asked: false,
            next: None,
            loop_deck: None,
            failures: 0,
            busy: false,
        };
        e.send_volume();
        e.send(Cmd::Spectrum(e.config.spectrum));
        e
    }

    fn frames(&self, secs: f64) -> u32 {
        (secs.max(0.0) * self.rate as f64).round() as u32
    }

    fn send(&mut self, cmd: Cmd) {
        // The mixer takes commands every device period, or on the engine thread while asleep.
        if self.link.cmds.push(cmd).is_ok()
            && let Some((deck, era)) = cmd.assignment()
        {
            self.assignments[deck] = era;
        }
    }

    fn send_volume(&mut self) {
        let v = if self.config.muted {
            0.0
        } else {
            self.config.volume.clamp(0.0, 1.0).powi(2)
        };
        self.send(Cmd::Volume(v));
    }

    pub fn playing(&self) -> bool {
        self.playing
    }

    /// Frames wait for room in a ring (the mixer drops stale ones soon): pump again soon.
    pub fn busy(&self) -> bool {
        self.playing && self.busy
    }

    /// Tells the host something from outside the engine (the output changed).
    pub fn notify(&mut self, event: Event) {
        self.host.event(event);
    }

    pub fn command(&mut self, cmd: Command) {
        let f = self.config.fades;
        match cmd {
            Command::Play { track, from, start } => {
                self.failures = 0;
                self.send(Cmd::Selecting);
                let fade = if from > 0.0 { self.frames(f.play) } else { 0 };
                self.play(track, from, start.then_some(fade), None);
            }
            Command::Resume => {
                if self.current.is_some() && !self.playing {
                    let fade = self.frames(f.play);
                    self.send(Cmd::Transport { play: true, fade });
                    self.playing = true;
                }
            }
            Command::Toggle => {
                self.command(if self.playing {
                    Command::Pause
                } else {
                    Command::Resume
                });
            }
            Command::RewindPaused => {
                self.playing = false;
                self.send(Cmd::Transport {
                    play: false,
                    fade: 0,
                });
                self.seek(0.);
            }
            Command::Pause => {
                if self.playing {
                    let fade = self.frames(f.pause);
                    self.send(Cmd::Transport { play: false, fade });
                    self.playing = false;
                }
            }
            Command::Stop => {
                self.stop(self.frames(f.pause));
            }
            Command::Seek(secs) => self.seek(secs),
            Command::SeekTrack { key, position } => {
                // The engine and callback can observe a handover on different turns.
                if self.song().is_some_and(|s| s.track.key == key)
                    && status(&self.position, None).key == Some(key)
                {
                    self.seek(position);
                }
            }
            Command::Volume(v) => {
                self.config.volume = v.clamp(0.0, 1.0);
                if v > 0.0 {
                    self.config.muted = false;
                }
                self.send_volume();
            }
            Command::Mute(m) => {
                self.config.muted = m;
                self.send_volume();
            }
            Command::Crossfade(secs) => {
                let was = self.config.crossfade > 0.0;
                self.config.crossfade = secs.max(0.0);
                // a next song made ready the other way is made ready again
                if was != (self.config.crossfade > 0.0)
                    && matches!(self.next, Some(Next::Chained { .. } | Next::Armed { .. }))
                {
                    self.queue_changed(true);
                }
            }
            Command::Loop(range) => self.set_loop(range),
            Command::QueueChanged => self.queue_changed(false),
            Command::Spectrum(on) => {
                self.config.spectrum = on;
                self.send(Cmd::Spectrum(on));
            }
        }
    }

    fn stop(&mut self, out: u32) {
        for d in 0..2 {
            self.send(Cmd::Release { deck: d, out });
        }
        if out == 0 {
            self.send(Cmd::Transport {
                play: false,
                fade: 0,
            });
        }
        self.playing = false;
        self.cur = None;
        self.current = None;
        self.next = None;
        self.loop_deck = None;
        self.songs.clear();
    }

    fn new_handle(&mut self) -> u32 {
        self.last_handle = self.last_handle.wrapping_add(1).max(1);
        self.last_handle
    }

    /// Plays from the requested position with the given fade-in, or cues paused.
    fn play(&mut self, track: Track, from: f64, fade_in: Option<u32>, loop_end: Option<i64>) {
        let source = match Source::open(&track.path) {
            Ok(source) => source,
            Err(error) => {
                self.failed_selection(&track, fade_in.is_some(), error);
                return;
            }
        };
        let info = source.info();
        let deck = self
            .cur
            .unwrap_or(if self.free[0] || !self.free[1] { 0 } else { 1 });
        let max = info
            .frames
            .map(|f| f.saturating_sub(info.rate as u64 / 4))
            .unwrap_or(u64::MAX);
        let from_frame = ((from.max(0.0) * info.rate as f64) as u64).min(max);
        let handle = self.new_handle();
        let era = match self.feeders[deck].load(source, handle, track.key, from_frame, false) {
            Ok(g) => g,
            Err(e) => {
                self.failed_selection(&track, fade_in.is_some(), e);
                return;
            }
        };
        let keep = self.current;
        self.failures = 0;
        self.songs.retain(|h, _| Some(*h) == keep);
        self.songs.insert(
            handle,
            Song {
                track,
                rate: info.rate,
                frames: info.frames,
                request: Some(RequestedPosition {
                    deck,
                    era,
                    seconds: from_frame as f64 / info.rate as f64,
                }),
            },
        );
        // whatever waited on the other deck belonged to the song before (the mixer lets it go)
        self.next = None;
        self.loop_deck = None;
        self.asked = false;
        let out = self.frames(self.config.fades.seek_out);
        self.send(Cmd::Replace {
            deck,
            era,
            out,
            in_: fade_in.unwrap_or(0),
            loop_rewind: loop_end.map_or(0., |end| {
                (end - self.feeders[deck].out_pos(from_frame)) as f64 / self.rate as f64
            }),
        });
        self.cur = Some(deck);
        self.current = Some(handle);
        self.free[deck] = false;
        self.playing = fade_in.is_some();
        self.send(Cmd::Transport {
            play: self.playing,
            fade: fade_in.unwrap_or(0),
        });
        self.fill();
    }

    fn failed_selection(&mut self, track: &Track, start: bool, error: crate::Error) {
        self.stop(0);
        self.host.event(Event::Failed {
            track: track.clone(),
            error: error.to_string(),
            selection: true,
        });
        // A failed paused cue never reached the end of the queue.
        if !start {
            return;
        }
        self.failures += 1;
        if self.failures < MAX_FAILURES
            && let Some(next) = self.host.next_after(track)
        {
            self.play(next, 0.0, Some(0), None);
        } else {
            self.host.event(Event::Stopped);
        }
    }

    fn song(&self) -> Option<&Song> {
        self.songs.get(&self.current?)
    }

    fn active_loop(&self) -> Option<(f64, f64)> {
        let song = self.song()?;
        let lp = song.track.loop_range?;
        let len = song.secs();
        let end = match (lp.end, len) {
            (Some(e), Some(l)) => e.min(l),
            (Some(e), None) => e,
            (None, Some(l)) => l,
            (None, None) => return None,
        };
        let start = lp.start.max(0.0);
        (end - start >= 1.).then_some((start, end))
    }

    fn seek(&mut self, secs: f64) {
        let (Some(deck), Some(h), Some(song)) = (self.cur, self.current, self.song()) else {
            return;
        };
        let (len, rate, track) = (song.secs(), song.rate, song.track.clone());
        let mut secs = secs.max(0.0);
        if let Some(len) = len {
            secs = secs.min((len - 0.25).max(0.0));
        }
        if let Some((a, b)) = self.active_loop()
            && secs >= b - 0.06
        {
            secs = a;
        }
        let frame = (secs * rate as f64) as u64;
        // a chained next song is decoded again after the seek
        if let Some(Next::Chained { handle }) = self.next.clone() {
            self.next = self.songs.get(&handle).map(|s| Next::Want(s.track.clone()));
        }
        let era = if self.feeders[deck].handle() == Some(h) {
            self.feeders[deck].restart(frame)
        } else {
            // the decoding already went on to the chained song: open this one again
            match Source::open(&track.path) {
                Ok(s) => self.feeders[deck].load(s, h, track.key, frame, false),
                Err(e) => Err(e),
            }
        };
        match era {
            Ok(era) => {
                self.songs.get_mut(&h).unwrap().request = Some(RequestedPosition {
                    deck,
                    era,
                    seconds: frame as f64 / rate as f64,
                });
                let f = self.config.fades;
                let (out, in_) = (self.frames(f.seek_out), self.frames(f.seek_in));
                self.send(Cmd::Seek {
                    deck,
                    era,
                    out,
                    in_,
                });
                self.fill();
            }
            Err(e) => self.host.event(Event::Failed {
                track,
                error: e.to_string(),
                selection: false,
            }),
        }
    }

    fn set_loop(&mut self, range: Option<LoopRange>) {
        let Some(h) = self.current else { return };
        if let Some(song) = self.songs.get_mut(&h) {
            song.track.loop_range = range;
        }
        // whatever waits (a next song, the old loop's start) is made ready again
        self.drop_next();
        if self.loop_deck.take().is_some() {
            self.send(Cmd::Disarm);
        }
        self.asked = false;
    }

    /// Lets go of the next song made ready; true if it went (it may be too close to stop).
    fn drop_next(&mut self) -> bool {
        match self.next.clone() {
            Some(Next::Armed { .. }) => self.send(Cmd::Disarm),
            Some(Next::Chained { handle }) => {
                let Some(deck) = self.cur else { return false };
                let f = &mut self.feeders[deck];
                if f.chained() == Some(handle) {
                    f.unchain();
                } else if f.joined() == Some(handle) {
                    // decoded already: drop it unless it's about to be heard
                    let st = status(&self.position, None);
                    let near = st
                        .duration
                        .is_some_and(|d| d - st.position < CHAIN_LOCK_SECS);
                    if near {
                        return false;
                    }
                    if let Some((era, in_ring)) = self.feeders[deck].break_chain()
                        && in_ring
                    {
                        self.send(Cmd::Rechain { deck, era });
                    }
                }
            }
            _ => {}
        }
        self.next = None;
        true
    }

    /// The queue changed (or the crossfade did): a next song that's no longer the answer is let
    /// go and the right one made ready.
    fn queue_changed(&mut self, redo: bool) {
        let Some(track) = self.song().map(|s| s.track.clone()) else {
            return;
        };
        if self.active_loop().is_some() {
            return;
        }
        let made = match &self.next {
            Some(Next::Armed { handle } | Next::Chained { handle }) => self
                .songs
                .get(handle)
                .map(|s| (s.track.key, s.track.path.clone())),
            Some(Next::Want(_)) | None => None,
        };
        if !self.asked && self.next.is_none() {
            return;
        }
        let want = self.host.next_after(&track);
        let same = made.is_some() && want.as_ref().map(|t| (t.key, t.path.clone())) == made;
        if same && !redo {
            return;
        }
        if self.drop_next() {
            self.next = want.map(Next::Want);
        }
    }

    /// One round of the engine's work: the mixer's news, decoding, what's due.
    pub fn pump(&mut self) {
        while let Ok((deck, assignment, e)) = self.link.events.pop() {
            // A seek may supersede a release, but the same song really did start.
            let heard_current =
                matches!(e, Evt::Started { handle } if self.current == Some(handle));
            if self.assignments[deck] == assignment || heard_current {
                self.on_mixer(deck, e);
            }
        }
        self.fill();
        self.plan();
    }

    fn fill(&mut self) {
        self.busy = false;
        for d in 0..2 {
            let mut fill = self.feeders[d].fill();
            // only the current song's end waits for the next one to be chained on
            // (a song chained ahead waits until it's current; another deck's song just ends)
            if self.feeders[d].held() && Some(d) != self.cur {
                self.feeders[d].finish();
                fill = self.feeders[d].fill();
            }
            self.busy |= fill.waiting;
            if let Some((handle, err)) = fill.failed
                && let Some(song) = self.songs.get(&handle)
            {
                let track = song.track.clone();
                self.host.event(Event::Failed {
                    track,
                    error: err.to_string(),
                    selection: false,
                });
            }
        }
    }

    fn on_mixer(&mut self, deck: usize, e: Evt) {
        match e {
            Evt::Current => {
                if self.current.is_none() {
                    return;
                }
                self.cur = Some(deck);
                self.free[deck] = false;
                if self.loop_deck == Some(deck) {
                    // the loop jumped: its start plays now; the old deck gets it next time
                    self.loop_deck = None;
                }
            }
            Evt::Started { handle } => {
                let Some(song) = self.songs.get(&handle) else {
                    return;
                };
                let track = song.track.clone();
                let was = self.current.replace(handle);
                if was != Some(handle) {
                    if let Some(Next::Armed { handle: h } | Next::Chained { handle: h }) = self.next
                        && h == handle
                    {
                        self.next = None;
                    }
                    self.asked = false;
                    self.loop_deck = None;
                    self.failures = 0;
                    let next = match self.next {
                        Some(Next::Armed { handle } | Next::Chained { handle }) => Some(handle),
                        _ => None,
                    };
                    self.songs
                        .retain(|h, _| *h == handle || Some(*h) == was || Some(*h) == next);
                }
                self.host.event(Event::Started { track });
            }
            Evt::Ended { handle } => {
                if let Some(song) = self.songs.get(&handle) {
                    let track = song.track.clone();
                    self.host.event(Event::Ended { track });
                }
            }
            Evt::Freed => {
                self.free[deck] = true;
                if self.loop_deck == Some(deck) {
                    self.loop_deck = None;
                }
                if let Some(Next::Armed { handle }) = self.next
                    && self.feeders[deck].handle() == Some(handle)
                {
                    // let go before it started: wanted again if still the answer
                    self.next = self.songs.get(&handle).map(|s| Next::Want(s.track.clone()));
                }
                if self.cur == Some(deck) {
                    self.cur = None;
                }
                self.feeders[deck].clear();
            }
            Evt::Finished { handle, end } => {
                if self.current != Some(handle) {
                    return;
                }
                let Some(track) = self.song().map(|s| s.track.clone()) else {
                    return;
                };
                if let Some((start, _)) = self.active_loop() {
                    self.play(
                        track,
                        start,
                        self.playing.then(|| self.frames(self.config.fades.loop_in)),
                        Some(end),
                    );
                    return;
                }
                // The mixer exhausted its frames before a successor was ready. Honor the
                // answer we already got (including skips past bad files and the failure cap).
                let next = match self.next.take() {
                    Some(Next::Want(t)) => Some(t),
                    Some(Next::Chained { handle } | Next::Armed { handle }) => {
                        self.songs.get(&handle).map(|s| s.track.clone())
                    }
                    None if !self.asked => self.host.next_after(&track),
                    None => None,
                };
                if let Some(next) = next {
                    self.play(next, 0.0, self.playing.then_some(0), None);
                } else {
                    self.play(track, 0.0, None, None);
                    self.host.event(Event::Stopped);
                }
            }
        }
    }

    /// What's due: the loop's start made ready, the next song asked for and made ready.
    fn plan(&mut self) {
        let (Some(c), Some(h)) = (self.cur, self.current) else {
            return;
        };
        let other = 1 - c;
        let held = self.feeders[c].held() && self.feeders[c].handle() == Some(h);
        if self.active_loop().is_some() {
            if held {
                self.feeders[c].finish();
            }
            if self.loop_deck.is_none() && self.free[other] && self.next.is_none() {
                self.arm_loop(other);
            }
            return;
        }
        if !self.asked && held {
            // decoded to its end already (a short song): what follows is wanted now
            self.asked = true;
            if let Some(track) = self.songs.get(&h).map(|s| s.track.clone()) {
                self.next = self.host.next_after(&track).map(Next::Want);
            }
        }
        if !self.asked && self.playing {
            let st = status(&self.position, None);
            let Some(song) = self.songs.get(&h) else {
                return;
            };
            let left = match (st.duration, song.secs()) {
                (Some(d), _) | (None, Some(d)) => d - st.position,
                (None, None) => f64::INFINITY,
            };
            if left <= self.config.ask_ahead && st.key == Some(song.track.key) {
                self.asked = true;
                let track = song.track.clone();
                self.host.event(Event::NearEnd {
                    track: track.clone(),
                    left,
                });
                self.next = self.host.next_after(&track).map(Next::Want);
            }
        }
        if let Some(Next::Want(t)) = self.next.clone() {
            self.make_ready(c, t);
        }
        if held {
            if self.feeders[c].chained().is_none() {
                self.feeders[c].finish();
            }
            self.fill();
        }
    }

    /// Makes the next song ready: chained on the current deck if it can, else armed on the other.
    fn make_ready(&mut self, c: usize, track: Track) {
        let Some(cur_song) = self.song() else { return };
        let (cur_key, cur_len) = (cur_song.track.key, cur_song.secs());
        let x = self.config.crossfade;
        let long = |len: Option<f64>| len.is_some_and(|l| l > 2.0 * x + CROSSFADE_MARGIN);
        // a crossfade needs the other deck free; a chain needs nothing
        let other = 1 - c;
        let wants_crossfade = x > 0.0 && track.key != cur_key && long(cur_len);
        if wants_crossfade && !self.free[other] {
            return;
        }
        let source = match Source::open(&track.path) {
            Ok(source) => source,
            Err(error) => {
                self.host.event(Event::Failed {
                    track: track.clone(),
                    error: error.to_string(),
                    selection: false,
                });
                self.failures += 1;
                self.next = if self.failures < MAX_FAILURES {
                    self.host.next_after(&track).map(Next::Want)
                } else {
                    None
                };
                return;
            }
        };
        let info = source.info();
        let next_len = info.frames.map(|f| f as f64 / info.rate as f64);
        let crossfade = wants_crossfade && (next_len.is_none() || long(next_len));
        let handle = self.new_handle();
        let song = Song {
            track: track.clone(),
            rate: info.rate,
            frames: info.frames,
            request: None,
        };
        let source = if crossfade {
            source
        } else {
            match self.feeders[c].chain(source, handle, track.key) {
                Ok(()) => {
                    self.songs.insert(handle, song);
                    self.next = Some(Next::Chained { handle });
                    return;
                }
                Err(s) => *s,
            }
        };
        // another rate, or a crossfade: the other deck
        if !self.free[other] {
            return;
        }
        let era = match self.feeders[other].load(source, handle, track.key, 0, !crossfade) {
            Ok(g) => g,
            Err(e) => {
                self.host.event(Event::Failed {
                    track,
                    error: e.to_string(),
                    selection: false,
                });
                self.next = None;
                return;
            }
        };
        self.songs.insert(handle, song);
        self.send(Cmd::Load { deck: other, era });
        let how = if crossfade {
            Handover::Crossfade {
                len: self.frames(x),
            }
        } else {
            Handover::Gapless
        };
        self.send(Cmd::Arm { deck: other, how });
        self.free[other] = false;
        self.next = Some(Next::Armed { handle });
        self.fill();
    }

    fn arm_loop(&mut self, deck: usize) {
        let (Some(h), Some((a, b))) = (self.current, self.active_loop()) else {
            return;
        };
        let Some(song) = self.songs.get(&h) else {
            return;
        };
        let (rate, key, path) = (song.rate, song.track.key, song.track.path.clone());
        let start = (a * rate as f64) as u64;
        // the loop's start, on the other deck: the same song again (its feeder may still hold it)
        let era = if self.feeders[deck].handle() == Some(h) {
            self.feeders[deck].restart(start)
        } else {
            match Source::open(&path) {
                Ok(s) => self.feeders[deck].load(s, h, key, start, false),
                Err(e) => Err(e),
            }
        };
        let Ok(era) = era else { return };
        // the jump comes just before the song's very end at the latest
        let end_out = self.feeders[deck]
            .out_len()
            .map(|e| e - 1)
            .unwrap_or(i64::MAX);
        let at = self.feeders[deck]
            .out_pos((b * rate as f64) as u64)
            .min(end_out);
        let f = self.config.fades;
        let (out, in_) = (self.frames(f.loop_out), self.frames(f.loop_in));
        self.send(Cmd::Load { deck, era });
        self.send(Cmd::Arm {
            deck,
            how: Handover::Jump { at, out, in_ },
        });
        self.free[deck] = false;
        self.loop_deck = Some(deck);
        self.fill();
    }

    /// The output's rate changed (another device): what played starts again where it was.
    pub fn resync(&mut self, rate: u32, rendered_eras: [u32; 2]) {
        let st = status(&self.position, None);
        self.rate = rate;
        for f in &mut self.feeders {
            f.set_rate(rate);
        }
        self.free = [true, true];
        self.cur = None;
        self.next = None;
        self.loop_deck = None;
        self.asked = false;
        if let Some(song) = self.song() {
            let position = song
                .request
                .as_ref()
                .filter(|r| (r.era.wrapping_sub(rendered_eras[r.deck]) as i32) > 0)
                .map_or(st.position, |r| r.seconds);
            self.play(
                song.track.clone(),
                position,
                self.playing.then(|| self.frames(self.config.fades.play)),
                None,
            );
        }
    }
}

/// Reads the published position. `now_ns`: the player's clock, to move a playing position on
/// from when it was published (live output only).
pub(crate) fn status(position: &Position, now_ns: Option<u64>) -> Status {
    let mut w = [0u64; 7];
    position.read(&mut w);
    let rate = w[4].max(1) as f64;
    if w[1] & FLAG_SONG == 0 {
        return Status::default();
    }
    let playing = w[1] & FLAG_PLAYING != 0;
    let end = w[3] as i64;
    let duration = (end != UNKNOWN).then(|| end as f64 / rate);
    let mut position = w[2] as f64 / rate;
    if let Some(now) = now_ns
        && playing
        && w[1] & FLAG_NO_OUTPUT == 0
    {
        position += (now as f64 - w[5] as f64) / 1e9;
    }
    if let Some(d) = duration {
        position = position.min(d);
    }
    Status {
        key: Some(w[0]),
        position: position.max(0.0),
        loop_offset: f64::from_bits(w[6]),
        duration,
        playing,
    }
}
