//! `--add`'s channel. The running copy keeps a hidden message-only window, found by its class name; another launch
//! sends it the paths in `WM_COPYDATA` and hears back 1 once that copy has them. Windows finds such a window on the
//! caller's own desktop only, so a launch never reaches another signed-in person's String.

use super::{Hub, Instance, Names, Owned, Signal, lock};
use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};
use std::{cell::RefCell, collections::VecDeque};
use std::{io, thread};
use windows::Win32::Foundation::{
    ERROR_CLASS_ALREADY_EXISTS, GetLastError, HANDLE, HWND, LPARAM, LRESULT, WAIT_OBJECT_0, WPARAM,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    CreateEventW, INFINITE, OpenMutexW, SYNCHRONIZATION_ACCESS_RIGHTS, SetEvent,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, FindWindowExW, GetWindowThreadProcessId,
    HWND_MESSAGE, MSG, MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, PM_REMOVE, PeekMessageW,
    QS_SENDMESSAGE, RegisterClassW, SMTO_ABORTIFHUNG, SMTO_BLOCK, SendMessageTimeoutW,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_COPYDATA, WM_NCCREATE, WNDCLASSW,
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
    static ROUTER: RefCell<Option<Arc<Mutex<Router>>>> = const { RefCell::new(None) };
}

type PlaylistAdd = (String, Vec<PathBuf>);
type Accept = Box<dyn Fn(Option<String>, Vec<PathBuf>, usize) -> bool + Send>;

struct Router {
    hub: Arc<Mutex<Hub>>,
    accept: Option<Accept>,
    early_named: VecDeque<PlaylistAdd>,
    early_sizes: VecDeque<usize>,
    early_paths: usize,
    early_bytes: usize,
}

impl Router {
    fn new(hub: Arc<Mutex<Hub>>) -> Self {
        Self {
            hub,
            accept: None,
            early_named: VecDeque::new(),
            early_sizes: VecDeque::new(),
            early_paths: 0,
            early_bytes: 0,
        }
    }
    fn deliver(&mut self, name: Option<String>, paths: Vec<PathBuf>, bytes: usize) -> bool {
        if paths.is_empty() {
            return false;
        }
        if let Some(accept) = &self.accept {
            return accept(name, paths, bytes);
        }
        let mut hub = lock(&self.hub);
        if hub.sink.is_none() {
            if self.early_paths + paths.len() > Instance::MAX_ADD_PATHS
                || self.early_bytes + bytes > Instance::MAX_ADD_BYTES
            {
                return false;
            }
            self.early_paths += paths.len();
            self.early_bytes += bytes;
            self.early_sizes.push_back(bytes);
        } else if name.is_some() {
            return false; // the legacy listener only handles plain paths
        }
        if let Some(name) = name {
            self.early_named.push_back((name, paths));
            hub.deliver(Signal::Add(Vec::new()));
        } else {
            hub.deliver(Signal::Add(paths));
        }
        true
    }
}

/// The running copy's end: the window and the thread that runs its messages. Dropping it closes both.
pub(super) struct Receiver {
    stop: Arc<Owned>,
    thread: Option<thread::JoinHandle<()>>,
    router: Arc<Mutex<Router>>,
}

impl Receiver {
    pub(super) fn open(class: &str, hub: Arc<Mutex<Hub>>) -> io::Result<Receiver> {
        let class = HSTRING::from(class);
        let stop = Arc::new(Owned(unsafe {
            CreateEventW(None, true, false, PCWSTR::null())
        }?));
        let stopping = stop.clone();
        let (tx, rx) = mpsc::channel();
        let router = Arc::new(Mutex::new(Router::new(hub)));
        let incoming = router.clone();
        let thread = thread::Builder::new()
            .name("string-add".into())
            .spawn(move || {
                ROUTER.with(|r| *r.borrow_mut() = Some(incoming));
                match unsafe { window(&class) } {
                    Ok(hwnd) => {
                        let _ = tx.send(Ok(()));
                        // PeekMessage services synchronous sends. Only our unnamed event stops the loop.
                        while unsafe {
                            MsgWaitForMultipleObjectsEx(
                                Some(&[stopping.0]),
                                INFINITE,
                                QS_SENDMESSAGE,
                                MWMO_INPUTAVAILABLE,
                            )
                        }
                        .0 == WAIT_OBJECT_0.0 + 1
                        {
                            let _ =
                                unsafe { PeekMessageW(&mut MSG::default(), None, 0, 0, PM_REMOVE) };
                        }
                        let _ = unsafe { DestroyWindow(hwnd) };
                    }
                    Err(e) => {
                        let _ = tx.send(Err(e));
                    }
                }
            })?;
        match rx.recv() {
            Ok(Ok(())) => Ok(Receiver {
                stop,
                thread: Some(thread),
                router,
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
        if let (Ok(()), Some(t)) = (unsafe { SetEvent(self.stop.0) }, self.thread.take()) {
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
        WM_NCCREATE => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        _ => LRESULT(0),
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
    if bytes
        .as_chunks::<2>()
        .0
        .iter()
        .filter(|u| **u == [0, 0])
        .count()
        > Instance::MAX_ADD_PATHS + usize::from(cd.tag == PLAYLIST)
    {
        return false;
    }
    let (name, paths) = if cd.tag == PLAYLIST {
        let Some((name, paths)) = decode_playlist(bytes) else {
            return false;
        };
        (Some(name), paths)
    } else {
        (None, decode(bytes))
    };
    ROUTER.with_borrow(|router| {
        router
            .as_ref()
            .is_some_and(|r| r.lock().unwrap().deliver(name, paths, len))
    })
}

/// The other launch's end: finds the window (waiting up to `wait` for a copy still starting), sends every part.
pub(super) fn send(class: &str, paths: &[PathBuf], wait: Duration) -> bool {
    let Some(prefix) = class.strip_suffix(".Add") else {
        return false;
    };
    send_parts(
        &Names::custom(prefix).mutex,
        class,
        MAGIC,
        &encode(paths),
        wait,
    )
}

impl Instance {
    pub const MAX_ADD_PATHS: usize = 8192;
    pub const MAX_ADD_BYTES: usize = 8 << 20;

    /// Install before the window listener. Admission, including early handover, is serialized with incoming sends.
    pub fn route_adds(
        &mut self,
        accept: impl Fn(Option<String>, Vec<PathBuf>, usize) -> bool + Send + 'static,
    ) {
        let Some(receiver) = &self.add else { return };
        let mut router = receiver.router.lock().unwrap();
        assert!(!self.listening && router.accept.is_none());
        let hub = router.hub.clone();
        lock(&hub).early.retain_mut(|s| {
            let Signal::Add(paths) = s else { return true };
            let (name, paths) = if paths.is_empty() {
                let (name, paths) = router.early_named.pop_front().unwrap();
                (Some(name), paths)
            } else {
                (None, std::mem::take(paths))
            };
            let bytes = router.early_sizes.pop_front().unwrap();
            assert!(
                accept(name, paths, bytes),
                "early adds require an empty receiving queue"
            );
            false
        });
        router.accept = Some(Box::new(accept));
    }
}

impl Names {
    /// Verify the receiver against this instance's mutex before giving it any paths.
    pub fn send_paths(&self, paths: &[PathBuf], wait: Duration) -> bool {
        self.add
            .as_deref()
            .is_some_and(|class| send_parts(&self.mutex, class, MAGIC, &encode(paths), wait))
    }
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
        send_parts(&self.mutex, class, PLAYLIST, &[part], wait)
    }
}

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQueryMutant(
        handle: HANDLE,
        class: u32,
        info: *mut [usize; 2],
        size: u32,
        written: *mut u32,
    ) -> i32;
}

fn mutex_owner(mutex: HANDLE) -> Option<u32> {
    // MutantOwnerInformation returns CLIENT_ID (process, thread). A name or a handle alone
    // does not identify the thread currently owning the mutex.
    let mut id = [0usize; 2];
    let size = std::mem::size_of_val(&id) as u32;
    let mut written = 0;
    let status = unsafe { NtQueryMutant(mutex, 1, &mut id, size, &mut written) };
    (status >= 0 && written == size && id[0] != 0)
        .then(|| u32::try_from(id[0]).ok())
        .flatten()
}

fn owned_window(hwnd: HWND, mutex: HANDLE) -> bool {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid != 0 && mutex_owner(mutex) == Some(pid)
}

fn send_parts(mutex: &str, class: &str, tag: usize, parts: &[Vec<u16>], wait: Duration) -> bool {
    let Ok(handle) = (unsafe {
        OpenMutexW(
            SYNCHRONIZATION_ACCESS_RIGHTS(1),
            false,
            &HSTRING::from(mutex),
        )
    }) else {
        return false;
    };
    let mutex = Owned(handle);
    let class = HSTRING::from(class);
    let deadline = Instant::now() + wait;
    let hwnd = 'find: loop {
        let mut after = None;
        while let Ok(h) =
            unsafe { FindWindowExW(Some(HWND_MESSAGE), after, &class, PCWSTR::null()) }
        {
            if owned_window(h, mutex.0) {
                break 'find h;
            }
            if Some(h) == after || Instant::now() >= deadline {
                return false;
            }
            after = Some(h);
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(40));
    };
    parts.iter().all(|part| {
        if !owned_window(hwnd, mutex.0) {
            return false;
        }
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
    fn early_plain_and_named_adds_share_the_same_limits() {
        let mut r = Router::new(Default::default());
        let path = || vec![PathBuf::from(r"C:\Music\a.wav")];
        assert!(r.deliver(None, path(), Instance::MAX_ADD_BYTES - 8));
        assert!(r.deliver(Some("List".into()), path(), 8));
        assert!(!r.deliver(None, path(), 1));
        assert_eq!(lock(&r.hub).early.len(), 2);
        let mut r = Router::new(Default::default());
        assert!(r.deliver(None, vec![PathBuf::new(); Instance::MAX_ADD_PATHS], 64));
        assert!(!r.deliver(Some("List".into()), path(), 2));
    }

    #[test]
    fn a_destroy_notification_does_not_end_the_message_thread() {
        use windows::Win32::UI::WindowsAndMessaging::{WM_DESTROY, WM_QUIT};
        std::thread::spawn(|| unsafe {
            proc(HWND::default(), WM_DESTROY, WPARAM(0), LPARAM(0));
            assert!(
                !PeekMessageW(&mut MSG::default(), None, WM_QUIT, WM_QUIT, PM_REMOVE).as_bool()
            );
        })
        .join()
        .unwrap();
    }

    #[test]
    fn receiver_verification_requires_a_current_kernel_mutex_owner() {
        use super::super::{Claim, claim_quietly};
        use windows::Win32::System::Threading::CreateMutexW;
        let names = Names::custom(&format!("StringOwnerTest.{}", std::process::id()));
        let Claim::First(me) = claim_quietly(&names).unwrap() else {
            panic!("expected isolated instance")
        };
        let class = HSTRING::from(names.add.as_deref().unwrap());
        let hwnd =
            unsafe { FindWindowExW(Some(HWND_MESSAGE), None, &class, PCWSTR::null()) }.unwrap();
        assert!(owned_window(hwnd, me._mutex.0));
        let unowned = Owned(unsafe { CreateMutexW(None, false, PCWSTR::null()) }.unwrap());
        assert!(!owned_window(hwnd, unowned.0));
        let different = Names {
            mutex: format!("{}.other", names.mutex),
            ..names.clone()
        };
        let sample = [PathBuf::from(r"C:\Music\example.wav")];
        assert!(!different.send_paths(&sample, Duration::ZERO));
        assert!(!different.send_playlist_add("Mix", &sample, Duration::ZERO));
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
