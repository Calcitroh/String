//! The visualizer's feed: 64 log-spaced bands from 35 Hz to 15 kHz, measured from what the
//! decks play (before the volume, so muting doesn't stop the picture), the way the JS player's
//! Web Audio analyser did it: a 2048-point FFT with a Blackman window, magnitudes smoothed with
//! 0.7 of the last frame, decibels mapped from -88..-12 dB to 0..1, the loudest bin per band.
//!
//! The sound thread measures about 280 times a second (the rate the JS analyser ran at on a
//! 279 Hz screen, so the smoothing feels the same) and publishes the bands; a screen reads the
//! latest frame with a few atomic loads. The screen's own shaping (`pow(1.5)`, the treble lift,
//! its per-frame easing) stays on the screen's side, as in viz.js.

use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};

use crate::shared::SeqLock;

pub const BANDS: usize = 64;
const SIZE: usize = 2048;
const SMOOTHING: f32 = 0.7;
const MIN_DB: f32 = -88.0;
const MAX_DB: f32 = -12.0;
const LOW_HZ: f64 = 35.0;
const HIGH_HZ: f64 = 15_000.0;
/// Measurements per second.
const RATE_HZ: u32 = 280;

/// The published bands: 64 values packed two to a word, then the frame count.
pub(crate) type Published = SeqLock<{ BANDS / 2 }>;

/// A handle on the latest bands: cheap to clone, cheap to read, from any thread.
#[derive(Clone)]
pub struct Spectrum {
    pub(crate) shared: Arc<Published>,
}

impl Spectrum {
    pub(crate) fn new() -> Spectrum {
        Spectrum {
            shared: Arc::new(SeqLock::new()),
        }
    }

    /// Copies the latest 64 bands (0 silent to 1 loud, low to high) into `out` and returns how
    /// many frames have been measured so far (0: none yet).
    pub fn read(&self, out: &mut [f32; BANDS]) -> u64 {
        let mut words = [0u64; BANDS / 2];
        let n = self.shared.read(&mut words);
        for (pair, w) in out.as_chunks_mut::<2>().0.iter_mut().zip(words) {
            pair[0] = f32::from_bits(w as u32);
            pair[1] = f32::from_bits((w >> 32) as u32);
        }
        n
    }
}

pub(crate) struct Analyser {
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    /// The last SIZE mono samples, oldest at `at`.
    history: Vec<f32>,
    at: usize,
    hop: usize,
    since: usize,
    input: Vec<f32>,
    output: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    smooth: Vec<f32>,
    /// Bin ranges per band.
    edges: [(usize, usize); BANDS],
    top_bin: usize,
    /// Samples since the last non-silent one (a quiet analyser stops working).
    quiet: usize,
    resting: bool,
    out: Arc<Published>,
}

impl Analyser {
    pub fn new(rate: u32, out: Arc<Published>) -> Analyser {
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(SIZE);
        let window = (0..SIZE)
            .map(|n| {
                let x = std::f32::consts::TAU * n as f32 / SIZE as f32;
                0.42 - 0.5 * x.cos() + 0.08 * (2.0 * x).cos()
            })
            .collect();
        let mut a = Analyser {
            input: fft.make_input_vec(),
            output: fft.make_output_vec(),
            scratch: fft.make_scratch_vec(),
            fft,
            window,
            history: vec![0.0; SIZE],
            at: 0,
            hop: 1,
            since: 0,
            smooth: vec![0.0; SIZE / 2 + 1],
            edges: [(1, 2); BANDS],
            top_bin: 2,
            quiet: 0,
            resting: true,
            out,
        };
        a.set_rate(rate);
        a
    }

    pub fn set_rate(&mut self, rate: u32) {
        self.hop = (rate / RATE_HZ).max(1) as usize;
        let bin_hz = rate as f64 / SIZE as f64;
        let edge = |i: usize| -> usize {
            let hz = LOW_HZ * (HIGH_HZ / LOW_HZ).powf(i as f64 / BANDS as f64);
            ((hz / bin_hz).round() as usize).max(1)
        };
        let bins = SIZE / 2;
        for (b, e) in self.edges.iter_mut().enumerate() {
            let lo = edge(b).min(bins - 1);
            let hi = edge(b + 1).max(lo + 1).min(bins);
            *e = (lo, hi);
        }
        self.top_bin = self.edges[BANDS - 1].1;
    }

    /// Takes in a block of stereo frames.
    pub fn feed(&mut self, block: &[f32]) {
        for f in block.as_chunks::<2>().0 {
            let m = 0.5 * (f[0] + f[1]);
            if m != 0.0 {
                self.quiet = 0;
                self.resting = false;
            } else {
                self.quiet += 1;
            }
            self.history[self.at] = m;
            self.at = (self.at + 1) % SIZE;
            self.since += 1;
            if self.since >= self.hop {
                self.since = 0;
                if !self.resting {
                    self.measure();
                }
            }
        }
    }

    fn measure(&mut self) {
        let silent = self.quiet >= SIZE;
        if silent {
            // nothing but zeros in the window: the bands only fall
            for s in &mut self.smooth[..self.top_bin] {
                *s *= SMOOTHING;
            }
        } else {
            let (old, new) = self.history.split_at(self.at);
            for ((i, w), s) in self
                .input
                .iter_mut()
                .zip(&self.window)
                .zip(new.iter().chain(old))
            {
                *i = s * w;
            }
            if self
                .fft
                .process_with_scratch(&mut self.input, &mut self.output, &mut self.scratch)
                .is_err()
            {
                return;
            }
            let norm = 1.0 / SIZE as f32;
            for k in 1..self.top_bin {
                let mag = self.output[k].norm() * norm;
                self.smooth[k] = SMOOTHING * self.smooth[k] + (1.0 - SMOOTHING) * mag;
            }
        }
        let mut words = [0u64; BANDS / 2];
        let mut any = false;
        for (b, &(lo, hi)) in self.edges.iter().enumerate() {
            let peak = self.smooth[lo..hi].iter().copied().fold(0.0f32, f32::max);
            let v = if peak > 0.0 {
                ((20.0 * peak.log10() - MIN_DB) / (MAX_DB - MIN_DB)).clamp(0.0, 1.0)
            } else {
                0.0
            };
            any |= v > 0.0;
            words[b / 2] |= (v.to_bits() as u64) << (32 * (b % 2));
        }
        self.out.write(&words);
        if silent && !any {
            // everything has fallen to nothing: rest until sound comes back
            self.resting = true;
            self.smooth.iter_mut().for_each(|s| *s = 0.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(rate: u32, hz: f32, amp: f32, secs: f32) -> [f32; BANDS] {
        let spec = Spectrum::new();
        let mut a = Analyser::new(rate, spec.shared.clone());
        let n = (rate as f32 * secs) as usize;
        let block: Vec<f32> = (0..n)
            .flat_map(|i| {
                let v = amp * (std::f32::consts::TAU * hz * i as f32 / rate as f32).sin();
                [v, v]
            })
            .collect();
        a.feed(&block);
        let mut out = [0.0; BANDS];
        assert!(spec.read(&mut out) > 0);
        out
    }

    #[test]
    fn a_tone_lights_its_band() {
        let bands = run(48_000, 1000.0, 0.5, 0.5);
        let loudest = (0..BANDS)
            .max_by(|a, b| bands[*a].total_cmp(&bands[*b]))
            .unwrap();
        // 1 kHz sits about 37 bands up a 35 Hz..15 kHz log scale of 64
        let want = (64.0 * (1000.0f64 / 35.0).ln() / (15000.0f64 / 35.0).ln()) as usize;
        assert!(
            loudest.abs_diff(want) <= 1,
            "band {loudest}, want about {want}"
        );
        assert!(
            bands[loudest] > 0.8,
            "a loud tone reads near the top: {}",
            bands[loudest]
        );
        assert!(bands[5] < 0.1 && bands[60] < 0.1, "far bands stay dark");
    }

    #[test]
    fn same_picture_at_any_rate() {
        // the FFT is 2048 points at any rate (as Web Audio's), so bins fall a little differently
        // beside a pure tone; the picture as a whole stays the same
        let a = run(44_100, 440.0, 0.3, 0.5);
        let b = run(48_000, 440.0, 0.3, 0.5);
        let peak = |x: &[f32; BANDS]| (0..BANDS).max_by(|i, j| x[*i].total_cmp(&x[*j])).unwrap();
        assert!(peak(&a).abs_diff(peak(&b)) <= 1);
        let mean = a.iter().zip(&b).map(|(x, y)| (x - y).abs()).sum::<f32>() / BANDS as f32;
        assert!(mean < 0.05, "the pictures differ by {mean} on average");
    }

    #[test]
    fn silence_falls_to_zero_then_rests() {
        let spec = Spectrum::new();
        let mut a = Analyser::new(48_000, spec.shared.clone());
        let tone: Vec<f32> = (0..24_000)
            .flat_map(|i| {
                let v = 0.5 * (i as f32 * 0.1).sin();
                [v, v]
            })
            .collect();
        a.feed(&tone);
        a.feed(&vec![0.0; 2 * 48_000]);
        let mut out = [1.0; BANDS];
        spec.read(&mut out);
        assert!(out.iter().all(|v| *v == 0.0));
        assert!(a.resting);
    }
}
