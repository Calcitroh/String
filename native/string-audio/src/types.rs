//! What the app hands the engine and hears back: songs, loops, settings, events, and the `Host`
//! the engine asks what plays next. The queue lives in the app (string-core); the engine only
//! asks, and says what really started and ended.

use std::path::PathBuf;

/// A song to play. `key` is the app's own number for this place in its queue (the engine never
/// reads it; it comes back in events and in `Status`); the same key twice in a row means the
/// same place again (Repeat one), which plays gapless and never crossfades.
#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub key: u64,
    pub path: PathBuf,
    /// An A–B loop remembered for this song, played from the start.
    pub loop_range: Option<LoopRange>,
}

impl Track {
    pub fn new(key: u64, path: impl Into<PathBuf>) -> Track {
        Track {
            key,
            path: path.into(),
            loop_range: None,
        }
    }

    pub fn with_loop(mut self, range: Option<LoopRange>) -> Track {
        self.loop_range = range;
        self
    }
}

/// A stretch of one song played over and over: from `start` seconds to `end` (or the song's
/// end). A stretch shorter than one second counts as no loop.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoopRange {
    pub start: f64,
    pub end: Option<f64>,
}

/// The engine's settings. Every length is in seconds.
#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    /// Crossfade between songs (0: gapless).
    pub crossfade: f64,
    /// Volume 0..1 (heard as its square, as in the JS player) and mute.
    pub volume: f32,
    pub muted: bool,
    pub fades: Fades,
    /// How long before a song's end the engine asks what plays next.
    pub ask_ahead: f64,
    /// Measure the spectrum from the start.
    pub spectrum: bool,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            crossfade: 0.0,
            volume: 1.0,
            muted: false,
            fades: Fades::default(),
            ask_ahead: 20.0,
            spectrum: false,
        }
    }
}

/// The short fades, like the JS player's ramps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fades {
    /// In on play (JS: 0.22).
    pub play: f64,
    /// Out on pause (JS: 0.16).
    pub pause: f64,
    /// Out before a seek or a song change: short, so the jump answers at once.
    pub seek_out: f64,
    /// In after a seek.
    pub seek_in: f64,
    /// A loop's dip: out to the loop's end (JS: 0.04), in from its start (JS: 0.08).
    pub loop_out: f64,
    pub loop_in: f64,
}

impl Default for Fades {
    fn default() -> Fades {
        Fades {
            play: 0.22,
            pause: 0.16,
            seek_out: 0.06,
            seek_in: 0.16,
            loop_out: 0.04,
            loop_in: 0.08,
        }
    }
}

/// What happened, told to the `Host` on the engine's thread.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// This song is heard now: it began, or the last one handed over to it.
    Started { track: Track },
    /// This song played to its last frame.
    Ended { track: Track },
    /// The end is near: the engine is about to ask what follows (`left` seconds remain).
    NearEnd { track: Track, left: f64 },
    /// This song couldn't be opened, or broke while playing.
    Failed { track: Track, error: String },
    /// The last song ended and nothing followed.
    Stopped,
    /// Sound goes to this output now (at the start, and after the device changed).
    Output { name: String, rate: u32 },
}

/// The app's side of the engine: the queue's answer to "what plays next", and the events.
/// Called on the engine's own thread; keep both quick.
pub trait Host: Send + 'static {
    /// What plays after `current`. Asked near each song's end, and again after
    /// `queue_changed`; it must not change anything (the answer may be dropped).
    fn next_after(&mut self, current: &Track) -> Option<Track>;
    /// Something happened.
    fn event(&mut self, event: Event);
}

/// The player as the screens see it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Status {
    /// The current song's key.
    pub key: Option<u64>,
    /// Seconds into it.
    pub position: f64,
    /// Cumulative seconds the mixer actually jumped back at loop boundaries.
    pub loop_offset: f64,
    /// Its length, when known.
    pub duration: Option<f64>,
    pub playing: bool,
}

/// A command for the engine.
#[derive(Clone, Debug)]
pub(crate) enum Command {
    Play {
        track: Track,
        from: f64,
        start: bool,
    },
    Resume,
    Pause,
    Stop,
    Seek(f64),
    Volume(f32),
    Mute(bool),
    Crossfade(f64),
    Loop(Option<LoopRange>),
    QueueChanged,
    Spectrum(bool),
}
