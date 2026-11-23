use crate::{Error, source::Source};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};

type Opened = Result<Source, Error>;

struct Job {
    path: PathBuf,
    reply: mpsc::Sender<Opened>,
}

/// One open in progress and at most one queued replacement. Each request owns its
/// reply channel. Replacing a request drops the old receiver even if I/O is stuck.
pub(super) struct Preload {
    queued: Arc<Mutex<Option<Job>>>,
    wake: mpsc::SyncSender<()>,
    pending: Option<(PathBuf, mpsc::Receiver<Opened>)>,
    ready: Option<Source>,
    /// The offline renderer advances a virtual clock, independently of wall time.
    pub wait: bool,
}

impl Preload {
    pub fn new(open: impl Fn(&Path) -> Opened + Send + 'static) -> Self {
        let (wake, signal) = mpsc::sync_channel(1);
        let queued = Arc::new(Mutex::new(None::<Job>));
        let jobs = queued.clone();
        let _ = std::thread::Builder::new()
            .name("audio preload".into())
            .spawn(move || {
                while signal.recv().is_ok() {
                    // Release the lock before touching the file. Queue edits never wait for I/O.
                    let job = jobs.lock().unwrap().take();
                    if let Some(job) = job {
                        let _ = job.reply.send(open(&job.path));
                    }
                }
            });
        Self {
            queued,
            wake,
            pending: None,
            ready: None,
            wait: false,
        }
    }

    pub fn is_for(&self, path: &Path) -> bool {
        self.pending.as_ref().is_some_and(|(p, _)| p == path)
    }

    pub fn request(&mut self, path: &Path) {
        if !self.is_for(path) {
            let (reply, rx) = mpsc::channel();
            self.ready = None;
            self.pending = Some((path.to_owned(), rx));
            *self.queued.lock().unwrap() = Some(Job {
                path: path.to_owned(),
                reply,
            });
            if let Err(mpsc::TrySendError::Disconnected(())) = self.wake.try_send(()) {
                self.queued.lock().unwrap().take();
            }
        }
    }

    pub fn poll(&mut self) -> Option<Opened> {
        if let Some(source) = self.ready.take() {
            return Some(Ok(source));
        }
        let (_, rx) = self.pending.as_ref()?;
        let result = if self.wait {
            rx.recv().ok()
        } else {
            match rx.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Empty) => return None,
                Err(mpsc::TryRecvError::Disconnected) => None,
            }
        };
        Some(result.unwrap_or_else(|| Err(Error::Open("source worker unavailable".into()))))
    }

    pub fn keep(&mut self, source: Source) {
        self.ready = Some(source);
    }

    pub fn clear(&mut self) {
        if self.pending.take().is_some() {
            self.ready = None;
            self.queued.lock().unwrap().take();
        }
    }
}
