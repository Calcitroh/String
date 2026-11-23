use crate::services::{Discord as DiscordInfo, DiscordStatus};
use crate::{
    model::{CoverSource, Song},
    playback::Session,
    ui::covers::source_image,
};
use egui::Context;
use std::sync::Mutex;
use std::{
    io::{self, Cursor},
    path::Path,
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};
use string_audio::Status;
use string_platform::presence::{Event, Sent};
use string_platform::{
    errors_log::ErrorLog,
    media,
    presence::{Display, NowPlaying, Options, Presence},
    taskbar::ThumbButtons,
};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize};

type Artwork = Arc<CoverSource>;
struct Covers {
    tx: mpsc::Sender<Artwork>,
    rx: mpsc::Receiver<(Artwork, Vec<u8>)>,
}
impl Covers {
    fn new(ctx: &Context) -> Self {
        let (tx, jobs) = mpsc::channel::<Artwork>();
        let (done, rx) = mpsc::channel();
        let ctx = ctx.clone();
        thread::spawn(move || {
            let initialized = unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.is_ok();
            while let Ok(first) = jobs.recv() {
                let source = jobs.try_iter().last().unwrap_or(first);
                let bytes = source_image(&source, 420).thumbnail(420, 420).into_rgb8();
                let mut jpeg = Cursor::new(Vec::new());
                if bytes.write_to(&mut jpeg, image::ImageFormat::Jpeg).is_ok() {
                    if done.send((source, jpeg.into_inner())).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            }
            if initialized {
                unsafe { RoUninitialize() };
            }
        });
        Self { tx, rx }
    }
}

#[derive(Default)]
struct Discord {
    id: Option<String>,
    presence: Option<Presence>,
    status: Arc<Mutex<DiscordInfo>>,
    covers: bool,
    display: Display,
}
impl Discord {
    fn configure(&mut self, s: &Session, root: &Path, ctx: &Context) -> bool {
        let id = s
            .setting("discord")
            .as_bool()
            .unwrap_or(false)
            .then(|| s.setting("discordId").as_str().unwrap_or("").trim());
        let covers = s.setting("dcCovers").as_bool().unwrap_or(false);
        let display = Display::from_setting(s.setting("dcShow").as_str().unwrap_or("song"));
        let changed = self.id.as_deref() != id;
        if changed {
            self.presence.take();
            self.id = id.map(str::to_owned);
            *self.status.lock().unwrap() = DiscordInfo::default();
            if let Some(id) = id {
                let info = self.status.clone();
                info.lock().unwrap().status = DiscordStatus::Starting;
                let ctx = ctx.clone();
                match Presence::start(Options::new(id, root), move |event| {
                    let mut d = info.lock().unwrap();
                    match event {
                        Event::Waiting => {
                            d.status = DiscordStatus::Waiting;
                            d.showing.clear();
                        }
                        Event::Connected(user) => {
                            d.status = DiscordStatus::Connected;
                            d.user = user;
                            d.error.clear();
                        }
                        Event::Sent(Sent::Playing(title)) => d.showing = title,
                        Event::Sent(Sent::Clear) => d.showing.clear(),
                        Event::Warning(msg) => d.error = msg,
                        Event::Refused(msg) => {
                            d.status = DiscordStatus::Refused;
                            d.error = msg;
                            d.showing.clear();
                        }
                    }
                    ctx.request_repaint();
                }) {
                    Ok(p) => {
                        if !p.is_running() {
                            self.status.lock().unwrap().status = DiscordStatus::Stopped;
                        }
                        self.presence = Some(p);
                    }
                    Err(_) => {
                        self.status.lock().unwrap().status = if id.is_empty() {
                            DiscordStatus::NoId
                        } else {
                            DiscordStatus::BadId
                        }
                    }
                }
            }
        }
        let republish = changed || self.display != display || self.covers != covers;
        self.display = display;
        self.covers = covers;
        republish
    }
    fn publish(&self, song: Option<&Song>, playing: bool, pos: Duration, length: Duration) {
        if let Some(p) = &self.presence {
            p.set(song.filter(|_| playing).map(|s| {
                let mut now = NowPlaying::playing(&s.title, &s.artist, &s.album, pos, length);
                if length.is_zero() {
                    now.ends_at = 0;
                }
                now.covers = self.covers;
                now.display = self.display;
                now
            }));
        }
    }
}

struct Clock {
    at: Instant,
    st: Status,
    seek: u64,
}
impl Clock {
    fn changed(&self, st: Status, seek: u64, at: Instant) -> bool {
        let expected = self.st.position
            + if self.st.playing {
                (at - self.at).as_secs_f64()
            } else {
                0.
            };
        self.seek != seek
            || self.st.key != st.key
            || self.st.duration != st.duration
            || self.st.playing != st.playing
            || (st.position - expected).abs() > 0.30
    }
}
fn secs(seconds: f64) -> Duration {
    Duration::try_from_secs_f64(seconds).unwrap_or_default()
}

#[derive(Default)]
pub struct Transport {
    // The model generation also changes on tag and cover edits.
    current: Option<(String, u64)>,
    clock: Option<Clock>,
    covers: Option<Covers>,
    discord: Discord,
}

impl Transport {
    pub fn discord_status(&self) -> Arc<Mutex<DiscordInfo>> {
        self.discord.status.clone()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        s: &Session,
        st: Status,
        media: Option<&media::MediaControls>,
        taskbar: Option<&ThumbButtons>,
        root: &Path,
        log: &ErrorLog,
        ctx: &Context,
    ) {
        let report = |r: io::Result<()>| {
            if let Err(e) = r {
                log.write("media controls", e);
            }
        };
        let song = s.current();
        let metadata_changed = self.current.as_ref().map(|(id, g)| (id.as_str(), *g))
            != song.map(|song| (song.id.as_str(), s.generation));
        let at = Instant::now();
        let moved = self
            .clock
            .as_ref()
            .is_none_or(|c| c.changed(st, s.seek_revision, at));
        let refresh = self
            .clock
            .as_ref()
            .is_none_or(|c| st.playing && at - c.at >= Duration::from_secs(5));
        let length = secs(
            st.duration
                .or_else(|| song.map(|s| s.duration))
                .unwrap_or_default(),
        );
        let pos = if self
            .clock
            .as_ref()
            .is_some_and(|c| c.seek != s.seek_revision && c.st.key == st.key)
        {
            secs(s.seek_position)
        } else {
            secs(st.position)
        };
        if let Some(tb) = taskbar
            && tb.playing() != st.playing
        {
            tb.set_playing(st.playing);
        }
        if let Some(media) = media {
            if metadata_changed {
                match song {
                    Some(song) => {
                        report(describe(media, song, None));
                        let covers = self.covers.get_or_insert_with(|| Covers::new(ctx));
                        let _ = covers.tx.send(song.cover.clone());
                    }
                    None => report(media.clear()),
                }
            }
            if let Some(covers) = &self.covers {
                for (source, bytes) in covers.rx.try_iter() {
                    if let Some(song) = song.filter(|s| *s.cover == *source) {
                        report(describe(media, song, Some(&bytes)));
                    }
                }
            }
            if song.is_some() && (metadata_changed || moved || refresh) {
                report(media.set_playing(st.playing));
                report(media.set_timeline(pos, length));
            }
        }
        let settings_changed = self.discord.configure(s, root, ctx);
        if metadata_changed || moved || settings_changed {
            self.discord.publish(song, st.playing, pos, length);
        }
        if metadata_changed {
            self.current = song.map(|sng| (sng.id.clone(), s.generation));
        }
        if metadata_changed || moved || refresh {
            self.clock = Some(Clock {
                at,
                st,
                seek: s.seek_revision,
            });
        }
    }
}

fn describe(media: &media::MediaControls, s: &Song, cover: Option<&[u8]>) -> io::Result<()> {
    media.set_song(&media::Song {
        title: &s.title,
        artist: &s.artist,
        album: &s.album,
        cover,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pause_seek_repeat_and_track_change_republish_the_clock() {
        let at = Instant::now();
        let st = Status {
            key: Some(1),
            position: 2.,
            duration: Some(80.),
            playing: true,
        };
        let clock = Clock { at, st, seek: 0 };
        assert!(!clock.changed(
            Status { position: 3., ..st },
            0,
            at + Duration::from_secs(1)
        ));
        assert!(clock.changed(st, 1, at));
        assert!(clock.changed(
            Status {
                playing: false,
                ..st
            },
            0,
            at
        ));
        assert!(clock.changed(Status { position: 0., ..st }, 0, at));
        assert!(clock.changed(Status { key: Some(2), ..st }, 0, at));
    }
}
