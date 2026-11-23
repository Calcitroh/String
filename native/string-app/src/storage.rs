mod feedback;
mod timing;
use crate::playback::Session;
use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use egui::Context;
use feedback::Feedback;
use std::{
    io,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};
use string_core::{
    data::{DataFolder, Document, Model, Profile},
    library::Library,
    state::State,
    stats::Listening,
};
use timing::{EDIT, LISTENING, POSITION, Timing};
pub fn folder(root: &Path, explicit: bool) -> DataFolder {
    let backup = if explicit {
        root.join("backup")
    } else {
        documents()
            .unwrap_or_else(|| root.to_owned())
            .join("String Backup")
    };
    DataFolder::new(root, backup)
}
fn documents() -> Option<PathBuf> {
    use windows::Win32::{
        System::Com::CoTaskMemFree,
        UI::Shell::{FOLDERID_Documents, KF_FLAG_DEFAULT, SHGetKnownFolderPath},
    };
    unsafe {
        let raw = SHGetKnownFolderPath(&FOLDERID_Documents, KF_FLAG_DEFAULT, None).ok()?;
        let path = raw.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(raw.0.cast()));
        path
    }
}
#[derive(Default)]
struct Batch {
    state: Option<State>,
    listening: Option<Listening>,
    library: Option<Library>,
}
impl Batch {
    fn take(s: &mut Session) -> Self {
        Self {
            state: std::mem::take(&mut s.state_dirty).then(|| s.state.clone()),
            listening: std::mem::take(&mut s.listening_dirty).then(|| s.listening.clone()),
            library: std::mem::take(&mut s.library_dirty).then(|| s.library.clone()),
        }
    }
}
#[derive(Default)]
enum Write {
    #[default]
    Skipped,
    Saved,
    Failed(String),
}
#[derive(Default)]
pub struct Receipt {
    // State, listening and library retain independent save and retry deadlines.
    files: [Write; 3],
    backup_errors: Vec<String>,
    pending: bool,
}
impl Receipt {
    pub fn failed(&self) -> bool {
        self.pending
            || self
                .files
                .iter()
                .any(|file| matches!(file, Write::Failed(_)))
    }
}
fn write<T: Model>(src: Option<T>, doc: &mut Document<T>, backups: &mut Vec<String>) -> Write {
    let Some(src) = src else {
        return Write::Skipped;
    };
    doc.value = src;
    match doc.save() {
        Ok(r) => {
            if let Some(e) = r.backup_error {
                backups.push(format!("Backup of {}: {e}", T::NAME));
            }
            Write::Saved
        }
        Err(e) => Write::Failed(format!("Saving {}: {e}", T::NAME)),
    }
}
type Job = Box<dyn FnOnce(&mut Profile) + Send>;
pub struct Storage {
    tx: Sender<Job>,
    pending: Option<Receiver<Receipt>>,
    timing: Timing,
    failures: [Option<String>; 3],
    last_transport: (Option<u64>, bool),
    pub(crate) feedback: Feedback,
}
impl Storage {
    pub fn open(folder: &DataFolder, ctx: &Context) -> io::Result<(Self, Session)> {
        let mut profile: Profile = folder.open()?;
        // Copy the original bytes after a good load and before handing any caller a writer.
        let mut feedback = Feedback::default();
        if let Err(e) = folder.keep_opening_copy() {
            feedback.backup(format!(
                "String couldn't keep a copy of your library before opening it: {e}"
            ));
        }
        let s = Session::new(
            profile.state.value.clone(),
            profile.listening.value.clone(),
            profile.library.value.clone(),
        );
        // The worker owns the original document fingerprints for the entire open session.
        let (tx, rx) = unbounded::<Job>();
        let ctx = ctx.clone();
        thread::Builder::new()
            .name("profile writer".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    job(&mut profile);
                    ctx.request_repaint();
                }
            })?;
        Ok((
            Self {
                tx,
                pending: None,
                timing: Timing::default(),
                failures: Default::default(),
                last_transport: (None, false),
                feedback,
            },
            s,
        ))
    }
    pub fn poll(&mut self, s: &mut Session) -> Option<Receipt> {
        let r = match self.pending.as_ref()?.try_recv() {
            Ok(r) => r,
            Err(crossbeam_channel::TryRecvError::Empty) => return None,
            Err(_) => broken(),
        };
        self.pending = None;
        self.received(s, &r);
        Some(r)
    }
    fn received(&mut self, s: &mut Session, receipt: &Receipt) {
        for msg in &receipt.backup_errors {
            self.feedback.backup(msg.clone());
        }
        for (file, (result, dirty)) in receipt.files.iter().zip(Self::dirty(s)).enumerate() {
            match result {
                Write::Skipped => {}
                Write::Saved => self.failures[file] = None,
                Write::Failed(reason) => {
                    *dirty = true;
                    self.failures[file] = Some(reason.clone());
                    self.timing.failed(file, Instant::now());
                }
            }
        }
        let errors: Vec<_> = self.failures.iter().flatten().cloned().collect();
        self.feedback.saved(&errors);
    }

    fn dirty(s: &mut Session) -> [&mut bool; 3] {
        [
            &mut s.state_dirty,
            &mut s.listening_dirty,
            &mut s.library_dirty,
        ]
    }
    fn remember(&mut self, s: &mut Session, now: Instant, delays: [Duration; 3]) {
        for (file, (dirty, delay)) in Self::dirty(s).into_iter().zip(delays).enumerate() {
            if std::mem::take(dirty) {
                self.timing.mark(file, now, delay);
            }
        }
    }
    pub fn tick(&mut self, s: &mut Session, st: string_audio::Status, now: Instant) {
        // UI, queue and scan edits already waiting retain the short save delay during playback.
        self.remember(s, now, [EDIT; 3]);
        let transport = (st.key, st.playing);
        let changed = self.last_transport != transport;
        self.last_transport = transport;
        let plays = s.plays_revision;
        s.tick(st, false);
        let position_delay = if changed || plays != s.plays_revision {
            EDIT
        } else {
            POSITION
        };
        if changed && self.timing.due[0].is_some() {
            self.timing.mark(0, now, EDIT);
        }
        self.remember(s, now, [position_delay, LISTENING, EDIT]);
    }
    pub fn autosave(&mut self, s: &mut Session, now: Instant) -> Option<Duration> {
        self.remember(s, now, [EDIT; 3]);
        if self.pending.is_some() {
            return None;
        } // The writer wakes the window on completion.
        let ready = self.timing.take(now);
        for (dirty, ready) in Self::dirty(s).into_iter().zip(ready) {
            *dirty |= ready;
        }
        self.schedule(s);
        self.timing.remaining(now)
    }
    fn send(&self, s: &mut Session) -> Receiver<Receipt> {
        let (tx, rx) = bounded(1);
        let b = Batch::take(s);
        self.with_profile(move |profile| {
            let mut r = Receipt::default();
            r.files = [
                write(b.state, &mut profile.state, &mut r.backup_errors),
                write(b.listening, &mut profile.listening, &mut r.backup_errors),
                write(b.library, &mut profile.library, &mut r.backup_errors),
            ];
            let _ = tx.send(r);
        });
        rx
    }
    pub(crate) fn with_profile(&self, job: impl FnOnce(&mut Profile) + Send + 'static) {
        let _ = self.tx.send(Box::new(job));
    }
    pub fn schedule(&mut self, s: &mut Session) {
        if self.pending.is_none() && (s.state_dirty || s.listening_dirty || s.library_dirty) {
            self.pending = Some(self.send(s));
        }
    }
    pub fn flush(&mut self, s: &mut Session) -> Receipt {
        self.flush_until(s, Instant::now() + Duration::from_secs(10))
    }
    pub(crate) fn flush_until(&mut self, s: &mut Session, deadline: Instant) -> Receipt {
        let previous = self.wait_for_save(s, deadline);
        if previous.pending {
            return previous;
        }
        // Close bypasses every deadline and includes the latest position and listening ticks.
        for (dirty, due) in Self::dirty(s).into_iter().zip(self.timing.due.iter_mut()) {
            *dirty |= due.take().is_some();
        }
        self.timing = Timing::default();
        self.schedule(s);
        self.wait_for_save(s, deadline)
    }
    fn wait_for_save(&mut self, s: &mut Session, deadline: Instant) -> Receipt {
        let Some(rx) = &self.pending else {
            return Receipt::default();
        };
        let receipt = match rx.recv_deadline(deadline) {
            Ok(receipt) => receipt,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                self.feedback
                    .saved(&["The profile is still being written".into()]);
                return Receipt {
                    pending: true,
                    ..Default::default()
                };
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => broken(),
        };
        self.pending = None;
        self.received(s, &receipt);
        receipt
    }
}
fn broken() -> Receipt {
    Receipt {
        files: std::array::from_fn(|_| Write::Failed("Profile writer stopped".into())),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn retry_saves_the_latest_edit_after_the_file_becomes_writable() {
        let dir = tempfile::tempdir().unwrap();
        let folder = folder(dir.path(), true);
        let (mut store, mut session) = Storage::open(&folder, &Context::default()).unwrap();
        let lock = dir.path().join(".state.lock");
        fs::create_dir(&lock).unwrap();
        session.set_field("route", "songs".into());
        assert!(store.flush(&mut session).failed());
        let retry = store.timing.due[0].unwrap();
        session.set_field("route", "liked".into());
        fs::remove_dir(&lock).unwrap();
        store.autosave(&mut session, retry - Duration::from_millis(1));
        assert!(store.pending.is_none());
        store.autosave(&mut session, retry);
        assert!(store.pending.is_some());
        assert!(
            !store
                .wait_for_save(&mut session, Instant::now() + Duration::from_secs(10))
                .failed()
        );
        assert_eq!(folder.open().unwrap().state.value.fields["route"], "liked");
        assert_eq!(store.feedback.logs.len(), 1);
        assert_eq!(store.feedback.messages.len(), 2);
        assert_eq!(store.feedback.messages[1], "Your changes are saved again");
    }

    #[test]
    fn a_failed_state_write_does_not_delay_or_announce_recovery_for_a_healthy_cache_save() {
        let dir = tempfile::tempdir().unwrap();
        let folder = folder(dir.path(), true);
        let (mut store, mut session) = Storage::open(&folder, &Context::default()).unwrap();
        fs::create_dir(dir.path().join(".state.lock")).unwrap();
        session.set_field("route", "songs".into());
        assert!(store.flush(&mut session).failed());
        session.library_dirty = true;
        let now = Instant::now();
        store.autosave(&mut session, now);
        store.autosave(&mut session, now + EDIT);
        assert!(store.pending.is_some());
        store.wait_for_save(&mut session, now + Duration::from_secs(5));
        assert!(dir.path().join("library.json").exists());
        assert!(store.feedback.reason.is_some());
        assert_eq!(store.feedback.messages.len(), 1);
    }

    #[test]
    fn close_saves_both_pending_clocks_after_the_opening_copy() {
        let dir = tempfile::tempdir().unwrap();
        let folder = folder(dir.path(), true);
        let before = br#"{"v":1,"queue":{"pos":null},"route":"liked"}"#;
        fs::write(dir.path().join("state.json"), before).unwrap();
        let (mut store, mut session) = Storage::open(&folder, &Context::default()).unwrap();
        session.state.queue.seek(2.);
        session.state_dirty = true;
        session.listening_dirty = true;
        store.remember(&mut session, Instant::now(), [POSITION, LISTENING, EDIT]);
        assert!(!store.flush(&mut session).failed());
        assert_eq!(folder.open().unwrap().state.value.queue.pos(), 2.);
        assert!(dir.path().join("listening.json").is_file());
        let kept = dir
            .path()
            .join("backup/opened")
            .join(chrono::Local::now().date_naive().to_string());
        assert_eq!(fs::read(kept.join("state.json")).unwrap(), before);
    }
}
