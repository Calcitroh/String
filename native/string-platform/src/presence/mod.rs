//! Discord Rich Presence on a thread of String's own (String 1.x's `string-presence.exe`, presence\Program.cs, brought
//! inside): "Listening to <song>" with a time bar, through Discord's local pipe (`discord-ipc-N`) and the person's own
//! application ID. Activity type 2 and `status_display_type` from the setting; at most one update per 4.2 s (Discord
//! allows 5 per 20 s), always the newest; cleared on pause and when a stale song's time bar has run out; optional covers
//! (see `covers`). Without Discord it waits and tries again; when Discord goes away it reconnects and says the current
//! song again. Stopping (or dropping) closes the pipe, and Discord clears the status straight away.
//!
//! ```no_run
//! # use string_platform::presence::{Display, NowPlaying, Options, Presence};
//! # use std::time::Duration;
//! let data = std::path::Path::new(r"C:\data");
//! let presence = Presence::start(Options::new("123456789012345678", data), |event| {
//!     println!("{event}"); // "status connected …", "sent playing …": what Settings shows
//! })
//! .expect("the application ID is a long number");
//! let mut song = NowPlaying::playing("Glass Orbit", "Test Signal", "Synthetic Weather",
//!     Duration::from_secs(10), Duration::from_secs(180));
//! song.display = Display::from_setting("song");
//! presence.set(Some(song)); // again whenever it changes: pause, seek, the next song, Repeat one starting over
//! ```

mod activity;
mod covers;
mod ipc;

use covers::Covers;
use ipc::Pipe;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use std::{fmt, thread};

/// How the presence thread runs. [`Options::new`] has String's; the rest is for tests (a pipe, searches and waits of
/// their own).
#[derive(Clone, Debug)]
pub struct Options {
    /// The person's application ID on Discord's developer site (15 to 22 digits).
    pub app_id: String,
    /// `discord-covers.json` in String's data folder.
    pub covers_file: PathBuf,
    /// One pipe name instead of `discord-ipc-0` to `-9` (tests: a fake Discord of their own).
    pub pipe: Option<String>,
    /// Apple's iTunes search address.
    pub itunes: String,
    /// Deezer's track search address.
    pub deezer: String,
    /// At most one update this often: 4.2 s.
    pub min_interval: Duration,
    /// No Discord found: looked for again after 10 s.
    pub retry_after: Duration,
    /// Discord went away: looked for again after 4 s.
    pub reconnect_after: Duration,
}

impl Options {
    pub fn new(app_id: impl Into<String>, data_dir: &Path) -> Options {
        Options {
            app_id: app_id.into(),
            covers_file: data_dir.join("discord-covers.json"),
            pipe: None,
            itunes: "https://itunes.apple.com/search".into(),
            deezer: "https://api.deezer.com/search/track".into(),
            min_interval: Duration::from_millis(4200),
            retry_after: Duration::from_secs(10),
            reconnect_after: Duration::from_secs(4),
        }
    }
}

/// What plays, as String 1.x's presence.json said it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NowPlaying {
    /// False: paused (the status is cleared).
    pub playing: bool,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// When the song's start was, in Unix milliseconds (now minus the position).
    pub started_at: i64,
    /// When it ends, in Unix milliseconds.
    pub ends_at: i64,
    /// Look for a cover (Settings: covers on Discord, off by default).
    pub covers: bool,
    pub display: Display,
}

impl NowPlaying {
    /// A song playing now, `position` into its `length`.
    pub fn playing(
        title: &str,
        artist: &str,
        album: &str,
        position: Duration,
        length: Duration,
    ) -> NowPlaying {
        let started_at = unix_ms() - position.as_millis() as i64;
        NowPlaying {
            playing: true,
            title: title.into(),
            artist: artist.into(),
            album: album.into(),
            started_at,
            ends_at: started_at + length.as_millis() as i64,
            covers: false,
            display: Display::Song,
        }
    }
}

/// What Discord's member list shows under the person's name (`status_display_type`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Display {
    /// "Listening to <application name>".
    App,
    /// The artist.
    Artist,
    /// The song.
    #[default]
    Song,
}

impl Display {
    /// String's setting: "app", "artist", anything else the song.
    pub fn from_setting(s: &str) -> Display {
        match s {
            "app" => Display::App,
            "artist" => Display::Artist,
            _ => Display::Song,
        }
    }
}

/// What happened, for Settings. Its text is the 1.x helper's line (`status waiting`, `sent playing <title>`, ...).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// No Discord to talk to (not running, or gone): looked for again.
    Waiting,
    /// Connected; the name Discord gave (global name, else user name, else "to Discord").
    Connected(String),
    /// Discord was told something new.
    Sent(Sent),
    /// Discord said no to something; it carries on.
    Warning(String),
    /// Discord doesn't know the application ID: the thread has stopped (the helper's exit code 3).
    Refused(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sent {
    Playing(String),
    Clear,
}

impl fmt::Display for Event {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Event::Waiting => f.write_str("status waiting"),
            Event::Connected(name) => write!(f, "status connected {name}"),
            Event::Sent(Sent::Playing(title)) => write!(f, "sent playing {title}"),
            Event::Sent(Sent::Clear) => f.write_str("sent clear"),
            Event::Warning(text) => write!(f, "warn {text}"),
            Event::Refused(text) => write!(f, "error {text}"),
        }
    }
}

/// The application ID isn't 15 to 22 digits (the helper's exit code 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BadAppId;

impl fmt::Display for BadAppId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(
            "The application ID is the long number on your app's page on Discord's developer site",
        )
    }
}

impl std::error::Error for BadAppId {}

/// The presence thread. Dropping it stops the thread and closes the pipe (Discord clears the status).
pub struct Presence {
    tx: Sender<Msg>,
    thread: Option<thread::JoinHandle<()>>,
}

enum Msg {
    Set(Option<NowPlaying>),
    Wake,
    Stop,
}

impl Presence {
    /// Starts the thread. `on_event` is called on it: forward to the screens.
    pub fn start(
        options: Options,
        on_event: impl FnMut(Event) + Send + 'static,
    ) -> Result<Presence, BadAppId> {
        let id = options.app_id.as_str();
        if !(15..=22).contains(&id.len()) || !id.bytes().all(|b| b.is_ascii_digit()) {
            return Err(BadAppId);
        }
        let (tx, rx) = mpsc::channel();
        let wake = tx.clone();
        let thread = thread::Builder::new()
            .name("string-presence".into())
            .spawn(move || Worker::new(options, rx, wake, on_event).run())
            .ok();
        Ok(Presence { tx, thread })
    }

    /// What plays now (`None`: nothing). Send it again on every change: pause, play, seek, the next song, and each time
    /// Repeat one starts the song over (or its time bar runs out on Discord and clears).
    pub fn set(&self, now: Option<NowPlaying>) {
        let _ = self.tx.send(Msg::Set(now));
    }

    /// Whether the thread still runs (it stops by itself when Discord refuses the application ID).
    pub fn is_running(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }
}

impl Drop for Presence {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Stop);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// How often the thread looks at the pipe and the clock while connected.
const TICK: Duration = Duration::from_millis(100);

enum End {
    Stop,
    Closed,
    Refused(String),
}

struct Worker<F> {
    o: Options,
    rx: Receiver<Msg>,
    wake: Sender<Msg>,
    on_event: F,
    now: Option<NowPlaying>,
    covers: Covers,
    status: Option<Event>,
}

impl<F: FnMut(Event)> Worker<F> {
    fn new(o: Options, rx: Receiver<Msg>, wake: Sender<Msg>, on_event: F) -> Worker<F> {
        let covers = Covers::load(&o.covers_file, &o.itunes, &o.deezer);
        Worker {
            o,
            rx,
            wake,
            on_event,
            now: None,
            covers,
            status: None,
        }
    }

    fn run(mut self) {
        let names: Vec<String> = match &self.o.pipe {
            Some(p) => vec![p.clone()],
            None => (0..10).map(|i| format!("discord-ipc-{i}")).collect(),
        };
        loop {
            let Some(pipe) = Pipe::connect(&names) else {
                self.status(Event::Waiting);
                if self.idle(self.o.retry_after) {
                    return;
                }
                continue;
            };
            match self.session(pipe) {
                End::Stop => return,
                End::Refused(text) => {
                    (self.on_event)(Event::Refused(text));
                    return;
                }
                End::Closed => {}
            }
            self.status(Event::Waiting);
            if self.idle(self.o.reconnect_after) {
                return;
            }
        }
    }

    /// One connection, from the handshake until Discord goes away (or String stops it).
    fn session(&mut self, mut pipe: Pipe) -> End {
        let hello = json!({ "v": 1, "client_id": self.o.app_id }).to_string();
        if pipe.send(ipc::HANDSHAKE, &hello).is_err() {
            return End::Closed;
        }
        // a Discord that takes the connection but never answers it isn't waited on for ever: tried again later
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut frames = loop {
            match pipe.poll() {
                Err(_) => return End::Closed,
                Ok(f) if !f.is_empty() => break f.into_iter(),
                Ok(_) => {}
            }
            if Instant::now() >= deadline {
                return End::Closed;
            }
            if self.wait(Duration::from_millis(20)) {
                return End::Stop;
            }
        };
        let first = frames.next().expect("at least one frame");
        if first.op == ipc::CLOSE {
            let code = first.json["code"].as_i64().unwrap_or(0);
            let message = first.json["message"].as_str().unwrap_or("closed");
            if code == 4000 || message.to_lowercase().contains("client") {
                return End::Refused("Discord doesn't know that application ID: check it on Discord's developer site".into());
            }
            return End::Closed;
        }
        if first.json["evt"] != "READY" {
            return End::Closed;
        }
        let user = &first.json["data"]["user"];
        let name = user["global_name"]
            .as_str()
            .or(user["username"].as_str())
            .unwrap_or("to Discord");
        self.status(Event::Connected(name.to_string()));

        let mut pending: Vec<ipc::Frame> = frames.collect();
        let mut sent: Option<Option<String>> = None; // the activity last sent (None: nothing yet this connection)
        let mut said: Option<Sent> = None;
        let mut last_send: Option<Instant> = None;
        loop {
            // Discord's side: answer pings, notice errors and the pipe closing
            match pipe.poll() {
                Ok(f) => pending.extend(f),
                Err(_) => return End::Closed,
            }
            for f in pending.drain(..) {
                match f.op {
                    ipc::PING => {
                        if pipe.send(ipc::PONG, &f.raw).is_err() {
                            return End::Closed;
                        }
                    }
                    ipc::CLOSE => return End::Closed,
                    _ if f.json["evt"] == "ERROR" => {
                        let text = f.json["data"]["message"]
                            .as_str()
                            .unwrap_or("Discord said no");
                        (self.on_event)(Event::Warning(text.to_string()));
                    }
                    _ => {}
                }
            }
            // ours: the newest state, at most once per interval
            let (activity, summary) = self.build();
            let text = activity.as_ref().map(Value::to_string);
            let due = last_send.is_none_or(|t| t.elapsed() >= self.o.min_interval);
            if sent.as_ref() != Some(&text) && due {
                let mut args = json!({ "pid": std::process::id() });
                if let Some(a) = activity {
                    args["activity"] = a;
                }
                let message = json!({ "cmd": "SET_ACTIVITY", "args": args, "nonce": nonce() });
                if pipe.send(ipc::FRAME, &message.to_string()).is_err() {
                    return End::Closed;
                }
                sent = Some(text);
                last_send = Some(Instant::now());
                if said.as_ref() != Some(&summary) {
                    said = Some(summary.clone());
                    (self.on_event)(Event::Sent(summary));
                }
            }
            if self.wait(TICK) {
                return End::Stop;
            }
        }
    }

    fn build(&self) -> (Option<Value>, Sent) {
        let wake = self.wake.clone();
        activity::build(self.now.as_ref(), unix_ms(), |artist, title| {
            self.covers.get(artist, title, move || {
                let _ = wake.send(Msg::Wake);
            })
        })
    }

    /// Waits up to `d` for a message, keeping up with what plays; true when told to stop.
    fn wait(&mut self, d: Duration) -> bool {
        match self.rx.recv_timeout(d) {
            Ok(Msg::Set(now)) => {
                self.now = now;
                false
            }
            Ok(Msg::Wake) | Err(RecvTimeoutError::Timeout) => false,
            Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => true,
        }
    }

    /// Waits all of `d` (between connection attempts), keeping up with what plays; true when told to stop.
    fn idle(&mut self, d: Duration) -> bool {
        let end = Instant::now() + d;
        loop {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            if self.wait(left) {
                return true;
            }
        }
    }

    /// Waiting and Connected, each said once until it changes.
    fn status(&mut self, e: Event) {
        if self.status.as_ref() != Some(&e) {
            self.status = Some(e.clone());
            (self.on_event)(e);
        }
    }
}

/// Now, in Unix milliseconds (Discord's time bar counts in them).
pub fn unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// A fresh nonce for each command, shaped like a GUID (as the helper's were).
fn nonce() -> String {
    use std::hash::{BuildHasher, Hasher, RandomState};
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let s = RandomState::new();
    let mut h = s.build_hasher();
    h.write_u64(COUNT.fetch_add(1, Ordering::Relaxed));
    let a = h.finish();
    let mut h = s.build_hasher();
    h.write_u64(a);
    let b = h.finish();
    format!(
        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        a >> 32,
        (a >> 16) & 0xffff,
        a & 0xfff,
        (b >> 48) & 0x3fff | 0x8000,
        b & 0xffff_ffff_ffff
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_settings_by_default() {
        let o = Options::new("123456789012345678", Path::new(r"C:\data"));
        assert_eq!(o.min_interval, Duration::from_millis(4200));
        assert_eq!(o.covers_file, Path::new(r"C:\data\discord-covers.json"));
        assert_eq!(o.pipe, None);
        assert_eq!(o.itunes, "https://itunes.apple.com/search");
        assert_eq!(o.deezer, "https://api.deezer.com/search/track");
        assert_eq!(
            (o.retry_after, o.reconnect_after),
            (Duration::from_secs(10), Duration::from_secs(4))
        );
    }

    #[test]
    fn helper_lines() {
        assert_eq!(Event::Waiting.to_string(), "status waiting");
        assert_eq!(
            Event::Connected("Tester".into()).to_string(),
            "status connected Tester"
        );
        assert_eq!(
            Event::Sent(Sent::Playing("X".into())).to_string(),
            "sent playing X"
        );
        assert_eq!(Event::Sent(Sent::Clear).to_string(), "sent clear");
        assert_eq!(Event::Warning("no".into()).to_string(), "warn no");
        assert!(Event::Refused("x".into()).to_string().starts_with("error "));
        assert_eq!(Display::from_setting("app"), Display::App);
        assert_eq!(Display::from_setting("artist"), Display::Artist);
        assert_eq!(Display::from_setting("song"), Display::Song);
        assert_eq!(Display::from_setting(""), Display::Song);
    }

    #[test]
    fn nonces_differ() {
        let (a, b) = (nonce(), nonce());
        assert_ne!(a, b);
        assert_eq!(a.len(), 36);
        assert_eq!(&a[14..15], "4");
    }

    #[test]
    fn playing_now() {
        let before = unix_ms();
        let p = NowPlaying::playing(
            "T",
            "A",
            "B",
            Duration::from_secs(10),
            Duration::from_secs(180),
        );
        assert!(p.playing && p.ends_at - p.started_at == 180_000);
        assert!((before - 10_000 - p.started_at).abs() < 1000);
    }
}
