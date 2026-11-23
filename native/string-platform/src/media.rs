//! Media keys and Windows' media overlay (System Media Transport Controls), tied to String's window through
//! ISystemMediaTransportControlsInterop: the song (title, artist, album, cover), playing or paused, the time bar, and the
//! buttons pressed (keyboard media keys, the overlay, headsets).
//!
//! ```no_run
//! # use string_platform::{Hwnd, media::{MediaButton, MediaControls, Song}};
//! # use std::time::Duration;
//! # let hwnd = Hwnd(0);
//! let media = MediaControls::new(hwnd, |button| match button {
//!     MediaButton::Play | MediaButton::Pause | MediaButton::Next | MediaButton::Previous => { /* to the player */ }
//!     MediaButton::Stop => { /* pause, as String 1.x did */ }
//!     MediaButton::Rewind | MediaButton::FastForward => { /* 10 s back or on */ }
//!     MediaButton::Seek(to) => { /* seek */ }
//! })
//! .unwrap();
//! media.set_song(&Song { title: "Glass Orbit", artist: "Test Signal", album: "Synthetic Weather", cover: None }).unwrap();
//! media.set_playing(true).unwrap();
//! media.set_timeline(Duration::from_secs(10), Duration::from_secs(180)).unwrap();
//! ```

use crate::Hwnd;
use std::{io, sync::Arc, time::Duration};
use windows::Foundation::{TimeSpan, TypedEventHandler};
use windows::Media::{
    MediaPlaybackStatus, MediaPlaybackType, PlaybackPositionChangeRequestedEventArgs,
    SystemMediaTransportControls, SystemMediaTransportControlsButton,
    SystemMediaTransportControlsButtonPressedEventArgs,
    SystemMediaTransportControlsTimelineProperties,
};
use windows::Storage::Streams::{
    DataWriter, InMemoryRandomAccessStream, RandomAccessStreamReference,
};
use windows::Win32::System::WinRT::ISystemMediaTransportControlsInterop;
use windows::core::{HSTRING, Ref, factory};

/// A press on a media key, the overlay or a headset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaButton {
    /// The play/pause key sends `Play` while paused and `Pause` while playing.
    Play,
    Pause,
    /// String 1.x paused on it.
    Stop,
    Next,
    Previous,
    /// Back a little (String 1.x: 10 s).
    Rewind,
    /// On a little (String 1.x: 10 s).
    FastForward,
    /// The overlay's time bar was moved (where Windows shows one).
    Seek(Duration),
}

/// What plays. `cover`: the picture's bytes as stored in the song (JPEG or PNG).
#[derive(Clone, Copy, Debug, Default)]
pub struct Song<'a> {
    pub title: &'a str,
    pub artist: &'a str,
    pub album: &'a str,
    pub cover: Option<&'a [u8]>,
}

/// String's entry in the media overlay. Hidden until the first [`MediaControls::set_song`]; gone when dropped.
pub struct MediaControls {
    smtc: SystemMediaTransportControls,
    pressed: i64,
    seek: i64,
}

impl MediaControls {
    /// For the main window. `on_button` is called on a Windows thread of its own: forward it to the player.
    pub fn new(
        hwnd: Hwnd,
        on_button: impl Fn(MediaButton) + Send + Sync + 'static,
    ) -> io::Result<MediaControls> {
        let interop =
            factory::<SystemMediaTransportControls, ISystemMediaTransportControlsInterop>()?;
        let smtc: SystemMediaTransportControls = unsafe { interop.GetForWindow(hwnd.get()) }?;
        smtc.SetIsEnabled(false)?;
        smtc.SetIsPlayEnabled(true)?;
        smtc.SetIsPauseEnabled(true)?;
        smtc.SetIsNextEnabled(true)?;
        smtc.SetIsPreviousEnabled(true)?;
        smtc.SetIsStopEnabled(true)?;
        smtc.SetIsRewindEnabled(true)?;
        smtc.SetIsFastForwardEnabled(true)?;
        let on_button = Arc::new(on_button);
        let tell = on_button.clone();
        let pressed = smtc.ButtonPressed(&TypedEventHandler::new(
            move |_: Ref<SystemMediaTransportControls>,
                  args: Ref<SystemMediaTransportControlsButtonPressedEventArgs>| {
                if let Some(b) = button(args.ok()?.Button()?) {
                    tell(b);
                }
                Ok(())
            },
        ))?;
        let tell = on_button;
        let seek = smtc.PlaybackPositionChangeRequested(&TypedEventHandler::new(
            move |_: Ref<SystemMediaTransportControls>,
                  args: Ref<PlaybackPositionChangeRequestedEventArgs>| {
                tell(MediaButton::Seek(duration(
                    args.ok()?.RequestedPlaybackPosition()?,
                )));
                Ok(())
            },
        ))?;
        Ok(MediaControls {
            smtc,
            pressed,
            seek,
        })
    }

    /// The song now playing (or paused on); shows String in the overlay.
    pub fn set_song(&self, song: &Song) -> io::Result<()> {
        self.describe(song)?;
        self.smtc.SetIsEnabled(true)?;
        Ok(())
    }

    /// Nothing to play: String leaves the overlay and media keys go to whoever else plays.
    pub fn clear(&self) -> io::Result<()> {
        let d = self.smtc.DisplayUpdater()?;
        d.ClearAll()?;
        d.Update()?;
        self.smtc.SetPlaybackStatus(MediaPlaybackStatus::Closed)?;
        self.smtc.SetIsEnabled(false)?;
        Ok(())
    }

    pub fn set_playing(&self, playing: bool) -> io::Result<()> {
        let status = if playing {
            MediaPlaybackStatus::Playing
        } else {
            MediaPlaybackStatus::Paused
        };
        self.smtc.SetPlaybackStatus(status)?;
        Ok(())
    }

    /// The time bar: where the song is and how long it is. Call when a song starts, on a seek, on play and pause (and
    /// now and then while playing, so a drift never builds up).
    pub fn set_timeline(&self, position: Duration, length: Duration) -> io::Result<()> {
        let t = SystemMediaTransportControlsTimelineProperties::new()?;
        t.SetStartTime(span(Duration::ZERO))?;
        t.SetEndTime(span(length))?;
        t.SetMinSeekTime(span(Duration::ZERO))?;
        t.SetMaxSeekTime(span(length))?;
        t.SetPosition(span(position.min(length)))?;
        self.smtc.UpdateTimelineProperties(&t)?;
        Ok(())
    }

    /// The song's details, without showing String in the overlay (the tests stop here).
    fn describe(&self, song: &Song) -> io::Result<()> {
        let d = self.smtc.DisplayUpdater()?;
        d.ClearAll()?;
        d.SetType(MediaPlaybackType::Music)?;
        let m = d.MusicProperties()?;
        m.SetTitle(&HSTRING::from(song.title))?;
        m.SetArtist(&HSTRING::from(song.artist))?;
        m.SetAlbumTitle(&HSTRING::from(song.album))?;
        if let Some(bytes) = song.cover.filter(|b| !b.is_empty()) {
            d.SetThumbnail(&picture(bytes)?)?;
        }
        d.Update()?;
        Ok(())
    }
}

impl Drop for MediaControls {
    fn drop(&mut self) {
        let _ = self.smtc.RemoveButtonPressed(self.pressed);
        let _ = self.smtc.RemovePlaybackPositionChangeRequested(self.seek);
        let _ = self.smtc.SetIsEnabled(false);
    }
}

/// The cover's bytes as a stream the overlay can read whenever it likes.
fn picture(bytes: &[u8]) -> windows::core::Result<RandomAccessStreamReference> {
    let writer = DataWriter::new()?;
    writer.WriteBytes(bytes)?;
    let buffer = writer.DetachBuffer()?;
    let stream = InMemoryRandomAccessStream::new()?;
    stream.WriteAsync(&buffer)?.join()?;
    stream.Seek(0)?;
    RandomAccessStreamReference::CreateFromStream(&stream)
}

fn button(b: SystemMediaTransportControlsButton) -> Option<MediaButton> {
    Some(match b {
        SystemMediaTransportControlsButton::Play => MediaButton::Play,
        SystemMediaTransportControlsButton::Pause => MediaButton::Pause,
        SystemMediaTransportControlsButton::Stop => MediaButton::Stop,
        SystemMediaTransportControlsButton::Next => MediaButton::Next,
        SystemMediaTransportControlsButton::Previous => MediaButton::Previous,
        SystemMediaTransportControlsButton::Rewind => MediaButton::Rewind,
        SystemMediaTransportControlsButton::FastForward => MediaButton::FastForward,
        _ => return None,
    })
}

/// WinRT counts time in 100 ns ticks.
fn span(d: Duration) -> TimeSpan {
    TimeSpan {
        Duration: (d.as_nanos() / 100).min(i64::MAX as u128) as i64,
    }
}

fn duration(t: TimeSpan) -> Duration {
    Duration::from_nanos((t.Duration.max(0) as u64).saturating_mul(100))
}

#[cfg(test)]
mod tests {
    // Only on a hidden window of the tests' own, and never enabled: the person's media overlay and keys stay untouched.
    use super::*;
    use crate::test_window::TestWindow;

    const PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f,
        0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn the_song_reaches_the_controls_without_showing() {
        let w = TestWindow::new();
        let media = MediaControls::new(w.hwnd, |_| {}).unwrap();
        let song = Song {
            title: "Glass Orbit",
            artist: "Test Signal",
            album: "Synthetic Weather",
            cover: Some(PNG),
        };
        media.describe(&song).unwrap();
        media.set_playing(true).unwrap();
        media
            .set_timeline(Duration::from_secs(10), Duration::from_secs(180))
            .unwrap();
        let d = media.smtc.DisplayUpdater().unwrap();
        let m = d.MusicProperties().unwrap();
        assert_eq!(m.Title().unwrap(), "Glass Orbit");
        assert_eq!(m.Artist().unwrap(), "Test Signal");
        assert_eq!(m.AlbumTitle().unwrap(), "Synthetic Weather");
        assert_eq!(d.Type().unwrap(), MediaPlaybackType::Music);
        assert!(d.Thumbnail().is_ok());
        assert_eq!(
            media.smtc.PlaybackStatus().unwrap(),
            MediaPlaybackStatus::Playing
        );
        media.set_playing(false).unwrap();
        assert_eq!(
            media.smtc.PlaybackStatus().unwrap(),
            MediaPlaybackStatus::Paused
        );
        assert!(
            !media.smtc.IsEnabled().unwrap(),
            "a test must never show in the overlay"
        );
        assert!(media.smtc.IsNextEnabled().unwrap() && media.smtc.IsPreviousEnabled().unwrap());
        // as String 1.x's page set them: stop (pause), and back or on a little
        let s = &media.smtc;
        assert!(
            s.IsStopEnabled().unwrap()
                && s.IsRewindEnabled().unwrap()
                && s.IsFastForwardEnabled().unwrap()
        );
    }

    #[test]
    fn buttons_and_times() {
        assert_eq!(
            button(SystemMediaTransportControlsButton::Play),
            Some(MediaButton::Play)
        );
        assert_eq!(
            button(SystemMediaTransportControlsButton::Pause),
            Some(MediaButton::Pause)
        );
        assert_eq!(
            button(SystemMediaTransportControlsButton::Next),
            Some(MediaButton::Next)
        );
        assert_eq!(
            button(SystemMediaTransportControlsButton::Previous),
            Some(MediaButton::Previous)
        );
        assert_eq!(
            button(SystemMediaTransportControlsButton::Stop),
            Some(MediaButton::Stop)
        );
        assert_eq!(
            button(SystemMediaTransportControlsButton::FastForward),
            Some(MediaButton::FastForward)
        );
        assert_eq!(
            button(SystemMediaTransportControlsButton::Rewind),
            Some(MediaButton::Rewind)
        );
        assert_eq!(button(SystemMediaTransportControlsButton::Record), None);
        assert_eq!(span(Duration::from_millis(1500)).Duration, 15_000_000);
        assert_eq!(
            duration(TimeSpan {
                Duration: 15_000_000
            }),
            Duration::from_millis(1500)
        );
        assert_eq!(duration(TimeSpan { Duration: -5 }), Duration::ZERO);
    }
}
