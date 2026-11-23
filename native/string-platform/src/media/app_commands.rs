//! Transport keys delivered directly to the focused HWND, including older keyboards and remotes.
use super::MediaButton;
use crate::Hwnd;
use std::io;
use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    UI::{
        Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::{FAPPCOMMAND_MASK, WM_APPCOMMAND},
    },
};

pub struct AppCommands {
    hwnd: HWND,
    _callback: Box<Callback>,
}

struct Callback(Box<dyn Fn(MediaButton)>);
const SUBCLASS: usize = 0x5354_524d;

impl AppCommands {
    /// Attach and drop on the window's thread. The callback queues work for the player.
    pub fn new(hwnd: Hwnd, post: impl Fn(MediaButton) + 'static) -> io::Result<Self> {
        let callback = Box::new(Callback(Box::new(post)));
        let data = &*callback as *const Callback as usize;
        if !unsafe { SetWindowSubclass(hwnd.get(), Some(receive), SUBCLASS, data) }.as_bool() {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            hwnd: hwnd.get(),
            _callback: callback,
        })
    }
}

impl Drop for AppCommands {
    fn drop(&mut self) {
        unsafe {
            let _ = RemoveWindowSubclass(self.hwnd, Some(receive), SUBCLASS);
        }
    }
}

unsafe extern "system" fn receive(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _: usize,
    data: usize,
) -> LRESULT {
    if msg == WM_APPCOMMAND {
        // GET_APPCOMMAND_LPARAM strips the high-word device flags (key, mouse or OEM).
        let command = ((lp.0 as u32 >> 16) & !FAPPCOMMAND_MASK) as u16;
        let button = match command {
            11 => Some(MediaButton::Next),
            12 => Some(MediaButton::Previous),
            13 => Some(MediaButton::Stop),
            14 => Some(MediaButton::Toggle),
            46 => Some(MediaButton::Play),
            47 => Some(MediaButton::Pause),
            49 => Some(MediaButton::FastForward),
            50 => Some(MediaButton::Rewind),
            _ => None,
        };
        if let Some(b) = button {
            let callback = unsafe { &*(data as *const Callback) };
            (callback.0)(b);
            // TRUE consumes it. Forwarding it as well can dispatch the same key twice.
            return LRESULT(1);
        }
    }
    unsafe { DefSubclassProc(hwnd, msg, wp, lp) }
}
