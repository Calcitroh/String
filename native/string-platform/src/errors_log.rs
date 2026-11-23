//! `errors.log` in String's data folder (App.cs's LogError): unexpected errors and failed saves, newest last, each
//! `yyyy-MM-dd HH:mm:ss <where>: <what>` and a blank line. Past 512 KB it starts again, the old one kept as
//! `errors-previous.log`. Writing it never fails loudly: a log that can't be written is skipped.

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use windows::Win32::System::SystemInformation::GetLocalTime;

/// One per data folder, shared (`Arc`) by everything that logs, so writes never interleave.
pub struct ErrorLog {
    path: PathBuf,
    previous: PathBuf,
    lock: Mutex<()>,
}

impl ErrorLog {
    /// Bigger than this, the log starts again.
    pub const LIMIT: u64 = 512 * 1024;

    pub fn new(data_dir: &Path) -> ErrorLog {
        ErrorLog {
            path: data_dir.join("errors.log"),
            previous: data_dir.join("errors-previous.log"),
            lock: Mutex::new(()),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// `place`: where it happened ("save state", "ui", "crash"); `what`: the error.
    pub fn write(&self, place: &str, what: impl fmt::Display) {
        let _one_at_a_time = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        if fs::metadata(&self.path).is_ok_and(|m| m.len() > Self::LIMIT) {
            let _ = fs::rename(&self.path, &self.previous);
        }
        let t = unsafe { GetLocalTime() };
        let entry = format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02} {place}: {what}\r\n\r\n",
            t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
        );
        if let Ok(mut f) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            let _ = f.write_all(entry.as_bytes());
        }
    }
}

/// Panics go into the log too (as "crash", like App.cs's unhandled exceptions), then on to whatever reported them before.
pub fn log_panics(log: Arc<ErrorLog>) {
    let before = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log.write("crash", info);
        before(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(text: &str) -> Vec<&str> {
        text.split("\r\n\r\n").filter(|e| !e.is_empty()).collect()
    }

    fn dated(entry: &str) -> bool {
        let b = entry.as_bytes();
        entry.len() > 20
            && [4, 7].iter().all(|&i| b[i] == b'-')
            && b[10] == b' '
            && [13, 16].iter().all(|&i| b[i] == b':')
            && b[19] == b' '
            && b[..19]
                .iter()
                .enumerate()
                .all(|(i, c)| [4, 7, 10, 13, 16].contains(&i) || c.is_ascii_digit())
    }

    #[test]
    fn entries_as_app_cs_wrote_them() {
        let dir = tempfile::tempdir().unwrap();
        let log = ErrorLog::new(dir.path());
        log.write("save state", "The disk is full");
        log.write("ui", std::io::Error::other("broken"));
        let text = fs::read_to_string(log.path()).unwrap();
        assert!(text.ends_with("\r\n\r\n"));
        let e = entries(&text);
        assert_eq!(e.len(), 2);
        assert!(e.iter().all(|e| dated(e)), "{e:?}");
        assert_eq!(&e[0][20..], "save state: The disk is full");
        assert_eq!(&e[1][20..], "ui: broken");
    }

    #[test]
    fn starts_again_past_512_kb() {
        let dir = tempfile::tempdir().unwrap();
        let log = ErrorLog::new(dir.path());
        fs::write(log.path(), vec![b'x'; ErrorLog::LIMIT as usize]).unwrap();
        log.write("a", "at the limit: kept");
        assert!(!dir.path().join("errors-previous.log").exists());
        log.write("b", "over it: a new log");
        let previous = fs::read_to_string(dir.path().join("errors-previous.log")).unwrap();
        assert!(previous.starts_with("xxx") && previous.contains("a: at the limit: kept"));
        let now = fs::read_to_string(log.path()).unwrap();
        assert_eq!(entries(&now).len(), 1);
        assert!(now.contains("b: over it: a new log"));
    }

    #[test]
    fn a_log_that_cant_be_written_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let log = ErrorLog::new(&dir.path().join("no such folder"));
        log.write("save", "nowhere to go");
        assert!(!log.path().exists());
    }
}
