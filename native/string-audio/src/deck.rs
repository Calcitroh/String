//! A deck's feeding end, on the engine thread: songs decoded and resampled to the output rate
//! into a lock-free ring the sound thread reads. Markers in a second ring say where things are:
//! where a stretch of a song begins (and at which position), where a song really ends, and
//! where the frames stop. Each load, seek or next song is a new era, so the sound thread can
//! tell fresh frames from stale ones without the feeder ever waiting on it.
//!
//! The next song at the same sample rate is chained on: it goes through the same resampler
//! straight after the last frame of the song before, so the joint is exactly what one long
//! recording would give (separately resampled songs would sit a fraction of a sample apart and
//! tick at the joint).

use std::collections::VecDeque;

use crate::Error;
use crate::resample::Resampler;
use crate::source::Source;

/// How far ahead of the sound a deck is decoded.
const AHEAD_SECS: f64 = 1.5;
/// Frames decoded per step (bounded so one deck never holds up the other for long).
const STEP: usize = 4096;
/// A song's length when nobody knows it.
pub(crate) const UNKNOWN: i64 = i64::MIN;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Marker {
    /// The frame (counted since the ring was made) where this takes effect.
    pub at: u64,
    pub era: u32,
    pub kind: MarkerKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MarkerKind {
    /// A stretch starts here: this frame sits at output position `pos` of song `handle`
    /// (negative inside a kept resampler lead-in), whose length is `end` (or UNKNOWN).
    Begin {
        pos: i64,
        handle: u32,
        key: u64,
        end: i64,
    },
    /// The song's last playable frame ends here.
    End,
    /// No frames follow in this era.
    Stop,
}

/// The two rings of one deck: samples (interleaved stereo) and markers.
pub(crate) fn rings(rate: u32) -> (Feeder, (rtrb::Consumer<f32>, rtrb::Consumer<Marker>)) {
    let frames = (rate as f64 * AHEAD_SECS) as usize;
    let (sp, sc) = rtrb::RingBuffer::new(frames * 2);
    let (mp, mc) = rtrb::RingBuffer::new(64);
    let feeder = Feeder {
        samples: sp,
        markers: mp,
        pending: VecDeque::new(),
        written: 0,
        queued: 0,
        era: 0,
        rate,
        job: None,
        chain: None,
        joint: None,
        staged: Vec::new(),
        staged_at: 0,
        decoded: Vec::with_capacity(STEP * 2),
        resampled: Vec::new(),
    };
    (feeder, (sc, mc))
}

pub(crate) struct Feeder {
    samples: rtrb::Producer<f32>,
    markers: rtrb::Producer<Marker>,
    /// Markers waiting for the frames before them to be written.
    pending: VecDeque<Marker>,
    /// Frames written to the ring, and written plus staged.
    written: u64,
    queued: u64,
    era: u32,
    rate: u32,
    job: Option<Job>,
    /// The song to carry straight on with.
    chain: Option<Chain>,
    /// Where the chained song took over: kept until it's heard, in case it must go after all.
    joint: Option<Joint>,
    /// Frames ready but not yet in the ring (it was full).
    staged: Vec<f32>,
    staged_at: usize,
    decoded: Vec<f32>,
    resampled: Vec<f32>,
}

struct Job {
    source: Source,
    resampler: Resampler,
    handle: u32,
    key: u64,
    /// Resampler output frames still to drop (the lead-in, when it isn't kept).
    drop: usize,
    /// Kept lead-in frames at the stretch's start.
    lead: usize,
    /// Where the stretch's Begin marker sits, and its position.
    begin_at: u64,
    begin_pos: i64,
    /// Source frames fed to the resampler in this stretch.
    fed: u64,
    /// At the song's end, waiting for the engine's word: chain the next song on, or finish.
    held: bool,
    /// Told to finish at the end.
    release: bool,
    done: bool,
}

impl Job {
    fn end(&self) -> i64 {
        match self.source.info().frames {
            Some(f) => self.resampler.out_frames(f) as i64,
            None => UNKNOWN,
        }
    }
}

struct Chain {
    source: Source,
    handle: u32,
    key: u64,
}

#[derive(Clone, Copy)]
struct Joint {
    at: u64,
    era: u32,
    /// The song before: its handle, key, and the position of its end.
    handle: u32,
    key: u64,
    end: i64,
}

/// What a fill step found.
#[derive(Debug, Default)]
pub(crate) struct Fill {
    /// The song broke while decoding (it ends where it broke).
    pub failed: Option<(u32, Error)>,
    /// Frames are waiting for room in the ring.
    pub waiting: bool,
}

impl Feeder {
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// The song being decoded now.
    pub fn handle(&self) -> Option<u32> {
        self.job.as_ref().map(|j| j.handle)
    }

    /// The output position of source frame `frame` of the song being decoded.
    pub fn out_pos(&self, frame: u64) -> i64 {
        match &self.job {
            Some(j) => j.resampler.out_frames(frame) as i64,
            None => frame as i64,
        }
    }

    /// The song's playable length at the output rate, when known.
    pub fn out_len(&self) -> Option<i64> {
        self.job.as_ref().map(Job::end).filter(|e| *e != UNKNOWN)
    }

    /// Loads a song from source frame `from`; returns the new era. `keep_lead_in` keeps the
    /// resampler's pre-ringing before the first frame, for a hand-over between decks.
    pub fn load(
        &mut self,
        mut source: Source,
        handle: u32,
        key: u64,
        from: u64,
        keep_lead_in: bool,
    ) -> Result<u32, Error> {
        if from > 0 {
            source.seek(from)?;
        }
        let rates = (source.info().rate, self.rate);
        let resampler = match self.job.take() {
            Some(mut j) if j.resampler.rates() == rates => {
                j.resampler.reset();
                j.resampler
            }
            _ => Resampler::new(rates.0, rates.1),
        };
        let lead_in = resampler.lead_in();
        let lead = if keep_lead_in { lead_in } else { 0 };
        self.chain = None;
        self.joint = None;
        self.job = Some(Job {
            source,
            resampler,
            handle,
            key,
            drop: lead_in - lead,
            lead,
            begin_at: 0,
            begin_pos: 0,
            fed: 0,
            held: false,
            release: false,
            done: false,
        });
        Ok(self.begin(from))
    }

    /// The same song again from source frame `from` (a seek); returns the new era.
    pub fn restart(&mut self, from: u64) -> Result<u32, Error> {
        let job = self
            .job
            .as_mut()
            .ok_or_else(|| Error::Decode("nothing loaded".into()))?;
        job.source.seek(from)?;
        job.resampler.reset();
        job.drop = job.resampler.lead_in();
        job.lead = 0;
        job.fed = 0;
        job.held = false;
        job.release = false;
        job.done = false;
        self.chain = None;
        self.joint = None;
        Ok(self.begin(from))
    }

    /// Starts a new era at source frame `from` of the loaded job.
    fn begin(&mut self, from: u64) -> u32 {
        self.era = self.era.wrapping_add(1);
        // frames and markers not yet in the ring belong to the old era: gone
        self.staged.clear();
        self.staged_at = 0;
        self.pending.clear();
        self.queued = self.written;
        let job = self.job.as_mut().expect("a job was just set");
        job.begin_at = self.queued;
        job.begin_pos = job.resampler.out_frames(from) as i64 - job.lead as i64;
        let (pos, handle, key, end) = (job.begin_pos, job.handle, job.key, job.end());
        self.pending.push_back(Marker {
            at: self.queued,
            era: self.era,
            kind: MarkerKind::Begin {
                pos,
                handle,
                key,
                end,
            },
        });
        self.era
    }

    /// Carries straight on with `source` after the current song's last frame, if it can (same
    /// sample rate, the current song not decoded to its end yet).
    pub fn chain(&mut self, source: Source, handle: u32, key: u64) -> Result<(), Box<Source>> {
        match &self.job {
            Some(j)
                if !j.done
                    && !j.release
                    && self.chain.is_none()
                    && j.source.info().rate == source.info().rate =>
            {
                self.chain = Some(Chain {
                    source,
                    handle,
                    key,
                });
                Ok(())
            }
            _ => Err(Box::new(source)),
        }
    }

    /// The song chained on but not reached yet, if any.
    pub fn chained(&self) -> Option<u32> {
        self.chain.as_ref().map(|c| c.handle)
    }

    /// Drops the chained song before the feeder reached it.
    pub fn unchain(&mut self) -> bool {
        self.chain.take().is_some()
    }

    /// The chained song already took over in the decoding, but it isn't wanted: the song before
    /// ends where it ends, and nothing follows on this deck. Returns the era that marks the end,
    /// and whether frames of the dropped song are already in the ring (the mixer must skip them).
    pub fn break_chain(&mut self) -> Option<(u32, bool)> {
        let joint = self.joint.take()?;
        let into_ring = self.written > joint.at;
        if into_ring {
            self.staged.clear();
            self.staged_at = 0;
        } else {
            let keep = (joint.at - self.written) as usize;
            let have = (self.staged.len() - self.staged_at) / 2;
            self.staged.truncate(self.staged_at + keep.min(have) * 2);
        }
        self.queued = self.written.max(joint.at);
        // markers for the song before stay (its end); the dropped song's go
        self.pending.retain(|m| m.era < joint.era);
        self.job = None;
        self.chain = None;
        self.era = self.era.wrapping_add(1);
        let at = self.queued;
        let (handle, key, pos) = (joint.handle, joint.key, joint.end);
        self.pending.push_back(Marker {
            at,
            era: self.era,
            kind: MarkerKind::Begin {
                pos,
                handle,
                key,
                end: pos,
            },
        });
        self.pending.push_back(Marker {
            at,
            era: self.era,
            kind: MarkerKind::Stop,
        });
        Some((self.era, into_ring))
    }

    /// At the song's end and waiting: `chain` the next song or `finish`.
    pub fn held(&self) -> bool {
        self.job.as_ref().is_some_and(|j| j.held)
    }

    /// Already rendered into the ring and still available to the sound device.
    pub fn buffered_frames(&self) -> usize {
        (self.samples.buffer().capacity() - self.samples.slots()) / 2
    }

    /// Nothing is chained on: the song ends with its resampler's last frames.
    pub fn finish(&mut self) {
        if let Some(j) = self.job.as_mut() {
            j.release = true;
            j.held = false;
        }
    }

    /// The chained song took over in the decoding (its frames may not be heard yet).
    pub fn joined(&self) -> Option<u32> {
        self.joint.and_then(|_| self.handle())
    }

    /// Stops feeding (the deck's song is over or dropped).
    pub fn clear(&mut self) {
        self.job = None;
        self.chain = None;
        self.joint = None;
        self.staged.clear();
        self.staged_at = 0;
        self.queued = self.written;
        self.pending.clear();
    }

    /// A new output rate: whatever was loaded is dropped.
    pub fn set_rate(&mut self, rate: u32) {
        self.clear();
        self.rate = rate;
    }

    /// Decodes into the ring until it's full or the song is over.
    pub fn fill(&mut self) -> Fill {
        let mut fill = Fill::default();
        loop {
            self.flush();
            if self.staged_at < self.staged.len() {
                fill.waiting = true;
                return fill;
            }
            let Some(job) = self.job.as_mut() else {
                return fill;
            };
            if job.done {
                return fill;
            }
            // room for a step? (writing into a nearly full ring a frame at a time wastes calls)
            if self.samples.slots() < STEP * 2 {
                return fill;
            }
            self.decoded.clear();
            self.resampled.clear();
            let got = match job.source.read(&mut self.decoded, STEP) {
                Ok(n) => n,
                Err(e) => {
                    fill.failed = Some((job.handle, e));
                    0
                }
            };
            if got > 0 {
                job.fed += got as u64;
                job.resampler.process(&self.decoded, &mut self.resampled);
            } else {
                let end_at = job.begin_at + job.lead as u64 + job.resampler.out_frames(job.fed);
                let end_pos = job.begin_pos + (end_at - job.begin_at) as i64;
                if let Some(next) = self.chain.take() {
                    // the next song carries straight on through the same resampler
                    let era = self.era;
                    self.pending.push_back(Marker {
                        at: end_at,
                        era,
                        kind: MarkerKind::End,
                    });
                    self.era = self.era.wrapping_add(1);
                    self.joint = Some(Joint {
                        at: end_at,
                        era: self.era,
                        handle: job.handle,
                        key: job.key,
                        end: end_pos,
                    });
                    job.source = next.source;
                    job.handle = next.handle;
                    job.key = next.key;
                    job.begin_at = end_at;
                    job.begin_pos = 0;
                    job.lead = 0;
                    job.fed = 0;
                    job.held = false;
                    job.release = false;
                    let kind = MarkerKind::Begin {
                        pos: 0,
                        handle: job.handle,
                        key: job.key,
                        end: job.end(),
                    };
                    self.pending.push_back(Marker {
                        at: end_at,
                        era: self.era,
                        kind,
                    });
                    continue;
                }
                if !job.release {
                    // the engine decides what follows (it may chain a song on)
                    job.held = true;
                    return fill;
                }
                // the end: the rest of the resampler, and its ringing past the end
                let tail = job.resampler.lead_in();
                job.resampler.finish(tail, &mut self.resampled);
                job.done = true;
            }
            let mut frames = &self.resampled[..];
            if job.drop > 0 {
                let d = job.drop.min(frames.len() / 2);
                job.drop -= d;
                frames = &frames[d * 2..];
            }
            if self.staged_at == self.staged.len() {
                self.staged.clear();
                self.staged_at = 0;
            }
            self.staged.extend_from_slice(frames);
            self.queued += (frames.len() / 2) as u64;
            if job.done {
                let end = job.begin_at + job.lead as u64 + job.resampler.out_frames(job.fed);
                let era = self.era;
                self.pending.push_back(Marker {
                    at: end.min(self.queued),
                    era,
                    kind: MarkerKind::End,
                });
                self.pending.push_back(Marker {
                    at: self.queued,
                    era,
                    kind: MarkerKind::Stop,
                });
            }
        }
    }

    /// Writes staged frames into the ring, each marker just before its frame.
    fn flush(&mut self) {
        loop {
            while let Some(m) = self.pending.front() {
                if m.at > self.written {
                    break;
                }
                if self.markers.push(*m).is_err() {
                    return;
                }
                self.pending.pop_front();
            }
            let staged = (self.staged.len() - self.staged_at) / 2;
            if staged == 0 {
                return;
            }
            let limit = self
                .pending
                .front()
                .map(|m| (m.at - self.written) as usize)
                .unwrap_or(usize::MAX);
            let n = staged.min(limit).min(self.samples.slots() / 2);
            if n == 0 {
                return;
            }
            let (done, _) = self
                .samples
                .push_partial_slice(&self.staged[self.staged_at..self.staged_at + n * 2]);
            let n = done.len() / 2;
            self.written += n as u64;
            self.staged_at += n * 2;
        }
    }
}
