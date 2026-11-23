use crate::{playback::Session, playlist_add, scan::Scanner};
use std::{
    collections::HashSet,
    fs, io,
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    thread,
};
use string_platform::instance::Instance;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Location {
    pub path: PathBuf,
    pub directory: bool,
}

pub(crate) fn classify(paths: Vec<PathBuf>) -> Vec<Location> {
    let mut seen = HashSet::new();
    paths
        .into_iter()
        .filter(|p| p.is_absolute() && seen.insert(p.to_string_lossy().to_lowercase()))
        .filter_map(|path| {
            let m = fs::metadata(&path).ok()?;
            (m.is_dir() || m.is_file()).then(|| Location {
                path,
                directory: m.is_dir(),
            })
        })
        .collect()
}

#[derive(Default)]
struct Pending {
    closed: bool,
    paths: usize,
    bytes: usize,
    wake: Option<egui::Context>,
}

// The charge follows a request through the worker and its result queue until the session applies it.
struct Charge {
    state: Arc<Mutex<Pending>>,
    paths: usize,
    bytes: usize,
}
impl Drop for Charge {
    fn drop(&mut self) {
        let mut p = self.state.lock().unwrap();
        p.paths -= self.paths;
        p.bytes -= self.bytes;
    }
}

enum Source {
    Helper(Option<String>),
    Window,
}
type Job = (Source, Vec<PathBuf>, Charge);

#[derive(Clone)]
pub(crate) struct Input {
    state: Arc<Mutex<Pending>>,
    jobs: mpsc::Sender<Job>,
}

impl Input {
    pub(crate) fn submit(&self, name: Option<String>, paths: Vec<PathBuf>, bytes: usize) -> bool {
        self.enqueue(Source::Helper(name), paths, bytes)
    }
    pub(crate) fn window_path(&self, path: PathBuf) -> bool {
        let Ok(path) = std::path::absolute(path) else {
            return false;
        };
        let bytes = (path.as_os_str().len() + 1) * 2;
        self.enqueue(Source::Window, vec![path], bytes)
    }
    fn enqueue(&self, source: Source, paths: Vec<PathBuf>, bytes: usize) -> bool {
        let mut p = self.state.lock().unwrap();
        if p.closed
            || paths.is_empty()
            || paths.len() > Instance::MAX_ADD_PATHS - p.paths
            || bytes > Instance::MAX_ADD_BYTES - p.bytes
        {
            return false;
        }
        p.paths += paths.len();
        p.bytes += bytes;
        let charge = Charge {
            state: self.state.clone(),
            paths: paths.len(),
            bytes,
        };
        // Serialize submissions to preserve admission order. Release the lock before dropping a failed send.
        let sent = self.jobs.send((source, paths, charge));
        drop(p);
        sent.is_ok()
    }
}

pub(crate) struct Batch {
    source: Source,
    locations: Vec<Location>,
    _charge: Option<Charge>,
}
impl Batch {
    pub fn startup(name: Option<String>, locations: Vec<Location>) -> Self {
        Self {
            source: Source::Helper(name),
            locations,
            _charge: None,
        }
    }
    /// Adds the checked paths; for drops, returns what to tell the person.
    pub fn apply(self, s: &mut Session, scan: &mut Scanner) -> Option<String> {
        if self.locations.is_empty() {
            return None;
        }
        if matches!(self.source, Source::Window) {
            // Drops follow the pickers' rules: a folder inside a listed one isn't added again, only songs are taken.
            let (folders, songs): (Vec<_>, Vec<_>) =
                self.locations.into_iter().partition(|l| l.directory);
            let folders: Vec<PathBuf> = folders.into_iter().map(|l| l.path).collect();
            let songs: Vec<PathBuf> = songs.into_iter().map(|l| l.path).collect();
            let a = crate::screens::settings::add_roots_to(s, scan, &folders);
            let b = crate::screens::settings::add_songs_to(s, scan, &songs);
            return match (a, b) {
                (Some(a), Some(b)) => Some(format!("{a} · {b}")),
                (a, b) => a.or(b),
            };
        }
        if let Source::Helper(Some(name)) = &self.source {
            playlist_add::reserve(s, name, &self.locations);
        }
        for (directory, key) in [(true, "roots"), (false, "files")] {
            let mut additions = self
                .locations
                .iter()
                .filter(|p| p.directory == directory)
                .peekable();
            if additions.peek().is_none() {
                continue;
            }
            let mut values = s.field(key).as_array().cloned().unwrap_or_default();
            let mut seen: HashSet<_> = values
                .iter()
                .filter_map(|v| v.as_str().map(str::to_lowercase))
                .collect();
            let count = values.len();
            for loc in additions {
                let path = loc.path.to_string_lossy();
                if seen.insert(path.to_lowercase()) {
                    values.push(path.into_owned().into());
                }
            }
            if values.len() != count {
                s.set_field(key, values.into());
            }
        }
        scan.requested = true;
        None
    }
}

/// Dropped paths checked and added at once, with the same rules as a drop through the worker (the self-test's way).
pub(crate) fn add_dropped_now(
    s: &mut Session,
    scan: &mut Scanner,
    paths: Vec<PathBuf>,
) -> Option<String> {
    Batch {
        source: Source::Window,
        locations: classify(paths),
        _charge: None,
    }
    .apply(s, scan)
}

pub(crate) struct Adds {
    pub input: Input,
    ready: mpsc::Receiver<Batch>,
    pub startup: Option<Batch>,
}

impl Adds {
    pub fn new() -> io::Result<Self> {
        Self::with_classifier(classify)
    }
    fn with_classifier(
        inspect: impl Fn(Vec<PathBuf>) -> Vec<Location> + Send + 'static,
    ) -> io::Result<Self> {
        let (jobs, inbox) = mpsc::channel::<Job>();
        let (finished, ready) = mpsc::channel();
        thread::Builder::new()
            .name("string-add-paths".into())
            .spawn(move || {
                for (source, paths, charge) in inbox {
                    let locations = inspect(paths);
                    let state = charge.state.clone();
                    if finished
                        .send(Batch {
                            source,
                            locations,
                            _charge: Some(charge),
                        })
                        .is_err()
                    {
                        break;
                    }
                    if let Some(ctx) = state.lock().unwrap().wake.clone() {
                        ctx.request_repaint();
                    }
                }
            })?;
        Ok(Self {
            input: Input {
                state: Default::default(),
                jobs,
            },
            ready,
            startup: None,
        })
    }
    pub fn attach(&self, ctx: &egui::Context) {
        self.input.state.lock().unwrap().wake = Some(ctx.clone());
    }
    pub fn set_closed(&self, closed: bool) {
        self.input.state.lock().unwrap().closed = closed;
    }
    pub fn close_if_idle(&self) -> bool {
        let mut p = self.input.state.lock().unwrap();
        if p.paths != 0 {
            return false;
        }
        p.closed = true;
        true
    }
    pub fn pending(&self) -> bool {
        self.startup.is_some() || self.input.state.lock().unwrap().paths != 0
    }
    pub fn poll(&mut self, s: &mut Session, scan: &mut Scanner, toast: &mut crate::ui::Toast) {
        // Everything dropped that's ready now goes in together, so several songs get one message.
        let mut dropped = Vec::new();
        let mut charges = Vec::new();
        let mut missing = false;
        for batch in self.startup.take().into_iter().chain(self.ready.try_iter()) {
            if matches!(batch.source, Source::Window) {
                missing |= batch.locations.is_empty();
                dropped.extend(batch.locations);
                charges.push(batch._charge);
            } else {
                batch.apply(s, scan);
            }
        }
        if dropped.is_empty() {
            if missing {
                toast.set("This path does not exist.");
            }
            return;
        }
        let message = Batch {
            source: Source::Window,
            locations: dropped,
            _charge: None,
        }
        .apply(s, scan);
        // Their share of the pending limit is released only once they're in the library.
        drop(charges);
        if let Some(message) = message {
            toast.set(message);
        }
    }
    // Normal closes poll until ready. A forced event-loop exit also waits for accepted work, outside the session lock.
    pub fn next_at_exit(&mut self) -> Option<Batch> {
        self.startup
            .take()
            .or_else(|| self.pending().then(|| self.ready.recv().ok()).flatten())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn capacity_covers_queued_work_the_classifier_and_unapplied_results() {
        let (started, working) = mpsc::channel();
        let (resume, gate) = mpsc::channel();
        let adds = Adds::with_classifier(move |_| {
            started.send(thread::current().id()).unwrap();
            gate.recv().unwrap();
            Vec::new()
        })
        .unwrap();
        let path = || vec![PathBuf::from(r"C:\Music\example.wav")];
        assert!(adds.input.submit(None, path(), Instance::MAX_ADD_BYTES - 2));
        assert_ne!(
            working.recv_timeout(Duration::from_secs(2)).unwrap(),
            thread::current().id()
        );
        assert!(adds.input.submit(Some("List".into()), path(), 2));
        assert!(!adds.input.submit(None, path(), 2));
        resume.send(()).unwrap();
        let batch = adds.ready.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(!adds.input.submit(None, path(), 2));
        drop(batch);
        resume.send(()).unwrap();
        drop(adds.ready.recv_timeout(Duration::from_secs(2)).unwrap());
        assert!(
            adds.input
                .submit(None, vec![PathBuf::new(); Instance::MAX_ADD_PATHS], 2)
        );
        assert!(!adds.input.submit(None, path(), 2));
        adds.set_closed(true);
        resume.send(()).unwrap();
        drop(adds.ready.recv_timeout(Duration::from_secs(2)).unwrap());
        assert!(!adds.input.submit(None, path(), 2));
        adds.set_closed(false);
        assert!(adds.input.submit(None, path(), 2));
        resume.send(()).unwrap();
        drop(adds.ready.recv_timeout(Duration::from_secs(2)).unwrap());
    }
}
