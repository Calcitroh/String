use crate::{Error, source::Source};
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};

type Opened = Result<Source, Error>;
type Open = Arc<dyn Fn(&Path) -> Opened + Send + Sync>;

/// The next song's file opened off the engine thread. Each request opens on a thread of its own and owns its reply
/// channel: a replaced request is dropped at once, and a read stuck on a slow drive never holds up its replacement
/// (it finishes later into a channel nobody reads).
pub(super) struct Preload {
    open: Open,
    pending: Option<(PathBuf, mpsc::Receiver<Opened>)>,
    ready: Option<Source>,
    /// The offline renderer advances a virtual clock, independently of wall time.
    pub wait: bool,
}

impl Preload {
    pub fn new(open: impl Fn(&Path) -> Opened + Send + Sync + 'static) -> Self {
        Self {
            open: Arc::new(open),
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
            let (open, path) = (self.open.clone(), path.to_owned());
            // If no thread can be made, the reply is dropped with the closure and `poll` reports the open failed.
            let _ = std::thread::Builder::new()
                .name("audio preload".into())
                .spawn(move || {
                    let _ = reply.send(open(&path));
                });
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    /// The final review's case: B's open hangs on a slow share, the queue then wants C instead. C must arrive while
    /// B is still stuck, or Next (and the song's natural end) waits in silence.
    #[test]
    fn a_replacement_open_never_waits_behind_a_stuck_one() {
        let (release, gate) = mpsc::channel::<()>();
        let gate = Mutex::new(gate);
        let mut preload = Preload::new(move |path: &Path| {
            if path == Path::new("slow-share.mp3") {
                let _ = gate.lock().unwrap().recv_timeout(Duration::from_secs(10));
            }
            Err(Error::Open(path.display().to_string()))
        });
        preload.request(Path::new("slow-share.mp3"));
        std::thread::sleep(Duration::from_millis(50));
        preload.request(Path::new("local.mp3"));
        let end = Instant::now() + Duration::from_secs(3);
        let got = loop {
            if let Some(result) = preload.poll() {
                break result;
            }
            assert!(Instant::now() < end, "the replacement waited behind the stuck open");
            std::thread::sleep(Duration::from_millis(5));
        };
        assert!(matches!(got, Err(Error::Open(p)) if p == "local.mp3"));
        drop(release);
    }
}
