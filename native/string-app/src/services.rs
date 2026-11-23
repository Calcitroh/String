//! The calls Settings, Duplicates and Edit info make outside their own screens: pickers, Explorer, the Recycle
//! Bin, translation models, updates and Discord. The app installs one adapter with [`install`];
//! until then every call answers "not connected" and the screens say so.
use crossbeam_channel::{Receiver, bounded};
use egui::Context;
use std::{
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
    thread,
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UpdateInfo {
    /// This copy's version.
    pub current: String,
    /// Installed copies update themselves. A copy built from source does not.
    pub installed: bool,
    /// There is a place to get releases from.
    pub published: bool,
    /// The newest release's version, once a check has run.
    pub latest: String,
    pub notes: String,
    pub newer: bool,
}
#[derive(Clone, Copy, Debug)]
pub enum UpdateProgress {
    Receiving { got: u64, total: u64 },
    Verifying,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DiscordStatus {
    #[default]
    Off,
    NoId,
    BadId,
    Starting,
    Waiting,
    Connected,
    Refused,
    Stopped,
    Error,
}
#[derive(Clone, Debug, Default)]
pub struct Discord {
    pub status: DiscordStatus,
    /// The account Discord answered with.
    pub user: String,
    /// The song title on the profile right now, or empty while paused.
    pub showing: String,
    pub error: String,
}
/// What a screen just changed, for the parts of the app that follow it.
#[derive(Clone, Copy, Debug)]
pub enum Event<'a> {
    /// A key of `set` in state.json changed (`discord`, `discordId`, `dcShow`, `dcCovers`, `phoneStats`,
    /// `lyricsOnline`, `trLang`, `trModel`, `updates`).
    Setting(&'a str),
    /// A song's names or cover changed in Edit info.
    Edited(&'a str),
    /// A duplicate was dropped for the copy kept.
    Merged { from: &'a str, to: &'a str },
    /// Song files moved to the Recycle Bin.
    Removed(&'a [String]),
}
fn unwired<T>() -> io::Result<T> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "This isn't connected in this build yet",
    ))
}
/// Every method except `pickers`, `update_info`, `discord`, `edit_note` and `event` may block: the screens call
/// those on a worker thread through [`Task`].
pub trait Services: Send + Sync {
    /// Folder and file pickers work. Without them Settings offers a box to type a path into.
    fn pickers(&self) -> bool {
        false
    }
    /// An empty list when the person cancelled.
    fn pick_folders(&self, _title: &str) -> io::Result<Vec<PathBuf>> {
        unwired()
    }
    /// `filter` reads "Songs|*.mp3;*.flac|All files|*.*".
    fn pick_files(
        &self,
        _title: &str,
        _filter: &str,
        _start: Option<&Path>,
        _multi: bool,
    ) -> io::Result<Vec<PathBuf>> {
        unwired()
    }
    /// Show a file or folder in Explorer.
    fn reveal(&self, _path: &Path) -> io::Result<()> {
        unwired()
    }
    fn open_link(&self, _url: &str) -> io::Result<()> {
        unwired()
    }
    /// Move one song file to the Recycle Bin.
    fn recycle(&self, _path: &Path) -> io::Result<()> {
        unwired()
    }
    /// The translation models downloaded on this PC.
    fn translation_models(&self) -> io::Result<Vec<String>> {
        unwired()
    }
    /// What is known without asking anyone.
    fn update_info(&self) -> UpdateInfo {
        UpdateInfo {
            current: env!("CARGO_PKG_VERSION").into(),
            ..Default::default()
        }
    }
    /// Ask for the newest release.
    fn update_check(&self) -> io::Result<UpdateInfo> {
        unwired()
    }
    /// Download, check and start the installer. True when the installer started.
    fn update_install(
        &self,
        _version: &str,
        _progress: &mut dyn FnMut(UpdateProgress),
    ) -> io::Result<bool> {
        unwired()
    }
    /// The Discord helper's state. `None` leaves Settings to tell it from the switch and the ID alone.
    fn discord(&self) -> Option<Discord> {
        None
    }
    /// A sentence for Edit info about this song, from an add-on.
    fn edit_note(&self, _id: &str) -> Option<String> {
        None
    }
    fn event(&self, _event: Event<'_>) {}
}
struct Unwired;
impl Services for Unwired {}
static CURRENT: RwLock<Option<Arc<dyn Services>>> = RwLock::new(None);
pub fn install(services: Arc<dyn Services>) {
    *CURRENT.write().unwrap() = Some(services);
}
pub fn get() -> Arc<dyn Services> {
    CURRENT
        .read()
        .unwrap()
        .clone()
        .unwrap_or_else(|| Arc::new(Unwired))
}
/// One call on a worker thread. The window repaints when the answer is in.
pub struct Task<T>(Receiver<T>);
impl<T: Send + 'static> Task<T> {
    pub fn run(ctx: &Context, call: impl FnOnce(&dyn Services) -> T + Send + 'static) -> Self {
        let (tx, rx) = bounded(1);
        let ctx = ctx.clone();
        thread::spawn(move || {
            let _ = tx.send(call(&*get()));
            ctx.request_repaint();
        });
        Self(rx)
    }
    pub fn poll(&self) -> Option<T> {
        self.0.try_recv().ok()
    }
}
static NOTICES: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// A sentence for the toast, from any thread.
pub fn notice(message: impl Into<String>) {
    NOTICES.lock().unwrap().push(message.into());
}
pub fn take_notices() -> Vec<String> {
    std::mem::take(&mut NOTICES.lock().unwrap())
}
/// A call whose only answer is an error to show.
pub fn fire(ctx: &Context, call: impl FnOnce(&dyn Services) -> io::Result<()> + Send + 'static) {
    let ctx = ctx.clone();
    thread::spawn(move || {
        if let Err(e) = call(&*get()) {
            notice(e.to_string());
            ctx.request_repaint();
        }
    });
}
