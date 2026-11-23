//! The sound thread's side: two decks read from their rings and mixed, frame-exact. A deck is
//! the current song, an incoming one waiting for its moment, or an outgoing one fading or
//! playing out its last frames. Hand-overs happen on the exact frame: gapless (the next song's
//! resampler lead-in overlapping the last song's ringing, so the joint is seamless even when
//! resampling), an equal-power crossfade ending on the last song's last frame, or a loop's jump
//! (a 40 ms dip out to the loop's end, then in from its start, as the JS player did). Seeks and
//! song changes fade out and in on the same deck. Nothing here allocates, locks or waits.

use std::sync::Arc;

use crate::deck::{Marker, MarkerKind, UNKNOWN};
use crate::envelope::{Envelope, Shape};
use crate::shared::SeqLock;
use crate::spectrum::{Analyser, Published};

/// How a waiting deck takes over from the current one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Handover {
    /// Its first frame follows the current song's last one.
    Gapless,
    /// An equal-power crossfade of `len` frames, ending on the current song's last frame.
    Crossfade { len: u32 },
    /// At position `at` of the current song: out over `out` frames (ending at `at`), then the
    /// waiting deck in over `in_` frames. A loop's jump.
    Jump { at: i64, out: u32, in_: u32 },
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Cmd {
    /// Silence the old selection if opening its replacement takes another callback.
    Selecting,
    /// An idle deck holds era `era` now (a song made ready, not playing).
    Load { deck: usize, era: u32 },
    /// `deck` plays era `era` (another song) as the current song; what played fades out over
    /// `out` frames, the new one comes in over `in_` (0: at full level).
    Replace {
        deck: usize,
        era: u32,
        out: u32,
        in_: u32,
    },
    /// A song chained on after the current one on `deck` was dropped: when the deck gets
    /// there, it skips to era `era`.
    Rechain { deck: usize, era: u32 },
    /// The current song's deck jumps to generation `era` (a seek), out and in.
    Seek {
        deck: usize,
        era: u32,
        out: u32,
        in_: u32,
    },
    /// When the time comes, `deck` takes over from the current song.
    Arm { deck: usize, how: Handover },
    /// The armed deck won't take over after all (if it hasn't started).
    Disarm,
    /// `deck` stops (fading out over `out` frames if it plays).
    Release { deck: usize, out: u32 },
    /// Play or pause, fading over `fade` frames.
    Transport { play: bool, fade: u32 },
    /// The output gain (volume squared, 0 when muted).
    Volume(f32),
    /// Measure the spectrum or not.
    Spectrum(bool),
}

impl Cmd {
    pub fn assignment(self) -> Option<(usize, u32)> {
        match self {
            Self::Load { deck, era }
            | Self::Seek { deck, era, .. }
            | Self::Replace { deck, era, .. } => Some((deck, era)),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Evt {
    /// `deck` holds the current song now.
    Current,
    /// Song `handle` is heard: it began, or took over.
    Started { handle: u32 },
    /// Song `handle` played to its last frame.
    Ended { handle: u32 },
    /// `deck` is silent and nothing is armed on it: the engine may load it.
    Freed,
    /// The current song ran out and nothing followed.
    Finished { handle: u32 },
}

/// What the screens read about the current song: key, flags, position, end, rate, the time the
/// position was heard (ns), and the accumulated loop rewinds (seconds).
pub(crate) type Position = SeqLock<7>;
pub(crate) const FLAG_SONG: u64 = 1;
pub(crate) const FLAG_PLAYING: u64 = 2;
pub(crate) const FLAG_NO_OUTPUT: u64 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Idle,
    Current,
    /// Fading to silence; let go when it gets there.
    Fading,
    /// A gapless outgoing song past its end: its resampler ringing, at full level.
    Tail,
    /// A gapless incoming song inside its lead-in, before the current song's end.
    Leading,
}

#[derive(Clone, Copy, Debug)]
struct Pending {
    era: u32,
    in_: u32,
}

struct Deck {
    /// Load or seek command this deck last accepted, including a seek still fading out.
    assignment: u32,
    samples: rtrb::Consumer<f32>,
    markers: rtrb::Consumer<Marker>,
    read: u64,
    /// Frames of older generations are dropped unread.
    min_era: u32,
    era: u32,
    begun: bool,
    stopped: bool,
    pos: i64,
    handle: u32,
    key: u64,
    end: Option<i64>,
    est_end: Option<i64>,
    ended: bool,
    /// A song that reached its end in the last settle, to report.
    ended_now: Option<u32>,
    /// Eras below this, past the current one, are skipped (a dropped chained song).
    floor: u32,
    role: Role,
    gain: Envelope,
    /// A generation to switch to once the fade out is done.
    pending: Option<Pending>,
    /// The gain to start the next generation with: fade in over this many frames.
    fade_in: Option<u32>,
}

impl Deck {
    fn new(samples: rtrb::Consumer<f32>, markers: rtrb::Consumer<Marker>) -> Deck {
        Deck {
            assignment: 0,
            samples,
            markers,
            read: 0,
            min_era: 0,
            era: 0,
            begun: false,
            stopped: false,
            pos: 0,
            handle: 0,
            key: 0,
            end: None,
            est_end: None,
            ended: false,
            ended_now: None,
            floor: 0,
            role: Role::Idle,
            gain: Envelope::steady(1.0),
            pending: None,
            fade_in: None,
        }
    }

    fn audible(&self) -> bool {
        self.role != Role::Idle
    }

    /// Takes in the markers at the read position, drops stale frames, and returns how many
    /// frames of the current generation can be read now.
    fn settle(&mut self) -> usize {
        loop {
            // the sample count first: markers for every frame counted are visible after it
            let avail = self.samples.slots() / 2;
            while let Ok(m) = self.markers.peek() {
                let m = *m;
                if matches!(m.kind, MarkerKind::Begin { .. })
                    && m.era > self.era
                    && m.era < self.floor
                {
                    self.min_era = self.min_era.max(self.floor);
                }
                if m.era < self.min_era {
                    let _ = self.markers.pop();
                    continue;
                }
                if m.at > self.read {
                    break;
                }
                if let Some(p) = self.pending
                    && matches!(m.kind, MarkerKind::Begin { .. })
                    && m.era >= p.era
                {
                    // The old audio ran out before its seek fade did. Apply the switch
                    // before consuming the only Begin marker for the requested position.
                    self.switch_to(p.era, p.in_);
                }
                let _ = self.markers.pop();
                match m.kind {
                    MarkerKind::Begin {
                        pos,
                        handle,
                        key,
                        end,
                    } => {
                        self.era = m.era;
                        self.begun = true;
                        self.stopped = false;
                        self.pos = pos;
                        if handle != self.handle {
                            self.ended = false;
                        }
                        self.handle = handle;
                        self.key = key;
                        self.end = None;
                        self.est_end = (end != UNKNOWN).then_some(end);
                        if let Some(f) = self.fade_in.take() {
                            if f == 0 {
                                self.gain.set(1.0);
                            } else {
                                self.gain.set(0.0);
                                self.gain.ramp(1.0, f, Shape::Linear);
                            }
                        }
                    }
                    MarkerKind::End if m.era == self.era => {
                        self.end = Some(self.pos);
                        if !self.ended {
                            self.ended = true;
                            self.ended_now = Some(self.handle);
                        }
                    }
                    MarkerKind::Stop if m.era == self.era => self.stopped = true,
                    _ => {}
                }
            }
            if self.begun && self.era >= self.min_era {
                if self.stopped {
                    return 0;
                }
                let limit = match self.markers.peek() {
                    Ok(m) => (m.at - self.read) as usize,
                    Err(_) => usize::MAX,
                };
                return avail.min(limit);
            }
            // stale frames: drop them up to the next marker that counts
            let limit = match self.markers.peek() {
                Ok(m) => m.at.saturating_sub(self.read) as usize,
                Err(_) => avail,
            };
            let skip = avail.min(limit);
            if skip == 0 {
                return 0;
            }
            if let Ok(chunk) = self.samples.read_chunk(skip * 2) {
                chunk.commit_all();
            }
            self.read += skip as u64;
        }
    }

    /// Where this song's last frame ends, once the feeder has said.
    fn end_pos(&self) -> Option<i64> {
        if self.end.is_some() {
            return self.end;
        }
        match self.markers.peek() {
            Ok(m) if m.era == self.era && m.kind == MarkerKind::End => {
                Some(self.pos + (m.at - self.read) as i64)
            }
            _ => None,
        }
    }

    /// Reads `n` frames and adds them, at this deck's gain, into `out`.
    fn mix(&mut self, n: usize, out: &mut [f32]) {
        if let Ok(chunk) = self.samples.read_chunk(n * 2) {
            let (a, b) = chunk.as_slices();
            self.gain.mix(a, &mut out[..a.len()]);
            self.gain.mix(b, &mut out[a.len()..a.len() + b.len()]);
            chunk.commit_all();
        }
        self.read += n as u64;
        self.pos += n as i64;
    }

    fn switch_to(&mut self, era: u32, fade_in: u32) {
        self.min_era = self.min_era.max(era);
        self.begun = false;
        self.stopped = false;
        self.end = None;
        self.ended = false;
        self.ended_now = None;
        self.fade_in = Some(fade_in);
        self.pending = None;
    }
}

#[derive(Clone, Copy, Debug)]
struct Armed {
    deck: usize,
    how: Handover,
    /// Gapless: the incoming deck started early (in its lead-in); the switch comes at this
    /// position of the current song. Jump: the dip out has begun.
    started: Option<i64>,
}

pub(crate) struct Mixer {
    rate: u32,
    decks: [Deck; 2],
    cmds: rtrb::Consumer<Cmd>,
    events: rtrb::Producer<(usize, u32, Evt)>,
    cur: Option<usize>,
    armed: Option<Armed>,
    /// The song last reported as started.
    started: u32,
    loop_offset: f64,
    transport: Envelope,
    running: bool,
    /// When the last frame mixed for a device will have played (on the player's clock).
    audible_until: u64,
    volume: f32,
    volume_target: f32,
    volume_k: f32,
    analyser: Analyser,
    spectrum_on: bool,
    position: Arc<Position>,
}

/// The engine's ends of the mixer's command and event rings.
pub(crate) struct Link {
    pub cmds: rtrb::Producer<Cmd>,
    pub events: rtrb::Consumer<(usize, u32, Evt)>,
}

impl Mixer {
    pub fn new(
        rate: u32,
        decks: [(rtrb::Consumer<f32>, rtrb::Consumer<Marker>); 2],
        spectrum: Arc<Published>,
        position: Arc<Position>,
    ) -> (Mixer, Link) {
        let (cp, cc) = rtrb::RingBuffer::new(256);
        let (ep, ec) = rtrb::RingBuffer::new(256);
        let [a, b] = decks;
        let mixer = Mixer {
            rate,
            decks: [Deck::new(a.0, a.1), Deck::new(b.0, b.1)],
            cmds: cc,
            events: ep,
            cur: None,
            armed: None,
            started: 0,
            loop_offset: 0.,
            transport: Envelope::steady(1.0),
            running: false,
            audible_until: 0,
            volume: 1.0,
            volume_target: 1.0,
            volume_k: volume_k(rate),
            analyser: Analyser::new(rate, spectrum),
            spectrum_on: false,
            position,
        };
        (
            mixer,
            Link {
                cmds: cp,
                events: ec,
            },
        )
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    pub fn drained(&self, now_ns: u64) -> bool {
        (!self.running || self.decks.iter().all(|d| !d.audible())) && now_ns >= self.audible_until
    }

    /// A new output rate (the engine reloads what plays).
    pub fn set_rate(&mut self, rate: u32) {
        self.rate = rate;
        self.volume_k = volume_k(rate);
        self.analyser.set_rate(rate);
    }

    /// The old stream has stopped. Keep the last rendered position, and freeze interpolation
    /// until the next device publishes a frame. Transport intent still belongs to the engine.
    pub fn output_waiting(&mut self) {
        let mut snapshot = [0; 7];
        self.position.read(&mut snapshot);
        snapshot[1] |= FLAG_PLAYING | FLAG_NO_OUTPUT;
        self.position.write(&snapshot);
    }

    /// No audio is being rendered. Complete a pause or seek that was waiting for its fade
    /// so the replacement device cannot play the tail of a song paused during the outage.
    pub fn settle_pause(&mut self) {
        self.commands();
        self.running = false;
        self.transport.set(0.0);
        for deck in &mut self.decks {
            if let Some(pending) = deck.pending.take() {
                deck.switch_to(pending.era, 0);
            }
        }
    }

    /// Before a new device starts, discard buffered frames from retired loads so the
    /// feeder has space for the first real callback, even after a sample-rate change.
    pub fn prime(&mut self) {
        self.commands();
        for deck in &mut self.decks {
            deck.settle();
        }
    }

    /// Forgets every deck's state (the output's rate changed; the engine loads again).
    pub fn reset(&mut self) -> [u32; 2] {
        for d in &mut self.decks {
            d.role = Role::Idle;
            d.pending = None;
            d.fade_in = None;
            d.gain.set(1.0);
            d.begun = false;
            d.min_era = d.era.wrapping_add(1);
        }
        self.cur = None;
        self.armed = None;
        self.started = 0;
        self.decks.each_ref().map(|deck| deck.era)
    }

    fn emit(&mut self, deck: usize, e: Evt) {
        let _ = self.events.push((deck, self.decks[deck].assignment, e));
    }

    fn release(&mut self, d: usize) {
        let deck = &mut self.decks[d];
        deck.role = Role::Idle;
        deck.pending = None;
        deck.gain.set(1.0);
        if self.cur == Some(d) {
            self.cur = None;
        }
        if self.armed.is_some_and(|a| a.deck == d) {
            self.armed = None;
        }
        self.emit(d, Evt::Freed);
    }

    fn set_current(&mut self, d: usize) {
        if self.cur != Some(d) {
            self.cur = Some(d);
            self.emit(d, Evt::Current);
        }
    }

    /// Fades out every audible deck but `keep`, and drops an armed hand-over that hasn't begun.
    fn quiet_others(&mut self, keep: usize, out: u32) {
        if let Some(a) = self.armed.take()
            && a.deck != keep
            && self.decks[a.deck].role == Role::Idle
        {
            self.emit(a.deck, Evt::Freed);
        }
        let o = 1 - keep;
        if self.decks[o].audible() {
            if self.running && out > 0 {
                self.decks[o].role = Role::Fading;
                self.decks[o].pending = None;
                self.decks[o].gain.ramp(0.0, out, Shape::Linear);
            } else {
                self.release(o);
            }
        }
    }

    fn commands(&mut self) {
        let mut selecting = false;
        while let Ok(cmd) = self.cmds.pop() {
            if let Some((deck, era)) = cmd.assignment() {
                self.decks[deck].assignment = era;
            }
            match cmd {
                Cmd::Selecting => selecting = true,
                Cmd::Load { deck, era } => {
                    if self.armed.is_some_and(|a| a.deck == deck) {
                        self.armed = None;
                    }
                    let d = &mut self.decks[deck];
                    d.switch_to(era, 0);
                    d.fade_in = None;
                    d.gain.set(1.0);
                    d.role = Role::Idle;
                    if self.cur == Some(deck) {
                        self.cur = None;
                    }
                }
                Cmd::Replace {
                    deck,
                    era,
                    out,
                    in_,
                } => {
                    selecting = false;
                    self.quiet_others(deck, out);
                    self.change(deck, era, out, in_);
                    self.set_current(deck);
                }
                Cmd::Rechain { deck, era } => self.decks[deck].floor = era,
                Cmd::Seek {
                    deck,
                    era,
                    out,
                    in_,
                } => {
                    self.end_crossfade();
                    if self.cur != Some(deck) {
                        // the song moved on to the other deck just now: back to this one
                        self.quiet_others(deck, out);
                    } else if let Some(a) = self.armed
                        && a.started.is_some()
                        && a.how == Handover::Gapless
                    {
                        // a gapless hand-over under way: the incoming song goes quiet again
                        self.armed = None;
                        let o = a.deck;
                        self.decks[o].role = Role::Fading;
                        self.decks[o].gain.ramp(0.0, out.max(1), Shape::Linear);
                    }
                    self.change(deck, era, out, in_);
                    self.set_current(deck);
                }
                Cmd::Arm { deck, how } => {
                    if self.decks[deck].role == Role::Idle && self.cur != Some(deck) {
                        self.armed = Some(Armed {
                            deck,
                            how,
                            started: None,
                        });
                    }
                }
                Cmd::Disarm => {
                    if let Some(a) = self.armed.take() {
                        if a.started.is_some()
                            && matches!(a.how, Handover::Jump { .. })
                            && let Some(c) = self.cur
                        {
                            self.decks[c].gain.set(1.0);
                        }
                        self.release(a.deck);
                    }
                }
                Cmd::Release { deck, out } => {
                    if self.armed.is_some_and(|a| a.deck == deck) {
                        self.armed = None;
                    }
                    let d = &mut self.decks[deck];
                    if d.audible() && self.running && out > 0 {
                        d.role = Role::Fading;
                        d.pending = None;
                        d.gain.ramp(0.0, out, Shape::Linear);
                        if self.cur == Some(deck) {
                            self.cur = None;
                        }
                    } else {
                        self.release(deck);
                    }
                }
                Cmd::Transport { play, fade } => {
                    if play {
                        if !self.running {
                            self.transport.set(0.0);
                        }
                        self.running = true;
                        self.transport.ramp(1.0, fade, Shape::Linear);
                    } else if self.running {
                        self.end_crossfade();
                        self.transport.ramp(0.0, fade, Shape::Linear);
                        if fade == 0 {
                            self.running = false;
                        }
                    }
                }
                Cmd::Volume(v) => {
                    self.volume_target = v;
                    if !self.running {
                        // nothing sounds: no need to glide (and the first song starts at its level)
                        self.volume = v;
                    }
                }
                Cmd::Spectrum(on) => self.spectrum_on = on,
            }
        }
        if selecting {
            // A quick successful open uses the normal replacement fade. A slow open
            // must not leave the previous song playing while the engine waits for I/O.
            self.running = false;
            self.transport.set(0.0);
        }
    }

    fn end_crossfade(&mut self) {
        if let Some(c) = self.cur
            && self.decks[1 - c].role == Role::Fading
        {
            self.release(1 - c);
            self.decks[c].gain.set(1.0);
        }
    }

    /// The deck's content becomes generation `era`: after a fade out if it's playing.
    fn change(&mut self, deck: usize, era: u32, out: u32, in_: u32) {
        let running = self.running;
        let d = &mut self.decks[deck];
        let playing = d.audible() && d.begun && !d.stopped;
        d.role = Role::Current;
        if running && playing && out > 0 {
            d.gain.ramp(0.0, out, Shape::Linear);
            d.pending = Some(Pending { era, in_ });
        } else {
            d.switch_to(era, if running { in_ } else { 0 });
        }
    }

    /// Renders stereo frames into `out`. `heard_at` is when its first frame will be heard (ns on
    /// the player's clock).
    pub fn render(&mut self, out: &mut [f32], heard_at: u64) {
        self.commands();
        out.fill(0.0);
        if !self.running {
            // paused: no frames move, but a seek's or a new song's position shows
            self.decks[0].settle();
            self.decks[1].settle();
        }
        let frames = out.len() / 2;
        let mut done = 0;
        let mut spins = 0;
        while done < frames && self.running && spins < 64 {
            spins += 1;
            let ready = [self.decks[0].settle(), self.decks[1].settle()];
            if self.act(ready) {
                continue;
            }
            let n = self.run_length(ready, frames - done);
            if n == 0 {
                // An empty ring renders silence. A pause still finishes on device time.
                if self.transport.target() == 0.0 {
                    self.transport.advance(frames - done);
                    self.running = self.transport.moving();
                }
                break;
            }
            spins = 0;
            let slice = &mut out[done * 2..(done + n) * 2];
            for (i, deck) in self.decks.iter_mut().enumerate() {
                if deck.audible() && ready[i] > 0 {
                    deck.mix(n, slice);
                }
            }
            if self.transport.moving() || self.transport.target() != 1.0 {
                self.transport.scale(slice);
                if !self.transport.moving() && self.transport.target() == 0.0 {
                    self.running = false;
                }
            }
            done += n;
        }
        if self.spectrum_on {
            self.analyser.feed(out);
        }
        if done > 0 {
            self.audible_until = heard_at + done as u64 * 1_000_000_000 / self.rate as u64;
        }
        self.apply_volume(out);
        self.publish(heard_at + frames as u64 * 1_000_000_000 / self.rate as u64);
    }

    /// Does whatever is due at the decks' positions now. True if something changed (the decks
    /// are settled again before mixing).
    fn act(&mut self, ready: [usize; 2]) -> bool {
        // seeks and song changes whose fade out is done
        for d in 0..2 {
            let deck = &mut self.decks[d];
            if let Some(p) = deck.pending
                && (!deck.gain.moving() || deck.stopped || !self.running)
            {
                deck.switch_to(p.era, p.in_);
                return true;
            }
        }
        // songs that reached their end
        for d in 0..2 {
            if let Some(handle) = self.decks[d].ended_now.take() {
                self.emit(d, Evt::Ended { handle });
            }
        }
        // outgoing decks that are done
        for d in 0..2 {
            let deck = &self.decks[d];
            let faded =
                deck.role == Role::Fading && (!deck.gain.moving() && deck.gain.target() == 0.0);
            let over = matches!(deck.role, Role::Fading | Role::Tail) && deck.begun && deck.stopped;
            let stale = deck.role == Role::Fading && deck.pending.is_none() && !deck.begun;
            if faded || over || stale {
                self.release(d);
                return true;
            }
        }
        let Some(c) = self.cur else { return false };
        // the current deck's song is heard: report it once
        if self.decks[c].begun
            && self.decks[c].handle != self.started
            && self.decks[c].pending.is_none()
        {
            self.started = self.decks[c].handle;
            let handle = self.started;
            self.emit(c, Evt::Started { handle });
        }
        // A seek or replacement owns this deck. Its outgoing frames can reach EOF
        // during the short fade; never hand over based on that discarded position.
        if self.decks[c].pending.is_some() {
            return false;
        }
        if let Some(a) = self.armed {
            let o = a.deck;
            let other_ready = self.decks[o].begun && ready[o] > 0;
            let cd = &self.decks[c];
            let pos = cd.pos;
            match a.how {
                Handover::Gapless => {
                    if let Some(switch_at) = a.started {
                        if pos >= switch_at || cd.stopped {
                            self.decks[o].role = Role::Current;
                            self.decks[c].role = if self.decks[c].stopped {
                                Role::Idle
                            } else {
                                Role::Tail
                            };
                            self.armed = None;
                            if self.decks[c].role == Role::Idle {
                                self.release(c);
                            }
                            self.set_current(o);
                            return true;
                        }
                    } else if other_ready {
                        let lead = self.decks[o].pos.min(0);
                        let end = cd.end_pos();
                        if cd.stopped || end.is_some_and(|e| pos >= e + lead) {
                            let switch_at = end.unwrap_or(pos);
                            self.decks[o].role = Role::Leading;
                            self.decks[o].gain.set(1.0);
                            self.armed = Some(Armed {
                                started: Some(switch_at),
                                ..a
                            });
                            return true;
                        }
                    }
                }
                Handover::Crossfade { len } => {
                    let end = cd.end_pos().or(cd.est_end);
                    let Some(end) = end else {
                        self.armed = Some(Armed {
                            how: Handover::Gapless,
                            ..a
                        });
                        return true;
                    };
                    if cd.stopped || pos >= end - len as i64 {
                        let left = end - pos;
                        if left < (self.rate as i64 * 2) / 5 {
                            // too late for a crossfade worth hearing
                            self.armed = Some(Armed {
                                how: Handover::Gapless,
                                ..a
                            });
                            return true;
                        }
                        if other_ready {
                            let left = left as u32;
                            self.decks[o].role = Role::Current;
                            self.decks[o].gain.set(0.0);
                            self.decks[o].gain.ramp(1.0, left, Shape::SineIn);
                            self.decks[c].role = Role::Fading;
                            self.decks[c].pending = None;
                            self.decks[c].gain.ramp(0.0, left, Shape::CosineOut);
                            self.armed = None;
                            self.set_current(o);
                            return true;
                        }
                    }
                }
                Handover::Jump { at, out, in_ } => {
                    if a.started.is_none() && !cd.stopped && pos >= at - out as i64 {
                        // the dip out ends at `at`, or `out` from now if that's already passed
                        let len = if pos < at {
                            ((at - pos) as u32).clamp(1, out.max(1))
                        } else {
                            out.max(1)
                        };
                        self.decks[c].gain.ramp(0.0, len, Shape::Linear);
                        self.armed = Some(Armed {
                            started: Some(pos + len as i64),
                            ..a
                        });
                        return true;
                    }
                    let due = match a.started {
                        Some(switch_at) => pos >= switch_at,
                        None => false,
                    };
                    if due || cd.stopped {
                        self.loop_offset += (pos - self.decks[o].pos) as f64 / self.rate as f64;
                        self.release(c);
                        let od = &mut self.decks[o];
                        od.role = Role::Current;
                        if in_ > 0 {
                            od.gain.set(0.0);
                            od.gain.ramp(1.0, in_, Shape::Linear);
                        } else {
                            od.gain.set(1.0);
                        }
                        self.armed = None;
                        self.set_current(o);
                        return true;
                    }
                }
            }
        }
        // the current song ran out and nothing takes over
        let cd = &self.decks[c];
        if cd.begun && cd.stopped && cd.pending.is_none() && self.armed.is_none() {
            let handle = cd.handle;
            self.release(c);
            // Only the engine knows whether the queue ended. Keep transport intent
            // through its decision, including a seek racing this final callback.
            self.emit(c, Evt::Finished { handle });
            return true;
        }
        false
    }

    /// How many frames can be mixed before something is due.
    fn run_length(&self, ready: [usize; 2], mut n: usize) -> usize {
        let mut any = false;
        for (i, d) in self.decks.iter().enumerate() {
            if !d.audible() || ready[i] == 0 {
                continue;
            }
            any = true;
            n = n.min(ready[i]);
            if let Some(e) = d.end_pos()
                && d.pos < e
            {
                n = n.min((e - d.pos) as usize);
            }
            if d.gain.moving() && (d.role == Role::Fading || d.pending.is_some()) {
                n = n.min(d.gain.left() as usize);
            }
        }
        if !any {
            return 0;
        }
        if let (Some(c), Some(a)) = (self.cur, self.armed) {
            let cd = &self.decks[c];
            let pos = cd.pos;
            let until = |t: i64| {
                if t > pos {
                    Some((t - pos) as usize)
                } else {
                    None
                }
            };
            let mark = match a.how {
                Handover::Gapless => match a.started {
                    Some(s) => until(s),
                    None => cd
                        .end_pos()
                        .and_then(|e| until(e + self.decks[a.deck].pos.min(0))),
                },
                Handover::Crossfade { len } => cd
                    .end_pos()
                    .or(cd.est_end)
                    .and_then(|e| until(e - len as i64)),
                Handover::Jump { at, out, .. } => match a.started {
                    Some(switch_at) => until(switch_at),
                    None => until(at - out as i64),
                },
            };
            if let Some(m) = mark {
                n = n.min(m);
            }
        }
        if self.transport.moving() {
            n = n.min(self.transport.left() as usize);
        }
        n
    }

    fn apply_volume(&mut self, out: &mut [f32]) {
        if (self.volume - self.volume_target).abs() < 1e-5 {
            self.volume = self.volume_target;
            if self.volume != 1.0 {
                out.iter_mut().for_each(|v| *v *= self.volume);
            }
            return;
        }
        for f in out.as_chunks_mut::<2>().0 {
            self.volume += (self.volume_target - self.volume) * self.volume_k;
            f[0] *= self.volume;
            f[1] *= self.volume;
        }
    }

    fn publish(&self, heard_at: u64) {
        let mut w = [0u64; 7];
        if let Some(c) = self.cur {
            let d = &self.decks[c];
            if d.begun {
                w[0] = d.key;
                w[1] = FLAG_SONG
                    | if self.running && self.transport.target() == 1.0 {
                        FLAG_PLAYING
                    } else {
                        0
                    };
                w[2] = d.pos.max(0) as u64;
                w[3] = d.end_pos().or(d.est_end).unwrap_or(UNKNOWN) as u64;
                w[6] = self.loop_offset.to_bits();
            }
        }
        w[4] = self.rate as u64;
        w[5] = heard_at;
        self.position.write(&w);
    }
}

/// The volume follows its target with a 30 ms time constant (as the JS player's
/// `setTargetAtTime(v, t, 0.03)`).
fn volume_k(rate: u32) -> f32 {
    1.0 - (-1.0 / (0.03 * rate as f32)).exp()
}
