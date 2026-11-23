use crate::playback::Session;
use crossbeam_channel::{Receiver, bounded};
use egui::Context;
use std::{path::PathBuf, thread, time::Instant};
use string_core::library::{Library, ScanReport};
struct Job {
    rx: Receiver<(Library, ScanReport)>,
    inputs: [Vec<PathBuf>; 2],
    /// `Session::removals` when the scan started
    removals: u64,
    start: Instant,
}
#[derive(Default)]
pub struct Scanner {
    job: Option<Job>,
    pub requested: bool,
    pub completed: usize,
}
impl Scanner {
    pub fn running(&self) -> bool {
        self.job.is_some()
    }
    pub fn status(&self) -> Option<String> {
        let j = self.job.as_ref()?;
        Some(format!(
            "Scanning {} locations · {}s",
            j.inputs[0].len() + j.inputs[1].len(),
            j.start.elapsed().as_secs()
        ))
    }
    fn inputs(s: &Session) -> [Vec<PathBuf>; 2] {
        ["roots", "files"].map(|key| {
            s.field(key)
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str().map(PathBuf::from))
                .collect()
        })
    }
    pub fn poll(&mut self, s: &mut Session, ctx: &Context) -> Vec<String> {
        let mut errors = vec![];
        if let Some(j) = &self.job {
            match j.rx.try_recv() {
                Ok((lib, report)) => {
                    // a song moved to the Recycle Bin since this scan began would come back with its result
                    if j.inputs == Self::inputs(s) && j.removals == s.removals {
                        if report.changed() {
                            s.library = lib;
                            s.library_dirty = true;
                            s.rebuild();
                        }
                        errors.extend(
                            report
                                .issues
                                .into_iter()
                                .map(|e| format!("Scanning {}: {}", e.path.display(), e.error)),
                        );
                    } else {
                        self.requested = true;
                    }
                    self.job = None;
                    self.completed += 1;
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    self.job = None;
                    errors.push("Library scan stopped unexpectedly.".into())
                }
                _ => {}
            }
        }
        if self.requested && self.job.is_none() {
            self.requested = false;
            let inputs = Self::inputs(s);
            let [roots, files] = inputs.clone();
            let (tx, rx) = bounded(1);
            let mut lib = s.library.clone();
            let ctx = ctx.clone();
            self.job = Some(Job {
                rx,
                inputs,
                removals: s.removals,
                start: Instant::now(),
            });
            thread::spawn(move || {
                let rep = lib.scan(&roots, &files);
                let _ = tx.send((lib, rep));
                ctx.request_repaint();
            });
        }
        errors
    }
}
