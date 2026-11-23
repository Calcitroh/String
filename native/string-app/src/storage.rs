use crate::playback::Session;
use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use egui::Context;
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
pub struct Receipt {
    pub errors: Vec<String>,
    pending: bool,
    state: bool,
    listening: bool,
    library: bool,
}
impl Receipt {
    pub fn apply(&self, s: &mut Session) {
        s.state_dirty |= self.state;
        s.listening_dirty |= self.listening;
        s.library_dirty |= self.library;
    }
    pub fn failed(&self) -> bool {
        self.pending || self.state || self.listening || self.library
    }
}
fn write<T: Model>(src: Option<T>, doc: &mut Document<T>, errors: &mut Vec<String>) -> bool {
    let Some(src) = src else { return false };
    doc.value = src;
    match doc.save() {
        Ok(r) => {
            if let Some(e) = r.backup_error {
                errors.push(format!("Backup of {}: {e}", T::NAME));
            }
            false
        }
        Err(e) => {
            errors.push(format!("Saving {}: {e}", T::NAME));
            true
        }
    }
}
type Job = Box<dyn FnOnce(&mut Profile) + Send>;
pub struct Storage {
    tx: Sender<Job>,
    pending: Option<Receiver<Receipt>>,
}
impl Storage {
    pub fn open(folder: &DataFolder, ctx: &Context) -> io::Result<(Self, Session)> {
        let mut profile: Profile = folder.open()?;
        // Before the first save, and only once the files have opened (so a damaged file a retry later fixes isn't
        // what's kept): today's first opening keeps a copy of them as another version of String left them. Without
        // that copy nothing is saved; the page offers Try again.
        folder.keep_opening_copy().map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("String couldn't keep a copy of your library before opening it: {e}"),
            )
        })?;
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
        Ok((Self { tx, pending: None }, s))
    }
    pub fn poll(&mut self, s: &mut Session) -> Option<Receipt> {
        let r = match self.pending.as_ref()?.try_recv() {
            Ok(r) => r,
            Err(crossbeam_channel::TryRecvError::Empty) => return None,
            Err(_) => broken(),
        };
        self.pending = None;
        r.apply(s);
        Some(r)
    }
    fn send(&self, s: &mut Session) -> Receiver<Receipt> {
        let (tx, rx) = bounded(1);
        let b = Batch::take(s);
        self.with_profile(move |profile| {
            let mut r = Receipt::default();
            r.state = write(b.state, &mut profile.state, &mut r.errors);
            r.listening = write(b.listening, &mut profile.listening, &mut r.errors);
            r.library = write(b.library, &mut profile.library, &mut r.errors);
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
        self.schedule(s);
        let mut last = self.wait_for_save(s, deadline);
        last.errors.splice(0..0, previous.errors);
        last
    }
    fn wait_for_save(&mut self, s: &mut Session, deadline: Instant) -> Receipt {
        let Some(rx) = &self.pending else {
            return Receipt::default();
        };
        let receipt = match rx.recv_deadline(deadline) {
            Ok(receipt) => receipt,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                return Receipt {
                    pending: true,
                    errors: vec!["The profile is still being written".into()],
                    ..Default::default()
                };
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => broken(),
        };
        self.pending = None;
        receipt.apply(s);
        receipt
    }
}
fn broken() -> Receipt {
    Receipt {
        errors: vec!["Profile writer stopped".into()],
        state: true,
        listening: true,
        library: true,
        pending: false,
    }
}
