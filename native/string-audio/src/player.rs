//! The player the app uses: the engine on its own thread, the mixer on the sound device's, and
//! calls that just post a command and return. Status and spectrum reads touch no lock.

use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::deck;
use crate::engine::{self, Engine};
use crate::mixer::{Mixer, Position};
use crate::output::{self, Output};
use crate::spectrum::Spectrum;
use crate::types::{Command, Config, Event, Host, LoopRange, Status, Track};

/// The calls both players share (`&self` for the threaded one, `&mut self` offline).
macro_rules! controls {
    ($($m:ident)?) => {
        /// Plays `track` from `from` seconds, now. What played fades out.
        pub fn play_track(&$($m)? self, track: Track, from: f64) {
            self.send(Command::Play { track, from, start: true });
        }

        /// Makes `track` ready at `from` seconds, paused (after a restart).
        pub fn cue(&$($m)? self, track: Track, from: f64) {
            self.send(Command::Play { track, from, start: false });
        }

        /// Plays on, fading in.
        pub fn play(&$($m)? self) {
            self.send(Command::Resume);
        }

        /// Pauses, fading out.
        pub fn pause(&$($m)? self) {
            self.send(Command::Pause);
        }

        /// Stops and lets the song go.
        pub fn stop(&$($m)? self) {
            self.send(Command::Stop);
        }

        /// Moves the current song to `secs`, fading out and in.
        pub fn seek(&$($m)? self, secs: f64) {
            self.send(Command::Seek(secs));
        }

        /// Volume 0..1 (heard as its square).
        pub fn set_volume(&$($m)? self, volume: f32) {
            self.send(Command::Volume(volume));
        }

        pub fn set_muted(&$($m)? self, muted: bool) {
            self.send(Command::Mute(muted));
        }

        /// Crossfade seconds between songs (0: gapless).
        pub fn set_crossfade(&$($m)? self, secs: f64) {
            self.send(Command::Crossfade(secs));
        }

        /// The current song's loop (None clears it).
        pub fn set_loop(&$($m)? self, range: Option<LoopRange>) {
            self.send(Command::Loop(range));
        }

        /// The queue changed: the engine asks again what plays next.
        pub fn queue_changed(&$($m)? self) {
            self.send(Command::QueueChanged);
        }

        /// Measure the spectrum (on while a visualizer shows; off saves the work).
        pub fn set_spectrum(&$($m)? self, on: bool) {
            self.send(Command::Spectrum(on));
        }
    };
}
pub(crate) use controls;

enum Msg {
    Cmd(Command),
    Quit,
}

/// String's player: songs from the app's queue to the default sound device.
pub struct Player {
    cmds: mpsc::Sender<Msg>,
    position: Arc<Position>,
    spectrum: Spectrum,
    epoch: Instant,
    thread: Option<JoinHandle<()>>,
}

impl Player {
    /// Starts the player on the default output. Without a device it waits for one (the host
    /// hears `Event::Output` when sound has somewhere to go).
    pub fn new(config: Config, host: impl Host) -> Player {
        let (tx, rx) = mpsc::channel();
        let spectrum = Spectrum::new();
        let position = Arc::new(Position::new());
        let epoch = Instant::now();
        let (spec, pos) = (spectrum.shared.clone(), position.clone());
        let thread = std::thread::Builder::new()
            .name("string-audio engine".into())
            .spawn(move || run(rx, config, Box::new(host), spec, pos, epoch))
            .expect("start the engine thread");
        Player {
            cmds: tx,
            position,
            spectrum,
            epoch,
            thread: Some(thread),
        }
    }

    /// The current song, its position (moving on smoothly between the sound device's periods)
    /// and whether it plays.
    pub fn status(&self) -> Status {
        engine::status(&self.position, Some(self.epoch.elapsed().as_nanos() as u64))
    }

    /// The visualizer's feed (cheap to clone, cheap to read every frame).
    pub fn spectrum(&self) -> Spectrum {
        self.spectrum.clone()
    }

    fn send(&self, cmd: Command) {
        let _ = self.cmds.send(Msg::Cmd(cmd));
    }

    controls!();
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.cmds.send(Msg::Quit);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn run(
    rx: mpsc::Receiver<Msg>,
    config: Config,
    host: Box<dyn Host>,
    spectrum: Arc<crate::spectrum::Published>,
    position: Arc<Position>,
    epoch: Instant,
) {
    let rate = output::default_rate().unwrap_or(48_000);
    let (fa, ra) = deck::rings(rate);
    let (fb, rb) = deck::rings(rate);
    let (mixer, link) = Mixer::new(rate, [ra, rb], spectrum, position.clone());
    let mixer = Arc::new(Mutex::new(mixer));
    let mut engine = Engine::new(host, config, [fa, fb], link, position);
    let mut out = Output::new(mixer.clone(), epoch);
    loop {
        let wait = if engine.busy() {
            Duration::from_millis(3)
        } else if engine.playing() {
            Duration::from_millis(25)
        } else {
            Duration::from_millis(200)
        };
        match rx.recv_timeout(wait) {
            Ok(Msg::Quit) | Err(RecvTimeoutError::Disconnected) => break,
            Ok(Msg::Cmd(c)) => {
                engine.command(c);
                while let Ok(m) = rx.try_recv() {
                    match m {
                        Msg::Cmd(c) => engine.command(c),
                        Msg::Quit => return,
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
        engine.pump();
        if out.needs_reopen(engine.playing()) {
            out.close();
            if let Ok((name, new_rate)) = out.open() {
                let old_rate = mixer.lock().map(|m| m.rate()).unwrap_or(new_rate);
                if new_rate != old_rate {
                    if let Ok(mut m) = mixer.lock() {
                        m.set_rate(new_rate);
                        m.reset();
                    }
                    engine.resync(new_rate);
                }
                engine.notify(Event::Output {
                    name,
                    rate: new_rate,
                });
            }
        }
    }
    out.close();
}
