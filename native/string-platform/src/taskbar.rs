//! Previous, play/pause and next under String's taskbar thumbnail (ITaskbarList3), as App.cs had them: drawn glyphs
//! that turn dark when Windows' taskbar is light, the middle one swapping between play and pause, and presses delivered
//! to a callback. The window is subclassed to hear the presses (THBN_CLICKED) and the taskbar's "button created".
//!
//! Lives on the window's own thread (the one running its message loop): create it there, call it there; the callback
//! runs there too.

use crate::glyphs::{self, Glyph};
use crate::{Hwnd, com};
use std::cell::{Cell, RefCell};
use std::io;
use windows::Win32::Foundation::{ERROR_SUCCESS, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::Shell::{
    DefSubclassProc, ITaskbarList3, RemoveWindowSubclass, SetWindowSubclass, THB_FLAGS, THB_ICON,
    THB_TOOLTIP, THBF_ENABLED, THBF_HIDDEN, THBN_CLICKED, THUMBBUTTON, TaskbarList,
};
use windows::Win32::UI::WindowsAndMessaging::{
    ChangeWindowMessageFilterEx, DestroyIcon, HICON, IsWindowVisible, MSGFLT_ALLOW,
    RegisterWindowMessageW, SM_CXSMICON, WM_COMMAND, WM_DPICHANGED, WM_SETTINGCHANGE,
};
use windows::core::{PCWSTR, w};

/// Which thumbnail button was pressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThumbButton {
    Previous,
    /// The middle one: play while paused, pause while playing (App.cs's "toggle").
    PlayPause,
    Next,
}

const IDS: [u32; 3] = [0x5301, 0x5302, 0x5303];
const SUBCLASS: usize = 0x5354_5242;

/// The three buttons on one window. Hidden again and unhooked when dropped. Don't drop it from inside its callback.
pub struct ThumbButtons {
    inner: Box<Inner>,
}

struct Inner {
    hwnd: HWND,
    publish: bool,
    created: u32,
    list: RefCell<Option<ITaskbarList3>>,
    added: Cell<bool>,
    playing: Cell<bool>,
    light: Cell<bool>,
    icons: RefCell<Icons>,
    on_press: RefCell<Box<dyn FnMut(ThumbButton)>>,
}

impl ThumbButtons {
    /// Hooks the window and adds the buttons as soon as the window has its taskbar button (right away if it has one).
    pub fn new(
        hwnd: Hwnd,
        on_press: impl FnMut(ThumbButton) + 'static,
    ) -> io::Result<ThumbButtons> {
        Self::attach(hwnd, on_press, true)
    }

    /// The same window procedure on the off-screen self-test window, without adding shell buttons.
    pub fn messages_only(
        hwnd: Hwnd,
        on_press: impl FnMut(ThumbButton) + 'static,
    ) -> io::Result<Self> {
        Self::attach(hwnd, on_press, false)
    }

    fn attach(
        hwnd: Hwnd,
        on_press: impl FnMut(ThumbButton) + 'static,
        publish: bool,
    ) -> io::Result<Self> {
        com::init();
        let h = hwnd.get();
        let created = unsafe { RegisterWindowMessageW(w!("TaskbarButtonCreated")) };
        unsafe {
            // an elevated String would otherwise never hear from the taskbar, which isn't
            let _ = ChangeWindowMessageFilterEx(h, created, MSGFLT_ALLOW, None);
            let _ = ChangeWindowMessageFilterEx(h, WM_COMMAND, MSGFLT_ALLOW, None);
        }
        let light = light_taskbar();
        let inner = Box::new(Inner {
            hwnd: h,
            publish,
            created,
            list: RefCell::new(None),
            added: Cell::new(false),
            playing: Cell::new(false),
            light: Cell::new(light),
            icons: RefCell::new(Icons::draw(h, light)?),
            on_press: RefCell::new(Box::new(on_press)),
        });
        let data = &*inner as *const Inner as usize;
        if !unsafe { SetWindowSubclass(h, Some(subclass), SUBCLASS, data) }.as_bool() {
            return Err(io::Error::last_os_error());
        }
        inner.add();
        Ok(ThumbButtons { inner })
    }

    /// The middle button shows pause while music plays, play while it doesn't.
    pub fn set_playing(&self, playing: bool) {
        if self.inner.playing.replace(playing) != playing {
            self.inner.update(false);
        }
    }

    pub fn playing(&self) -> bool {
        self.inner.playing.get()
    }
}

impl Drop for ThumbButtons {
    fn drop(&mut self) {
        // buttons can't be taken off a window again: hidden instead
        self.inner.update(true);
        unsafe {
            let _ = RemoveWindowSubclass(self.inner.hwnd, Some(subclass), SUBCLASS);
        }
    }
}

impl Inner {
    fn list(&self) -> Option<ITaskbarList3> {
        let mut list = self.list.borrow_mut();
        if list.is_none() {
            let made: Option<ITaskbarList3> =
                unsafe { CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER) }.ok();
            *list = made.filter(|l| unsafe { l.HrInit() }.is_ok());
        }
        list.clone()
    }

    fn buttons(&self, hidden: bool) -> [THUMBBUTTON; 3] {
        let icons = self.icons.borrow();
        let playing = self.playing.get();
        let parts = [
            (icons.previous, "Previous"),
            if playing {
                (icons.pause, "Pause")
            } else {
                (icons.play, "Play")
            },
            (icons.next, "Next"),
        ];
        let mut out = [THUMBBUTTON::default(); 3];
        for (b, (&id, (icon, tip))) in out.iter_mut().zip(IDS.iter().zip(parts)) {
            b.dwMask = THB_ICON | THB_TOOLTIP | THB_FLAGS;
            b.iId = id;
            b.hIcon = icon;
            b.dwFlags = if hidden { THBF_HIDDEN } else { THBF_ENABLED };
            for (d, s) in b.szTip.iter_mut().zip(tip.encode_utf16()) {
                *d = s;
            }
        }
        out
    }

    /// Adds the buttons, once the window has a taskbar button (a hidden window has none: it comes when shown).
    fn add(&self) {
        if !self.publish || !unsafe { IsWindowVisible(self.hwnd) }.as_bool() {
            return;
        }
        let Some(list) = self.list() else { return };
        let b = self.buttons(false);
        let ok = unsafe {
            list.ThumbBarAddButtons(self.hwnd, &b).is_ok()
                || list.ThumbBarUpdateButtons(self.hwnd, &b).is_ok()
        };
        self.added.set(ok);
    }

    fn update(&self, hidden: bool) {
        if !self.added.get() {
            return;
        }
        if let Some(list) = self.list() {
            let _ = unsafe { list.ThumbBarUpdateButtons(self.hwnd, &self.buttons(hidden)) };
        }
    }

    /// New glyphs for a new screen scale or taskbar colour; the old icons go once the taskbar has the new ones.
    fn redraw(&self) {
        let light = light_taskbar();
        self.light.set(light);
        if let Ok(icons) = Icons::draw(self.hwnd, light) {
            let old = self.icons.replace(icons);
            self.update(false);
            drop(old);
        }
    }
}

unsafe extern "system" fn subclass(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    let inner = unsafe { &*(data as *const Inner) };
    if msg == inner.created {
        // a new taskbar button (the window was shown, or Explorer started again): a fresh list, the buttons again
        inner.list.replace(None);
        inner.add();
    } else if msg == WM_COMMAND && (wparam.0 >> 16) & 0xffff == THBN_CLICKED as usize {
        if let Some(b) = pressed((wparam.0 & 0xffff) as u32) {
            if let Ok(mut f) = inner.on_press.try_borrow_mut() {
                f(b);
            }
            return LRESULT(0);
        }
    } else if msg == WM_SETTINGCHANGE && is_colour_change(lparam) {
        if light_taskbar() != inner.light.get() {
            inner.redraw();
        }
    } else if msg == WM_DPICHANGED {
        let r = unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
        inner.redraw();
        return r;
    }
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

fn pressed(id: u32) -> Option<ThumbButton> {
    match IDS.iter().position(|&i| i == id)? {
        0 => Some(ThumbButton::Previous),
        1 => Some(ThumbButton::PlayPause),
        _ => Some(ThumbButton::Next),
    }
}

/// WM_SETTINGCHANGE with "ImmersiveColorSet": the light or dark setting changed.
fn is_colour_change(lparam: LPARAM) -> bool {
    lparam.0 != 0
        && unsafe { PCWSTR(lparam.0 as *const u16).to_string() }
            .is_ok_and(|s| s == "ImmersiveColorSet")
}

/// Windows' taskbar is light ("Choose your default Windows mode": Light).
fn light_taskbar() -> bool {
    let mut value = 0u32;
    let mut size = size_of::<u32>() as u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
            w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    r == ERROR_SUCCESS && value == 1
}

struct Icons {
    previous: HICON,
    play: HICON,
    pause: HICON,
    next: HICON,
}

impl Icons {
    fn draw(hwnd: HWND, light: bool) -> io::Result<Icons> {
        let dpi = match unsafe { GetDpiForWindow(hwnd) } {
            0 => 96,
            d => d,
        };
        let size = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, dpi) }.max(16) as u32;
        let ink = glyphs::ink(light);
        let mut icons = Icons {
            previous: HICON::default(),
            play: HICON::default(),
            pause: HICON::default(),
            next: HICON::default(),
        };
        icons.previous = glyphs::icon(Glyph::Previous, size, ink)?;
        icons.play = glyphs::icon(Glyph::Play, size, ink)?;
        icons.pause = glyphs::icon(Glyph::Pause, size, ink)?;
        icons.next = glyphs::icon(Glyph::Next, size, ink)?;
        Ok(icons)
    }
}

impl Drop for Icons {
    fn drop(&mut self) {
        for i in [self.previous, self.play, self.pause, self.next] {
            if !i.is_invalid() {
                unsafe {
                    let _ = DestroyIcon(i);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    // On a hidden window of the tests' own: it has no taskbar button, so the person's taskbar is never touched.
    use super::*;
    use crate::test_window::TestWindow;
    use std::rc::Rc;
    use windows::Win32::UI::WindowsAndMessaging::SendMessageW;

    fn press(w: &TestWindow, id: u32, code: u32) {
        unsafe {
            SendMessageW(
                w.hwnd.get(),
                WM_COMMAND,
                Some(WPARAM((code << 16 | id) as usize)),
                Some(LPARAM(0)),
            );
        }
    }

    #[test]
    fn presses_reach_the_callback() {
        let w = TestWindow::new();
        let got = Rc::new(RefCell::new(Vec::new()));
        let seen = got.clone();
        let thumbs = ThumbButtons::new(w.hwnd, move |b| seen.borrow_mut().push(b)).unwrap();
        for id in IDS {
            press(&w, id, THBN_CLICKED);
        }
        press(&w, IDS[0], 0); // a menu's WM_COMMAND with the same number isn't a press
        assert_eq!(
            *got.borrow(),
            [
                ThumbButton::Previous,
                ThumbButton::PlayPause,
                ThumbButton::Next
            ]
        );
        assert!(
            !thumbs.inner.added.get(),
            "a hidden window has no taskbar button to add to"
        );
        drop(thumbs);
        press(&w, IDS[2], THBN_CLICKED);
        assert_eq!(got.borrow().len(), 3, "unhooked when dropped");
    }

    #[test]
    fn play_and_pause_swap() {
        let w = TestWindow::new();
        let thumbs = ThumbButtons::new(w.hwnd, |_| {}).unwrap();
        let tip = |b: &THUMBBUTTON| {
            String::from_utf16_lossy(&b.szTip)
                .trim_end_matches('\0')
                .to_string()
        };
        let b = thumbs.inner.buttons(false);
        assert_eq!(
            [tip(&b[0]), tip(&b[1]), tip(&b[2])],
            ["Previous", "Play", "Next"]
        );
        assert!(
            b.iter()
                .all(|b| !b.hIcon.is_invalid() && b.dwFlags == THBF_ENABLED)
        );
        let play_icon = b[1].hIcon;
        thumbs.set_playing(true);
        assert!(thumbs.playing());
        let b = thumbs.inner.buttons(false);
        assert_eq!(tip(&b[1]), "Pause");
        assert_ne!(b[1].hIcon, play_icon);
        assert!(
            thumbs
                .inner
                .buttons(true)
                .iter()
                .all(|b| b.dwFlags == THBF_HIDDEN)
        );
        thumbs.inner.redraw();
        assert!(
            thumbs
                .inner
                .buttons(false)
                .iter()
                .all(|b| !b.hIcon.is_invalid())
        );
    }

    #[test]
    fn colour_change_message() {
        let name: Vec<u16> = "ImmersiveColorSet\0".encode_utf16().collect();
        assert!(is_colour_change(LPARAM(name.as_ptr() as isize)));
        let other: Vec<u16> = "Policy\0".encode_utf16().collect();
        assert!(!is_colour_change(LPARAM(other.as_ptr() as isize)));
        assert!(!is_colour_change(LPARAM(0)));
        let _ = light_taskbar(); // reads the setting, whichever it is
    }
}
