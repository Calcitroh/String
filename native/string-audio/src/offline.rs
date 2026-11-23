//! The whole player without a sound device: the same engine and mixer, driven by hand, rendering
//! into memory as fast as it can. Each block is preceded by one round of the engine's work, as
//! if its thread kept up perfectly, so tests come out the same every time.

use std::sync::Arc;

use crate::deck;
use crate::engine::{self, Engine};
use crate::mixer::Mixer;
use crate::spectrum::Spectrum;
use crate::types::{Command, Config, Host, LoopRange, Status, Track};

/// Frames rendered per block (about 10 ms, like a sound device's period).
const BLOCK: usize = 480;

pub struct OfflinePlayer {
    engine: Engine,
    mixer: Mixer,
    spectrum: Spectrum,
    position: Arc<crate::mixer::Position>,
    clock: u64,
    rate: u32,
    buf: Vec<f32>,
}

impl OfflinePlayer {
    pub fn new(rate: u32, config: Config, host: impl Host) -> OfflinePlayer {
        let spectrum = Spectrum::new();
        let position = Arc::new(crate::mixer::Position::new());
        let (fa, ra) = deck::rings(rate);
        let (fb, rb) = deck::rings(rate);
        let (mixer, link) = Mixer::new(rate, [ra, rb], spectrum.shared.clone(), position.clone());
        let mut engine = Engine::new(Box::new(host), config, [fa, fb], link, position.clone());
        engine.offline();
        OfflinePlayer {
            engine,
            mixer,
            spectrum,
            position,
            clock: 0,
            rate,
            buf: vec![0.0; BLOCK * 2],
        }
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Renders `frames` stereo frames (interleaved).
    pub fn render(&mut self, frames: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(frames * 2);
        while out.len() < frames * 2 {
            let n = BLOCK.min(frames - out.len() / 2);
            self.engine.pump();
            let buf = &mut self.buf[..n * 2];
            self.mixer.render(buf, self.clock);
            self.clock += n as u64 * 1_000_000_000 / self.rate as u64;
            out.extend_from_slice(buf);
        }
        self.engine.pump();
        out
    }

    /// Renders seconds of sound.
    pub fn render_secs(&mut self, secs: f64) -> Vec<f32> {
        self.render((secs * self.rate as f64).round() as usize)
    }

    pub fn status(&self) -> Status {
        engine::status(&self.position, None)
    }

    pub fn spectrum(&self) -> Spectrum {
        self.spectrum.clone()
    }

    fn send(&mut self, cmd: Command) {
        self.engine.command(cmd);
    }

    crate::player::controls!(mut);
}
