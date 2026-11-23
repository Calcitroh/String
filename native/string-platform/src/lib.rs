//! String on Windows: one copy at a time, media keys, taskbar buttons, Discord, dark title bar, pickers, Recycle Bin.
//!
//! Toolkit-agnostic: whatever draws the screens hands in its window's handle ([`Hwnd`]) and gets callbacks back. Each
//! part is a module of its own:
//!
//! - [`instance`]: one copy at a time, with String 1.x's names (`StringApp.*`), `--quit`, a second launch bringing the
//!   first forward.
//! - [`window`]: the dark title bar, no white flash at start, bringing the window forward, `window.json`.
//! - [`media`]: media keys and Windows' media overlay (SMTC): the song, its cover, playing or paused, the time bar.
//! - [`taskbar`]: previous, play/pause and next under the taskbar thumbnail.
//! - [`presence`]: Discord Rich Presence on a thread of its own ("Listening to ...", optional covers).
//! - [`files`]: folder and file pickers, reveal in Explorer, web links, the Recycle Bin, the backup folder.
//! - [`errors_log`]: `errors.log` in the data folder.
//!
//! Threads: [`taskbar::ThumbButtons`] and the pickers belong to the window's own thread (the one running its message
//! loop). [`media::MediaControls`], [`presence::Presence`] and [`instance::Instance`] can live anywhere; their callbacks
//! arrive on threads of their own, so a shell forwards them to its event loop.

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::num::NonZeroIsize;
use windows::Win32::Foundation::HWND;

mod com;
pub mod errors_log;
pub mod files;
mod glyphs;
pub mod instance;
pub mod media;
pub mod presence;
pub mod taskbar;
#[cfg(test)]
mod test_window;
pub mod window;

/// A top-level window's handle (an HWND), from whichever toolkit made the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Hwnd(pub isize);

impl Hwnd {
    /// The handle of a toolkit's window (winit, Slint and the rest implement `HasWindowHandle`); `None` if it isn't a
    /// Win32 window or isn't ready yet.
    pub fn from_window(window: &impl HasWindowHandle) -> Option<Hwnd> {
        match window.window_handle().ok()?.as_raw() {
            RawWindowHandle::Win32(h) => Some(Hwnd(h.hwnd.get())),
            _ => None,
        }
    }

    pub(crate) fn get(self) -> HWND {
        HWND(self.0 as *mut core::ffi::c_void)
    }
}

impl From<NonZeroIsize> for Hwnd {
    fn from(h: NonZeroIsize) -> Hwnd {
        Hwnd(h.get())
    }
}

impl From<isize> for Hwnd {
    fn from(h: isize) -> Hwnd {
        Hwnd(h)
    }
}
