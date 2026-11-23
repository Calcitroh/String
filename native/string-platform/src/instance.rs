//! One copy of String at a time, compatible with String 1.x: the same mutex (`StringApp.SingleInstance`) and events
//! (`StringApp.Show`, `StringApp.Quit`), so a second launch of either version brings the running one forward, and
//! `--quit` (the 1.x updater's step) asks either to save and close.
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
//! });
//! ```

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
        }
    }

    /// `<prefix>.SingleInstance`, `<prefix>.Show` and `<prefix>.Quit`.
    pub fn custom(prefix: &str) -> Names {
        Names {
            mutex: format!("{prefix}.SingleInstance"),
            show: Some(format!("{prefix}.Show")),
            quit: Some(format!("{prefix}.Quit")),
        }
    }
}

/// What another launch asked of the running copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signal {
    /// A second launch: bring the window forward (restore it if minimised).
    Show,
    /// `--quit`: save and close. Every request counts: one turned down doesn't stop the next being heard.
    Quit,
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
    watcher: Option<thread::JoinHandle<()>>,
}

/// Takes the mutex, or (another copy has it) asks that copy to come forward.
pub fn claim(names: &Names) -> io::Result<Claim> {
    let mutex = unsafe { CreateMutexW(None, true, &HSTRING::from(names.mutex.as_str())) }?;
    let existed = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    let mutex = Owned(mutex);
    let show = names.show.as_deref().map(event).transpose()?;
    if existed {
        if let Some(show) = &show {
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
    Ok(Claim::First(Instance {
        _mutex: mutex,
        show,
        quit,
        stop,
        watcher: None,
    }))
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
    /// Hears second launches and `--quit` on a thread of its own and calls `on_signal` there (forward it to the window's
    /// thread). Once; later calls do nothing. Stops when the instance is dropped.
    pub fn listen(&mut self, mut on_signal: impl FnMut(Signal) + Send + 'static) {
        if self.watcher.is_some() {
            return;
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
                        Some(Some(kind)) => on_signal(*kind),
                        _ => break, // the stop event, or a failed wait
                    }
                }
            });
        self.watcher = spawned.ok();
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
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
    // Never String's own names: a test that set StringApp.Show or StringApp.Quit would raise or close the person's String.
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
    }

    #[test]
    fn smoke_names_are_this_process_own() {
        let n = Names::smoke();
        assert_eq!(n.mutex, format!("StringApp.Smoke.{}", std::process::id()));
        assert_eq!((n.show, n.quit), (None, None));
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
}
