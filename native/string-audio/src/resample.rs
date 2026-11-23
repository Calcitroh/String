//! A song's frames to the output's sample rate (rubato's FFT resampler; nothing at all when the
//! rates already match). The resampler's own delay is accounted for exactly: output frame `k`
//! after a reset sits at input time `(k - lead_in) / ratio`, so a caller can keep or drop the
//! lead-in and the tail and still know where every frame belongs.

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler as _};

/// Input frames per resampler call: about 21 ms at 48 kHz, in sub-chunks of a few hundred.
const CHUNK: usize = 1024;

pub(crate) struct Resampler {
    fft: Option<Fft<f32>>,
    in_rate: u32,
    out_rate: u32,
    /// Input frames waiting for a full chunk.
    held: Vec<f32>,
    out: Vec<f32>,
    /// Input frames fed since the reset, and output frames made.
    fed: u64,
    made: u64,
}

impl Resampler {
    pub fn new(in_rate: u32, out_rate: u32) -> Resampler {
        let fft = (in_rate != out_rate).then(|| {
            Fft::<f32>::new(
                in_rate as usize,
                out_rate as usize,
                CHUNK,
                2,
                FixedSync::Input,
            )
            .expect("sample rates are positive")
        });
        let max_out = fft.as_ref().map(|f| f.output_frames_max()).unwrap_or(0);
        Resampler {
            fft,
            in_rate,
            out_rate,
            held: Vec::with_capacity(CHUNK * 2),
            out: vec![0.0; max_out * 2],
            fed: 0,
            made: 0,
        }
    }

    pub fn rates(&self) -> (u32, u32) {
        (self.in_rate, self.out_rate)
    }

    /// Output frames of pre-ringing before input frame 0 (0 when passing through).
    pub fn lead_in(&self) -> usize {
        self.fft.as_ref().map(|f| f.output_delay()).unwrap_or(0)
    }

    /// Output frames for `frames` input frames, at the output rate.
    pub fn out_frames(&self, frames: u64) -> u64 {
        if self.in_rate == self.out_rate {
            frames
        } else {
            ((frames as u128 * self.out_rate as u128 + self.in_rate as u128 / 2)
                / self.in_rate as u128) as u64
        }
    }

    pub fn reset(&mut self) {
        if let Some(f) = self.fft.as_mut() {
            f.reset();
        }
        self.held.clear();
        self.fed = 0;
        self.made = 0;
    }

    /// Feeds stereo frames; appends what comes out (with the lead-in, after a reset).
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        self.fed += (input.len() / 2) as u64;
        let Some(fft) = self.fft.as_mut() else {
            self.made += (input.len() / 2) as u64;
            out.extend_from_slice(input);
            return;
        };
        let mut input = input;
        loop {
            let need = fft.input_frames_next();
            if self.held.is_empty() && input.len() >= need * 2 {
                // straight from the caller's slice
                let (now, rest) = input.split_at(need * 2);
                Self::run(fft, &mut self.out, now, None, out, &mut self.made);
                input = rest;
                continue;
            }
            let take = (need * 2 - self.held.len()).min(input.len());
            self.held.extend_from_slice(&input[..take]);
            input = &input[take..];
            if self.held.len() < need * 2 {
                break;
            }
            Self::run(fft, &mut self.out, &self.held, None, out, &mut self.made);
            self.held.clear();
        }
    }

    /// No more input: appends the rest, up to `extra` output frames past the last input frame's
    /// place (the filter's ringing; pass the lead-in to keep all of it).
    pub fn finish(&mut self, extra: usize, out: &mut Vec<f32>) {
        let target = self.lead_in() as u64 + self.out_frames(self.fed) + extra as u64;
        let Some(fft) = self.fft.as_mut() else { return };
        let start = out.len();
        let mut flushed = 0;
        while self.made < target && flushed < 64 {
            let held = std::mem::take(&mut self.held);
            let partial = held.len() / 2;
            let mut buf = held;
            buf.resize(fft.input_frames_next() * 2, 0.0);
            Self::run(fft, &mut self.out, &buf, Some(partial), out, &mut self.made);
            flushed += 1;
        }
        // drop what lies past the target
        if self.made > target {
            let over = (self.made - target) as usize;
            let keep = (out.len() - start) / 2 - over.min((out.len() - start) / 2);
            out.truncate(start + keep * 2);
            self.made = target;
        }
    }

    fn run(
        fft: &mut Fft<f32>,
        scratch: &mut [f32],
        input: &[f32],
        partial: Option<usize>,
        out: &mut Vec<f32>,
        made: &mut u64,
    ) {
        let frames_in = input.len() / 2;
        let frames_out = scratch.len() / 2;
        let src = InterleavedSlice::new(input, 2, frames_in).expect("stereo input");
        let mut dst = InterleavedSlice::new_mut(scratch, 2, frames_out).expect("stereo output");
        let idx = partial.map(|p| Indexing::new().partial_len(p));
        let (_, n) = fft
            .process_into_buffer(&src, &mut dst, idx.as_ref())
            .expect("buffers sized by the resampler");
        out.extend_from_slice(&scratch[..n * 2]);
        *made += n as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, hz: f64, frames: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let v =
                    (2.0 * std::f64::consts::PI * hz * i as f64 / rate as f64).sin() as f32 * 0.5;
                [v, v]
            })
            .collect()
    }

    #[test]
    fn passes_through_when_rates_match() {
        let mut r = Resampler::new(48_000, 48_000);
        let input = sine(48_000, 1000.0, 1000);
        let mut out = Vec::new();
        r.process(&input, &mut out);
        r.finish(0, &mut out);
        assert_eq!(out, input);
        assert_eq!(r.lead_in(), 0);
    }

    #[test]
    fn lands_every_frame_where_it_belongs() {
        // 44.1 kHz to 48 kHz: after dropping the lead-in, output frame k is input time k / ratio
        let (a, b) = (44_100u32, 48_000u32);
        let frames = 44_100;
        let input = sine(a, 997.0, frames);
        let mut r = Resampler::new(a, b);
        let mut out = Vec::new();
        for chunk in input.chunks(2 * 777) {
            r.process(chunk, &mut out);
        }
        r.finish(0, &mut out);
        let lead = r.lead_in();
        let body = &out[lead * 2..];
        assert_eq!(body.len() / 2, 48_000, "length follows the ratio exactly");
        let want = sine(b, 997.0, 48_000);
        // away from the edges (where the input simply stops) the two agree closely
        let worst = (2_000..46_000)
            .map(|i| (body[i * 2] - want[i * 2]).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 1e-3, "worst error {worst}");
    }
}
