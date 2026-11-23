//! One copy of String at a time, compatible with String 1.x: the same mutex (`StringApp.SingleInstance`) and events
//! (`StringApp.Show`, `StringApp.Quit`), so a second launch of either version brings the running one forward, and
//! `--quit` (the 1.x updater's step) asks either to save and close. `--add` hands paths to a running String 2.0
//! ([`send_add`]), which adds them as if they were dropped on its window.
//!
//! ```no_run
//! use string_platform::instance::{self, Claim, Names, Signal};
//! let names = Names::string();
//! if std::env::args().any(|a| a == "--quit") {
//!     instance::request_quit(&names);
//!     return;
//! }
//! let Claim::First(mut me) = instance::claim(&names).unwrap() else { return }; // the running copy comes forward
//! me.listen(|signal| match signal {
//!     Signal::Show => { /* on the window's thread: window::bring_to_front */ }
//!     Signal::Quit => { /* save, then close */ }
//!     Signal::Add(paths) => { /* add them as if dropped on the window, then scan */ }
//! });
//! ```

mod add;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::{io, thread};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, WAIT_OBJECT_0,
};
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, INFINITE, OpenEventW, SetEvent,
    WaitForMultipleObjects,
};
use windows::core::{HSTRING, PCWSTR};

/// The names one copy is known by. [`Names::string`] are String's own, shared with String 1.x.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Names {
    pub mutex: String,
    /// Set by a second launch: the running copy comes forward. `None`: nothing to hear (a self-test).
    pub show: Option<String>,
    /// Set by `--quit`: the running copy saves and closes. `None`: nothing to hear (a self-test).
    pub quit: Option<String>,
    /// The class of the hidden window `--add` hands its paths to (String 2.0's alone: String 1.x has none). `None`:
    /// nothing to hear (a self-test).
    pub add: Option<String>,
}

impl Names {
    /// String's own names, the same as String 1.x's.
    pub fn string() -> Names {
        Names::custom("StringApp")
    }

    /// A self-test's (App.cs's `--smoke-test`): a mutex of this process's own and no named events, so a test copy never
    /// meets the person's String and nothing outside can raise or close it.
    pub fn smoke() -> Names {
        Names {
            mutex: format!("StringApp.Smoke.{}", std::process::id()),
            show: None,
            quit: None,
            add: None,
        }
    }

    /// `<prefix>.SingleInstance`, `<prefix>.Show`, `<prefix>.Quit` and `<prefix>.Add`.
    pub fn custom(prefix: &str) -> Names {
        Names {
            mutex: format!("{prefix}.SingleInstance"),
            show: Some(format!("{prefix}.Show")),
            quit: Some(format!("{prefix}.Quit")),
            add: Some(format!("{prefix}.Add")),
        }
    }
}

/// What another launch asked of the running copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Signal {
    /// A second launch: bring the window forward (restore it if minimised).
    Show,
    /// `--quit`: save and close. Every request counts: one turned down doesn't stop the next being heard.
    Quit,
    /// `--add`: these paths, as the other launch gave them (it sends absolute ones), to add as if they were dropped
    /// on the window. A long list may come in several parts, in order.
    Add(Vec<PathBuf>),
}

pub enum Claim {
    /// This is the only copy: keep the [`Instance`] for as long as the app runs.
    First(Instance),
    /// Another copy runs and has been asked to come forward: exit.
    AlreadyRunning,
}

/// Holding the single-instance mutex. Dropping it lets the next launch be the first.
pub struct Instance {
    _mutex: Owned,
    show: Option<Owned>,
    quit: Option<Owned>,
    stop: Owned,
    /// Where every signal goes: the listener once there is one, kept until then.
    hub: Arc<Mutex<Hub>>,
    add: Option<add::Receiver>,
    add_error: Option<io::Error>,
    listening: bool,
    watcher: Option<thread::JoinHandle<()>>,
}

type Sink = Box<dyn FnMut(Signal) + Send>;

#[derive(Default)]
struct Hub {
    sink: Option<Sink>,
    early: Vec<Signal>,
}

impl Hub {
    fn deliver(&mut self, signal: Signal) {
        match &mut self.sink {
            Some(sink) => sink(signal),
            None => self.early.push(signal),
        }
    }
}

fn lock(hub: &Mutex<Hub>) -> std::sync::MutexGuard<'_, Hub> {
    hub.lock().unwrap_or_else(|e| e.into_inner())
}

/// Takes the mutex, or (another copy has it) asks that copy to come forward.
pub fn claim(names: &Names) -> io::Result<Claim> {
    claim_as(names, true, || Ok(None))
}

/// As [`claim`], but a copy already running is left as it is, not brought forward: for `--add`, whose caller keeps its
/// own window in front.
pub fn claim_quietly(names: &Names) -> io::Result<Claim> {
    claim_as(names, false, || Ok(None))
}

fn claim_as(
    names: &Names,
    raise: bool,
    prepare_adds: impl FnOnce() -> io::Result<Option<add::Accept>>,
) -> io::Result<Claim> {
    let mutex = unsafe { CreateMutexW(None, true, &HSTRING::from(names.mutex.as_str())) }?;
    let existed = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    let mutex = Owned(mutex);
    let show = names.show.as_deref().map(event).transpose()?;
    if existed {
        if let Some(show) = show.as_ref().filter(|_| raise) {
            // Windows lets only the program just started take the foreground: hand that to the running copy (not from
            // the tests, which must leave the person's desktop alone)
            #[cfg(not(test))]
            unsafe {
                let _ = windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(
                    windows::Win32::UI::WindowsAndMessaging::ASFW_ANY,
                );
            }
            unsafe { SetEvent(show.0) }?;
        }
        return Ok(Claim::AlreadyRunning);
    }
    let quit = names.quit.as_deref().map(event).transpose()?;
    let stop = Owned(unsafe { CreateEventW(None, true, false, PCWSTR::null()) }?);
    // `--add`'s window opens at once, before the app's own: a helper's paths sent while String is still starting are
    // kept for its listener. Without it String runs as usual; `add_error` says why.
    let hub = Arc::new(Mutex::new(Hub::default()));
    let accept = prepare_adds()?;
    let (add, add_error) = match names.add.as_deref() {
        Some(class) => match add::Receiver::open(class, hub.clone(), accept) {
            Ok(r) => (Some(r), None),
            Err(e) => (None, Some(e)),
        },
        None => (None, None),
    };
    Ok(Claim::First(Instance {
        _mutex: mutex,
        show,
        quit,
        stop,
        hub,
        add,
        add_error,
        listening: false,
        watcher: None,
    }))
}

/// `--add`: hands `paths` to the running copy, which adds them as if they were dropped on its window. Waits up to `wait`
/// for a copy that is still starting. True once that copy has them; false when none took them: none runs, or the one
/// running has no such channel (String 1.x, a self-test).
pub fn send_add(names: &Names, paths: &[PathBuf], wait: Duration) -> bool {
    names.send_paths(paths, wait)
}

/// `--quit`: asks the running copy (String 1.x or this one) to save and close. False when none runs.
pub fn request_quit(names: &Names) -> bool {
    let Some(name) = &names.quit else {
        return false;
    };
    match unsafe { OpenEventW(EVENT_MODIFY_STATE, false, &HSTRING::from(name.as_str())) } {
        Ok(h) => {
            let h = Owned(h);
            unsafe { SetEvent(h.0) }.is_ok()
        }
        Err(_) => false,
    }
}

impl Instance {
    /// Hears second launches, `--quit` and `--add` on threads of their own and calls `on_signal` there (forward it to
    /// the window's thread); paths added before this are handed over first. Once; later calls do nothing. Stops when
    /// the instance is dropped.
    pub fn listen(&mut self, on_signal: impl FnMut(Signal) + Send + 'static) {
        if self.listening {
            return;
        }
        self.listening = true;
        {
            let mut hub = lock(&self.hub);
            let mut sink: Sink = Box::new(on_signal);
            for signal in std::mem::take(&mut hub.early) {
                sink(signal);
            }
            hub.sink = Some(sink);
        }
        let mut handles = vec![self.stop.0.0 as isize];
        let mut kinds = vec![None];
        for (h, kind) in [(&self.show, Signal::Show), (&self.quit, Signal::Quit)] {
            if let Some(h) = h {
                handles.push(h.0.0 as isize);
                kinds.push(Some(kind));
            }
        }
        if handles.len() == 1 {
            return;
        }
        let hub = self.hub.clone();
        let spawned = thread::Builder::new()
            .name("string-instance".into())
            .spawn(move || {
                let handles: Vec<HANDLE> = handles
                    .iter()
                    .map(|&h| HANDLE(h as *mut core::ffi::c_void))
                    .collect();
                loop {
                    let r = unsafe { WaitForMultipleObjects(&handles, false, INFINITE) };
                    match kinds.get(r.0.wrapping_sub(WAIT_OBJECT_0.0) as usize) {
                        Some(Some(kind)) => lock(&hub).deliver(kind.clone()),
                        _ => break, // the stop event, or a failed wait
                    }
                }
            });
        self.watcher = spawned.ok();
    }

    /// Why `--add` can't reach this copy, when its window couldn't be made (String runs on without it).
    pub fn add_error(&self) -> Option<&io::Error> {
        self.add_error.as_ref()
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        // `--add`'s window goes before the mutex, so a launch that finds it always finds this copy still running
        drop(self.add.take());
        if let Some(w) = self.watcher.take() {
            unsafe {
                let _ = SetEvent(self.stop.0);
            }
            let _ = w.join();
        }
    }
}

fn event(name: &str) -> io::Result<Owned> {
    // auto-reset, as String 1.x's: one set wakes one wait, and a set before anyone waits isn't lost
    Ok(Owned(unsafe {
        CreateEventW(None, false, false, &HSTRING::from(name))
    }?))
}

struct Owned(HANDLE);
// a kernel handle may be used and closed from any thread
unsafe impl Send for Owned {}
unsafe impl Sync for Owned {}
impl Drop for Owned {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    // Never String's own names: a test that set StringApp.Show or StringApp.Quit would raise or close the person's String,
    // and one that sent to StringApp.Add would add to their library.
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

    fn names() -> Names {
        static N: AtomicU32 = AtomicU32::new(0);
        let n = Names::custom(&format!(
            "StringPlatformTest.{}.{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        assert!(!n.mutex.starts_with("StringApp"));
        n
    }

    fn first(names: &Names) -> (Instance, mpsc::Receiver<Signal>) {
        let Claim::First(mut me) = claim(names).unwrap() else {
            panic!("expected to be the first copy")
        };
        let (tx, rx) = mpsc::channel();
        me.listen(move |s| {
            let _ = tx.send(s);
        });
        (me, rx)
    }

    #[test]
    fn string_1x_names() {
        let n = Names::string();
        assert_eq!(n.mutex, "StringApp.SingleInstance");
        assert_eq!(n.show.as_deref(), Some("StringApp.Show"));
        assert_eq!(n.quit.as_deref(), Some("StringApp.Quit"));
        assert_eq!(n.add.as_deref(), Some("StringApp.Add"));
    }

    #[test]
    fn smoke_names_are_this_process_own() {
        let n = Names::smoke();
        assert_eq!(n.mutex, format!("StringApp.Smoke.{}", std::process::id()));
        assert_eq!((n.show, n.quit, n.add), (None, None, None));
    }

    #[test]
    fn a_second_launch_brings_the_first_forward() {
        let names = names();
        let (_me, rx) = first(&names);
        assert!(matches!(claim(&names).unwrap(), Claim::AlreadyRunning));
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(Signal::Show));
        assert!(matches!(claim(&names).unwrap(), Claim::AlreadyRunning));
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(Signal::Show));
    }

    #[test]
    fn quit_reaches_the_running_copy_every_time() {
        let names = names();
        let (_me, rx) = first(&names);
        assert!(request_quit(&names));
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(Signal::Quit));
        assert!(request_quit(&names));
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(Signal::Quit));
    }

    #[test]
    fn quit_with_nothing_running_says_so() {
        assert!(!request_quit(&names()));
        assert!(!request_quit(&Names::smoke()));
    }

    #[test]
    fn a_signal_sent_before_listening_is_kept() {
        let names = names();
        let Claim::First(mut me) = claim(&names).unwrap() else {
            panic!()
        };
        assert!(matches!(claim(&names).unwrap(), Claim::AlreadyRunning));
        let (tx, rx) = mpsc::channel();
        me.listen(move |s| {
            let _ = tx.send(s);
        });
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(Signal::Show));
    }

    #[test]
    fn closing_frees_the_name() {
        let names = names();
        let (me, _rx) = first(&names);
        drop(me);
        assert!(matches!(claim(&names).unwrap(), Claim::First(_)));
    }

    fn paths(list: &[&str]) -> Vec<PathBuf> {
        list.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn add_reaches_the_running_copy_without_raising_it() {
        let names = names();
        let (_me, rx) = first(&names);
        assert!(matches!(
            claim_quietly(&names).unwrap(),
            Claim::AlreadyRunning
        ));
        let sent = paths(&[r"C:\Music\a.mp3", r"D:\Albums\Ágætis"]);
        assert!(send_add(&names, &sent, Duration::from_secs(5)));
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)),
            Ok(Signal::Add(sent))
        );
        // claim_quietly didn't ask for the window: no Show came, before or after
        assert_eq!(
            rx.recv_timeout(Duration::from_millis(300)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );
    }

    #[test]
    fn paths_added_before_listening_are_kept() {
        let names = names();
        let Claim::First(mut me) = claim(&names).unwrap() else {
            panic!()
        };
        assert!(me.add_error().is_none());
        assert!(send_add(
            &names,
            &paths(&[r"C:\one.mp3"]),
            Duration::from_secs(5)
        ));
        assert!(send_add(
            &names,
            &paths(&[r"C:\two.mp3"]),
            Duration::from_secs(5)
        ));
        let (tx, rx) = mpsc::channel();
        me.listen(move |s| {
            let _ = tx.send(s);
        });
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)),
            Ok(Signal::Add(paths(&[r"C:\one.mp3"])))
        );
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)),
            Ok(Signal::Add(paths(&[r"C:\two.mp3"])))
        );
    }

    #[test]
    fn a_long_add_arrives_whole_and_in_order() {
        let names = names();
        let (_me, rx) = first(&names);
        let sent: Vec<PathBuf> = (0..3000)
            .map(|i| PathBuf::from(format!(r"C:\{}\{i}.flac", "d".repeat(200))))
            .collect();
        assert!(send_add(&names, &sent, Duration::from_secs(5)));
        let mut got = vec![];
        let mut parts = 0;
        while got.len() < sent.len() {
            match rx.recv_timeout(Duration::from_secs(5)) {
                Ok(Signal::Add(p)) => got.extend(p),
                other => panic!("{other:?}"),
            }
            parts += 1;
        }
        assert!(parts >= 2);
        assert_eq!(got, sent);
    }

    #[test]
    fn add_with_nothing_running_says_so() {
        let t = std::time::Instant::now();
        assert!(!send_add(
            &names(),
            &paths(&[r"C:\a.mp3"]),
            Duration::from_millis(200)
        ));
        assert!(t.elapsed() < Duration::from_secs(3));
        assert!(!send_add(
            &Names::smoke(),
            &paths(&[r"C:\a.mp3"]),
            Duration::ZERO
        ));
    }

    #[test]
    fn a_self_test_copy_has_no_add_window() {
        // the smoke names: nothing outside can reach a test copy, and it never reaches the person's String
        let Claim::First(me) = claim(&Names::smoke()).unwrap() else {
            panic!()
        };
        assert!(me.add.is_none() && me.add_error().is_none());
    }

    #[test]
    fn a_stray_copydata_is_turned_down() {
        use windows::Win32::Foundation::{LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{
            FindWindowExW, HWND_MESSAGE, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_COPYDATA,
        };
        #[repr(C)]
        struct Cd {
            tag: usize,
            bytes: u32,
            data: *const u8,
        }
        let names = names();
        let (_me, rx) = first(&names);
        let class = HSTRING::from(names.add.as_deref().unwrap());
        let hwnd =
            unsafe { FindWindowExW(Some(HWND_MESSAGE), None, &class, PCWSTR::null()) }.unwrap();
        let text: Vec<u16> = "C:\\x.mp3\0".encode_utf16().collect();
        let cd = Cd {
            tag: 1,
            bytes: (text.len() * 2) as u32,
            data: text.as_ptr().cast(),
        };
        let mut answer = 7usize;
        let r = unsafe {
            SendMessageTimeoutW(
                hwnd,
                WM_COPYDATA,
                WPARAM(0),
                LPARAM(&cd as *const Cd as isize),
                SMTO_ABORTIFHUNG,
                5000,
                Some(&mut answer),
            )
        };
        assert!(r.0 != 0);
        assert_eq!(answer, 0);
        assert_eq!(
            rx.recv_timeout(Duration::from_millis(300)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );
    }

    #[test]
    fn closing_takes_the_add_window_with_it() {
        let names = names();
        let (me, _rx) = first(&names);
        drop(me);
        assert!(!send_add(&names, &paths(&[r"C:\a.mp3"]), Duration::ZERO));
        // and the next copy opens its own
        let (_again, rx) = first(&names);
        assert!(send_add(
            &names,
            &paths(&[r"C:\b.mp3"]),
            Duration::from_secs(5)
        ));
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)),
            Ok(Signal::Add(paths(&[r"C:\b.mp3"])))
        );
    }
}
