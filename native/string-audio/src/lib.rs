//! String's sound: decoding, two decks, gapless hand-over, crossfade, fades, loops and the spectrum.
//!
//! [`Player`] plays songs from the app's queue on the default sound device: the app starts one
//! with [`Player::play_track`], and the engine asks the app's [`Host`] what follows each song and
//! makes it ready so it starts on the exact frame after the last (or crossfades into it).
//! [`OfflinePlayer`] is the same engine rendering into memory, for tests and tools. [`Spectrum`]
//! is the visualizer's feed: 64 bands, cheap to read on every frame.
//!
//! # How it works
//!
//! - **Audio threads.** The app's calls post a command and return. A worker opens the next
//!   source while the engine continues to handle transport commands. The engine thread decodes
//!   (symphonia), resamples to the device's rate (rubato) and fills each deck's lock-free ring
//!   about 1.5 s ahead; it asks the host what's next and arms hand-overs. The sound device's
//!   thread runs the mixer: it reads the rings, applies fades and hand-overs on the exact frame,
//!   measures the spectrum and publishes the position. It never allocates, locks or waits.
//! - **Two decks.** The next song at the same sample rate is chained on the current deck,
//!   through the same resampler: the joint is exactly what one long recording would give. The
//!   other deck takes a crossfading song, a song at another rate (a gapless hand-over that
//!   overlaps the resampler's ringing), or a loop's start (the jump is a deck switch with the JS
//!   player's 40 ms dip out and 80 ms in).
//! - **Eras.** Every load or seek starts a new era on a deck; markers in the ring say where each
//!   era begins (at which position of which song), where a song ends and where frames stop, so
//!   the mixer drops stale frames and knows every frame's place without asking anyone.
//!
//! # What the app gives the engine
//!
//! - A [`Host`]: `next_after(current)` answers from the queue without changing it (Repeat, the
//!   shuffle order and the autoplay continuation all live there), and `event` hears what really
//!   started ([`Event::Started`]: move the queue's cursor, count the play, add to history), what
//!   ended, what failed (mark it unplayable), [`Event::NearEnd`] and [`Event::Stopped`].
//! - [`Track`] keys unique per place in the queue (the same key twice in a row is Repeat one),
//!   and the song's remembered loop in [`Track::loop_range`].
//! - [`Player::queue_changed`] after any edit to what comes next, [`Player::set_loop`] when the
//!   current song's loop changes.
//!
//! The screens read [`Player::status`] (song key, position, length, playing) and
//! [`Spectrum::read`] once per frame, and switch the spectrum off with [`Player::set_spectrum`]
//! while no visualizer shows.

mod deck;
mod engine;
mod envelope;
mod mixer;
mod mp4;
mod offline;
mod output;
mod player;
mod resample;
mod shared;
mod source;
mod spectrum;
mod types;

pub use offline::OfflinePlayer;
pub use player::Player;
pub use spectrum::{BANDS, Spectrum};
pub use types::{Config, Event, Fades, Host, LoopRange, Status, Track};

/// What can go wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The song couldn't be opened: missing, unreadable, or not a sound format this knows.
    Open(String),
    /// The song broke while playing.
    Decode(String),
    /// The sound device couldn't be opened.
    Output(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Open(m) => write!(f, "can't open {m}"),
            Error::Decode(m) => write!(f, "can't decode: {m}"),
            Error::Output(m) => write!(f, "no sound output: {m}"),
        }
    }
}

impl std::error::Error for Error {}

/// A whole song decoded: stereo f32 frames, interleaved, at its own sample rate.
pub struct Decoded {
    pub rate: u32,
    pub samples: Vec<f32>,
}

/// Decodes a whole song, gapless (encoder delay and padding trimmed).
pub fn decode_file(path: &std::path::Path) -> Result<Decoded, Error> {
    decode_from(path, 0, usize::MAX)
}

/// Decodes a song from playable frame `from`, at most `frames` frames.
pub fn decode_from(path: &std::path::Path, from: u64, frames: usize) -> Result<Decoded, Error> {
    let mut s = source::Source::open(path)?;
    if from > 0 {
        s.seek(from)?;
    }
    let mut samples = Vec::new();
    while samples.len() / 2 < frames {
        let want = (frames - samples.len() / 2).min(65_536);
        if s.read(&mut samples, want)? == 0 {
            break;
        }
    }
    Ok(Decoded {
        rate: s.info().rate,
        samples,
    })
}
