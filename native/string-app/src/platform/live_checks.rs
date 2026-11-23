//! End-to-end input on the self-test HWND. Each action waits for the engine's status and
//! the queue cursor before issuing another key. No desktop-wide input or active media session.
use crate::{smoke::Case, view::View};
use std::{
    io,
    time::{Duration, Instant},
};
use string_platform::Hwnd;
use windows::{
    Media::SystemMediaTransportControlsButton as Button,
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        UI::{
            Shell::{DefSubclassProc, SetWindowSubclass, THBN_CLICKED},
            WindowsAndMessaging::{SendMessageW, WM_APPCOMMAND, WM_COMMAND},
        },
    },
};

// Install first (under the production hook). If a regressed handler forgets a command, keep
// DefWindowProc from forwarding the synthetic media key to the desktop. No state to free.
pub(super) fn contain_keys(hwnd: Hwnd) -> io::Result<()> {
    unsafe extern "system" fn contain(
        h: HWND,
        m: u32,
        w: WPARAM,
        l: LPARAM,
        _: usize,
        _: usize,
    ) -> LRESULT {
        if m == WM_APPCOMMAND {
            LRESULT(0)
        } else {
            unsafe { DefSubclassProc(h, m, w, l) }
        }
    }
    if !unsafe { SetWindowSubclass(HWND(hwnd.0 as _), Some(contain), 1, 0) }.as_bool() {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(super) fn thumbnail(hwnd: Hwnd, id: usize) {
    unsafe {
        SendMessageW(
            HWND(hwnd.0 as _),
            WM_COMMAND,
            Some(WPARAM(((THBN_CLICKED as usize) << 16) | id)),
            None,
        );
    }
}

#[derive(Clone, Copy, Debug)]
enum Input {
    Thumb(usize),
    Key(u16),
    Smtc(Button),
}

// Each row gives the action, the expected queue index and whether output must be playing.
const STEPS: &[(Input, usize, bool)] = &[
    (Input::Thumb(0x5302), 0, false),
    (Input::Thumb(0x5302), 0, true),
    (Input::Thumb(0x5303), 1, true),
    (Input::Thumb(0x5301), 0, true),
    (Input::Key(14), 0, false),
    (Input::Key(14), 0, true),
    (Input::Key(11), 1, true),
    (Input::Key(12), 0, true),
    (Input::Key(47), 0, false),
    (Input::Key(46), 0, true),
    (Input::Smtc(Button::Pause), 0, false),
    (Input::Smtc(Button::Play), 0, true),
    (Input::Smtc(Button::Next), 1, true),
    (Input::Smtc(Button::Previous), 0, true),
];

#[derive(Default)]
pub(super) struct Probe {
    sent: usize,
    prior_key: Option<u64>,
    due: Option<Instant>,
}

pub(crate) const TRANSPORT: Case = Case {
    name: "windows-transport",
    prepare: |v| {
        let songs = &v.s.models.songs;
        // Even a one-song fixture can visit two separate queue entries and engine keys.
        let pair = [songs[0].clone(), songs.get(1).unwrap_or(&songs[0]).clone()];
        v.s.state.queue = Default::default();
        v.platform.live_check = Probe {
            prior_key: v.st.key,
            ..Default::default()
        };
        v.s.play_ids(v.player, &pair, 0, v.context());
    },
    ready,
};

fn ready(v: &mut View<'_>) -> bool {
    let p = &mut v.platform.live_check;
    let previous = p.sent.checked_sub(1).map(|i| STEPS[i]);
    let (index, playing) = previous.map_or((0, true), |(_, i, on)| (i, on));
    let changed_track = p.sent == 0 || (p.sent >= 2 && STEPS[p.sent - 2].1 != index);
    let confirmed = v.st.key.is_some()
        && (v.st.key != p.prior_key) == changed_track
        && v.st.playing == playing
        && v.s.state.queue.at() == Some(index)
        && (!playing || v.st.position > 0.0);
    if !confirmed {
        if p.due.is_some_and(|t| Instant::now() >= t) {
            v.platform.log.write(
                "transport self-test",
                format!(
                    "After {previous:?}: queue index {:?}, output {:?}",
                    v.s.state.queue.at(),
                    v.st
                ),
            );
            p.due = None;
        }
        return false;
    }
    let Some(&(input, _, _)) = STEPS.get(p.sent) else {
        return true;
    };
    let Some(hwnd) = v.platform.hwnd.filter(|_| v.force_mute && v.platform.smoke) else {
        return false;
    };
    p.prior_key = v.st.key;
    p.sent += 1;
    p.due = Some(Instant::now() + Duration::from_secs(2));
    match input {
        Input::Thumb(id) => thumbnail(hwnd, id),
        Input::Key(code) => unsafe {
            let h = HWND(hwnd.0 as _);
            if SendMessageW(
                h,
                WM_APPCOMMAND,
                Some(WPARAM(hwnd.0 as usize)),
                Some(LPARAM((code as isize) << 16)),
            ) != LRESULT(1)
            {
                v.platform
                    .log
                    .write("transport self-test", format!("Unhandled media key {code}"));
            }
        },
        Input::Smtc(b) => {
            if let Some(media) = &v.platform.media {
                v.platform
                    .optional("self-test media button", media.smoke_press(b));
            }
        }
    }
    false
}
