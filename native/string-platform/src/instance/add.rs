//! `--add`'s channel. The running copy keeps a hidden message-only window, found by its class name; another launch
//! sends it the paths in `WM_COPYDATA` and hears back 1 once that copy has them. Windows finds such a window on the
//! caller's own desktop only, so a launch never reaches another signed-in person's String.

use super::{Hub, Instance, Names, Signal, lock};
use std::cell::RefCell;
use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};
use std::{io, thread};
use windows::Win32::Foundation::{
    ERROR_CLASS_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, WPARAM,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, FindWindowExW, GetMessageW, HWND_MESSAGE,
    MSG, PostMessageW, PostQuitMessage, RegisterClassW, SMTO_ABORTIFHUNG, SMTO_BLOCK,
    SendMessageTimeoutW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE, WM_COPYDATA, WM_DESTROY,
    WNDCLASSW,
};
use windows::core::{HSTRING, PCWSTR};

/// "SADD": the tag String's `WM_COPYDATA` carries, so a stray message from something else is turned down.
const MAGIC: usize = 0x5341_4444;
/// "SPLS": one playlist name followed by its paths. Older receivers refuse the entire message.
const PLAYLIST: usize = 0x5350_4c53;
/// UTF-16 units in one message at most (1 MiB); a longer list goes in several.
const MAX_UNITS: usize = 1 << 19;
/// How long one message may take to be answered.
const ANSWER_MS: u32 = 5000;

/// The same layout as Windows' COPYDATASTRUCT.
#[repr(C)]
struct CopyData {
    tag: usize,
    bytes: u32,
    data: *const u8,
}

thread_local! {
    /// The receiving thread's hub, for the window procedure (which runs on that thread).
    static HUB: RefCell<Option<Arc<Mutex<Hub>>>> = const { RefCell::new(None) };
    static PLAYLISTS: RefCell<Option<mpsc::Sender<PlaylistAdd>>> = const { RefCell::new(None) };
}

type PlaylistAdd = (String, Vec<PathBuf>);

/// The running copy's end: the window and the thread that runs its messages. Dropping it closes both.
pub(super) struct Receiver {
    hwnd: isize,
    thread: Option<thread::JoinHandle<()>>,
    playlists: Option<mpsc::Receiver<PlaylistAdd>>,
}

impl Receiver {
    pub(super) fn open(class: &str, hub: Arc<Mutex<Hub>>) -> io::Result<Receiver> {
        let class = HSTRING::from(class);
        let (tx, rx) = mpsc::channel();
        let (playlist_tx, playlists) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("string-add".into())
            .spawn(move || {
                HUB.with(|h| *h.borrow_mut() = Some(hub));
                PLAYLISTS.with(|p| *p.borrow_mut() = Some(playlist_tx));
                match unsafe { window(&class) } {
                    Ok(hwnd) => {
                        let _ = tx.send(Ok(hwnd.0 as isize));
                        let mut msg = MSG::default();
                        while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
                            unsafe { DispatchMessageW(&msg) };
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Err(e));
                    }
                }
            })?;
        match rx.recv() {
            Ok(Ok(hwnd)) => Ok(Receiver {
                hwnd,
                thread: Some(thread),
                playlists: Some(playlists),
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => {
                let _ = thread.join();
                Err(io::Error::other("the --add window's thread ended"))
            }
        }
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        let posted = unsafe {
            PostMessageW(
                Some(HWND(self.hwnd as *mut _)),
                WM_CLOSE,
                WPARAM(0),
                LPARAM(0),
            )
        };
        if let (Ok(()), Some(t)) = (posted, self.thread.take()) {
            let _ = t.join();
        }
    }
}

unsafe fn window(class: &HSTRING) -> io::Result<HWND> {
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let wc = WNDCLASSW {
            lpfnWndProc: Some(proc),
            hInstance: instance.into(),
            lpszClassName: PCWSTR(class.as_ptr()),
            ..Default::default()
        };
        // registered already by an earlier copy in this process (the tests): the same procedure, so that's fine
        if RegisterClassW(&wc) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
            return Err(io::Error::last_os_error());
        }
        Ok(CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            PCWSTR::null(),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(instance.into()),
            None,
        )?)
    }
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_COPYDATA => LRESULT(isize::from(unsafe { take(lparam) })),
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// One message's paths, handed to the hub. False (turned down) for anything that isn't String's.
unsafe fn take(lparam: LPARAM) -> bool {
    let Some(cd) = (unsafe { (lparam.0 as *const CopyData).as_ref() }) else {
        return false;
    };
    let len = cd.bytes as usize;
    if !matches!(cd.tag, MAGIC | PLAYLIST)
        || !len.is_multiple_of(2)
        || len > MAX_UNITS * 2
        || (len > 0 && cd.data.is_null())
    {
        return false;
    }
    // Windows copied the data into this process for the length of this call
    let bytes = if len == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(cd.data, len) }
    };
    let paths = if cd.tag == PLAYLIST {
        let Some(request) = decode_playlist(bytes) else {
            return false;
        };
        if !PLAYLISTS.with_borrow(|p| p.as_ref().is_some_and(|tx| tx.send(request).is_ok())) {
            return false;
        }
        // Wake the same listener as a plain add. The app drains the named request as one unit.
        Vec::new()
    } else {
        decode(bytes)
    };
    HUB.with(|h| match &*h.borrow() {
        Some(hub) => {
            lock(hub).deliver(Signal::Add(paths));
            true
        }
        None => false,
    })
}

/// The other launch's end: finds the window (waiting up to `wait` for a copy still starting), sends every part.
pub(super) fn send(class: &str, paths: &[PathBuf], wait: Duration) -> bool {
    send_parts(class, MAGIC, &encode(paths), wait)
}

impl Instance {
    /// Take the named adds for this instance, including any received before the app opened.
    pub fn take_playlist_adds(&mut self) -> Option<mpsc::Receiver<(String, Vec<PathBuf>)>> {
        self.add.as_mut()?.playlists.take()
    }
}

impl Names {
    /// Send a named add in one message so a receiver accepts both the playlist and its paths together.
    pub fn send_playlist_add(&self, name: &str, paths: &[PathBuf], wait: Duration) -> bool {
        let Some(class) = self.add.as_deref() else {
            return false;
        };
        let name = name.trim();
        let mut part: Vec<_> = name.encode_utf16().collect();
        if part.is_empty() || part.contains(&0) || paths.is_empty() {
            return false;
        }
        part.push(0);
        for path in paths {
            let start = part.len();
            part.extend(path.as_os_str().encode_wide());
            if part.len() == start || part[start..].contains(&0) || part.len() >= MAX_UNITS {
                return false;
            }
            part.push(0);
        }
        send_parts(class, PLAYLIST, &[part], wait)
    }
}

fn send_parts(class: &str, tag: usize, parts: &[Vec<u16>], wait: Duration) -> bool {
    let class = HSTRING::from(class);
    let deadline = Instant::now() + wait;
    let hwnd = loop {
        if let Ok(h) = unsafe { FindWindowExW(Some(HWND_MESSAGE), None, &class, PCWSTR::null()) } {
            break h;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(40));
    };
    parts.iter().all(|part| {
        let cd = CopyData {
            tag,
            bytes: (part.len() * 2) as u32,
            data: part.as_ptr().cast(),
        };
        let mut answer = 0usize;
        let sent = unsafe {
            SendMessageTimeoutW(
                hwnd,
                WM_COPYDATA,
                WPARAM(0),
                LPARAM(&cd as *const CopyData as isize),
                SMTO_ABORTIFHUNG | SMTO_BLOCK,
                ANSWER_MS,
                Some(&mut answer),
            )
        };
        sent.0 != 0 && answer == 1
    })
}

/// Each path in UTF-16 ending in a 0, in parts of at most [`MAX_UNITS`]. An empty path, or one holding a 0, is left out.
fn encode(paths: &[PathBuf]) -> Vec<Vec<u16>> {
    let mut parts = vec![Vec::new()];
    for path in paths {
        let wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        if wide.is_empty() || wide.contains(&0) || wide.len() + 1 > MAX_UNITS {
            continue;
        }
        if parts
            .last()
            .is_some_and(|p| p.len() + wide.len() + 1 > MAX_UNITS)
        {
            parts.push(Vec::new());
        }
        let part = parts.last_mut().unwrap();
        part.extend(wide);
        part.push(0);
    }
    parts
}

fn decode(bytes: &[u8]) -> Vec<PathBuf> {
    wide_units(bytes)
        .split(|&u| u == 0)
        .filter(|p| !p.is_empty())
        .map(|p| PathBuf::from(OsString::from_wide(p)))
        .collect()
}

fn wide_units(bytes: &[u8]) -> Vec<u16> {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&b| u16::from_le_bytes(b))
        .collect()
}

fn decode_playlist(bytes: &[u8]) -> Option<PlaylistAdd> {
    let units = wide_units(bytes);
    let mut fields = units.strip_suffix(&[0])?.split(|&u| u == 0);
    let name = String::from_utf16(fields.next()?).ok()?;
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let paths = fields
        .map(|field| {
            let path = PathBuf::from(OsString::from_wide(field));
            path.is_absolute().then_some(path)
        })
        .collect::<Option<Vec<_>>>()?;
    (!paths.is_empty()).then(|| (name.to_owned(), paths))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(units: &[u16]) -> Vec<u8> {
        units.iter().flat_map(|u| u.to_le_bytes()).collect()
    }

    #[test]
    fn playlist_message_requires_a_name_and_complete_absolute_paths() {
        let payload = |s: &str| bytes(&s.encode_utf16().collect::<Vec<_>>());
        for bad in [
            "",
            "Mix\0",
            " \0C:\\a\0",
            "Mix\0C:\\a",
            "Mix\0C:\\a\0\0",
            "Mix\0relative\0",
        ] {
            assert!(decode_playlist(&payload(bad)).is_none(), "{bad:?}");
        }
        assert_eq!(
            decode_playlist(&payload(" Études \0C:\\z\0D:\\a\0")),
            Some((
                "Études".into(),
                vec![PathBuf::from(r"C:\z"), PathBuf::from(r"D:\a")]
            ))
        );
    }

    #[test]
    fn paths_round_trip_unicode_and_unpaired_surrogates() {
        let odd =
            OsString::from_wide(&[b'C' as u16, b':' as u16, b'\\' as u16, 0xD800, b'x' as u16]);
        let paths = vec![
            PathBuf::from(r"C:\Music\Sigur Rós\ágætis byrjun.flac"),
            PathBuf::from(r"D:\曲\🎵 song.mp3"),
            PathBuf::from(odd),
        ];
        let parts = encode(&paths);
        assert_eq!(parts.len(), 1);
        assert_eq!(decode(&bytes(&parts[0])), paths);
    }

    #[test]
    fn empty_paths_and_paths_holding_a_zero_are_left_out() {
        let zero = OsString::from_wide(&[b'a' as u16, 0, b'b' as u16]);
        let parts = encode(&[PathBuf::new(), PathBuf::from(zero), PathBuf::from(r"C:\a")]);
        assert_eq!(decode(&bytes(&parts[0])), vec![PathBuf::from(r"C:\a")]);
    }

    #[test]
    fn a_long_list_is_split_into_parts_that_each_fit() {
        let paths: Vec<PathBuf> = (0..3000)
            .map(|i| PathBuf::from(format!(r"C:\{}\{i}.mp3", "m".repeat(200))))
            .collect();
        let parts = encode(&paths);
        assert!(parts.len() >= 2);
        assert!(parts.iter().all(|p| p.len() <= MAX_UNITS));
        let back: Vec<PathBuf> = parts.iter().flat_map(|p| decode(&bytes(p))).collect();
        assert_eq!(back, paths);
    }

    #[test]
    fn decode_ignores_a_trailing_odd_byte_and_empty_entries() {
        let mut b = bytes(&[0, b'a' as u16, 0, 0, b'b' as u16]);
        b.push(7);
        assert_eq!(decode(&b), vec![PathBuf::from("a"), PathBuf::from("b")]);
    }
}
