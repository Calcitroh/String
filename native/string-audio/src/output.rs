//! The sound device: the system's default output through WASAPI shared mode (cpal), at the
//! device's own rate so nothing but our resampler touches the sound. cpal's default-device
//! stream follows Windows when the default device changes; when the stream breaks anyway
//! (headphones pulled with nothing to fall back to, a driver reset) or stops calling back, it is
//! opened again on whatever the default is then, once a second until one is there. A pause or
//! queue end closes the stream after the last buffer plays; the next play opens a fresh stream.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::Error;
use crate::engine::Engine;
use crate::mixer::Mixer;
use crate::types::Event;

/// A playing stream that hasn't called back for this long is taken as dead.
const SILENT_LIMIT: Duration = Duration::from_millis(1500);
const RETRY: Duration = Duration::from_secs(1);
/// The largest block the sound thread renders at once (bigger device periods come in pieces).
const MAX_BLOCK: usize = 8192;

pub(crate) struct Output<S = (cpal::Stream, cpal::Device)> {
    mixer: Arc<Mutex<Mixer>>,
    stream: Option<S>,
    default_device: Option<cpal::Device>,
    waiting: bool,
    broken: Arc<AtomicBool>,
    beats: Arc<AtomicU64>,
    last_beats: u64,
    last_beat_at: Instant,
    retry_at: Instant,
    epoch: Instant,
}

/// The default device's rate, if there is a device.
pub(crate) fn default_rate() -> Option<u32> {
    let device = cpal::default_host().default_output_device()?;
    device.default_output_config().ok().map(|c| c.sample_rate())
}

impl<S> Output<S> {
    pub fn new(mixer: Arc<Mutex<Mixer>>, epoch: Instant) -> Self {
        Output {
            mixer,
            stream: None,
            default_device: None,
            waiting: false,
            broken: Arc::new(AtomicBool::new(false)),
            beats: Arc::new(AtomicU64::new(0)),
            last_beats: 0,
            last_beat_at: epoch,
            retry_at: epoch,
            epoch,
        }
    }

    // Inject just the device operations and clock. The decision to drop, wait, retry and
    // resynchronise the real decks is the same with an in-memory stream in tests.
    fn update_with(
        &mut self,
        now: Instant,
        engine: &mut Engine,
        open: impl FnOnce(&mut Self) -> Result<(S, u32), Error>,
        start: impl FnOnce(&S) -> Result<String, Error>,
    ) {
        let now_ns = now.duration_since(self.epoch).as_nanos() as u64;
        // Keep the last buffer through its playback time, even after its fade has rendered.
        let drained = self.stream.is_some()
            && !engine.playing()
            && self.mixer.lock().unwrap().drained(now_ns);
        let reopen = drained || self.needs_reopen(now);
        if reopen && self.stream.take().is_some() {
            // Collect anything the retiring stream reported in its final callback.
            engine.pump();
        }
        if self.stream.is_some() {
            return;
        }
        if !engine.playing() {
            // Cues and paused seeks still publish a position without a device callback.
            let mut mixer = self.mixer.lock().unwrap();
            mixer.settle_pause();
            mixer.render(&mut [], now_ns);
            self.waiting = false;
            self.retry_at = now;
            return;
        }
        if !self.waiting {
            self.mixer.lock().unwrap().output_waiting();
            self.waiting = true;
        }
        if !reopen {
            return;
        }
        self.retry_at = now + RETRY;
        self.broken.store(false, Ordering::Relaxed);
        let Ok((stream, rate)) = open(self) else {
            return;
        };
        {
            let mut mixer = self.mixer.lock().unwrap();
            if mixer.rate() != rate {
                mixer.set_rate(rate);
                let rendered_eras = mixer.reset();
                drop(mixer);
                engine.resync(rate, rendered_eras);
            }
        }
        self.mixer.lock().unwrap().prime();
        engine.pump();
        if engine.playing()
            && let Ok(name) = start(&stream)
        {
            self.stream = Some(stream);
            self.waiting = false;
            self.last_beat_at = now;
            self.last_beats = self.beats.load(Ordering::Relaxed);
            engine.notify(Event::Output { name, rate });
        }
    }

    fn needs_reopen(&mut self, now: Instant) -> bool {
        if self.stream.is_none() {
            return now >= self.retry_at;
        }
        if self.broken.load(Ordering::Relaxed) {
            return true;
        }
        let beats = self.beats.load(Ordering::Relaxed);
        if beats != self.last_beats {
            self.last_beats = beats;
            self.last_beat_at = now;
            return false;
        }
        now.duration_since(self.last_beat_at) > SILENT_LIMIT
    }
}

impl Output {
    pub fn update(&mut self, engine: &mut Engine) {
        self.update_with(Instant::now(), engine, Self::open, |(stream, device)| {
            stream.play().map_err(|e| Error::Output(e.to_string()))?;
            // Reading endpoint properties can take longer than a device period. Let
            // the first callback play while Windows looks up the display name.
            Ok(device
                .description()
                .map(|d| d.name().to_owned())
                .unwrap_or_else(|_| "Speakers".into()))
        });
    }

    /// Build the stream paused. A replacement must never render at the old mixer's rate.
    fn open(&mut self) -> Result<((cpal::Stream, cpal::Device), u32), Error> {
        let device = self
            .default_device
            .take()
            .or_else(|| cpal::default_host().default_output_device())
            .ok_or_else(|| Error::Output("no output device".into()))?;
        let config = device
            .default_output_config()
            .map_err(|e| Error::Output(e.to_string()))?;
        let rate = config.sample_rate();
        let channels = config.channels().max(1) as usize;
        let stream_config = cpal::StreamConfig {
            channels: channels as u16,
            sample_rate: rate,
            buffer_size: cpal::BufferSize::Default,
        };
        let mixer = self.mixer.clone();
        let beats = self.beats.clone();
        let epoch = self.epoch;
        let mut stereo = vec![0.0f32; MAX_BLOCK * 2];
        let broken = self.broken.clone();
        let stream = device
            .build_output_stream::<f32, _, _>(
                stream_config,
                move |data: &mut [f32], info: &cpal::OutputCallbackInfo| {
                    let ts = info.timestamp();
                    let latency = ts
                        .playback
                        .checked_duration_since(ts.callback)
                        .unwrap_or_default();
                    let mut heard = (epoch.elapsed() + latency).as_nanos() as u64;
                    let rate = rate.max(1) as u64;
                    for chunk in data.chunks_mut(MAX_BLOCK * channels) {
                        let frames = chunk.len() / channels;
                        let buf = &mut stereo[..frames * 2];
                        match mixer.try_lock() {
                            Ok(mut m) => m.render(buf, heard),
                            Err(_) => buf.fill(0.0),
                        }
                        spread(buf, chunk, channels);
                        heard += frames as u64 * 1_000_000_000 / rate;
                    }
                    beats.fetch_add(1, Ordering::Relaxed);
                },
                move |_err| broken.store(true, Ordering::Relaxed),
                None,
            )
            .map_err(|e| Error::Output(e.to_string()))?;
        // WASAPI's default-device handle resolves the current endpoint on activation.
        // Once build consumes its audio client, keep only that handle for the next wake.
        if cfg!(windows) {
            self.default_device = Some(device.clone());
        }
        Ok(((stream, device), rate))
    }
}

#[cfg(test)]
mod tests;

/// Stereo into the device's channels: left and right first, the rest silent; mono gets both.
fn spread(stereo: &[f32], out: &mut [f32], channels: usize) {
    match channels {
        2 => out.copy_from_slice(stereo),
        1 => {
            for (o, f) in out.iter_mut().zip(stereo.as_chunks::<2>().0) {
                *o = 0.5 * (f[0] + f[1]);
            }
        }
        c => {
            for (o, f) in out.chunks_exact_mut(c).zip(stereo.as_chunks::<2>().0) {
                o[0] = f[0];
                o[1] = f[1];
                o[2..].fill(0.0);
            }
        }
    }
}
