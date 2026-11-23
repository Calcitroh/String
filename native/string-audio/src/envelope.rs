//! Gain that moves over a set number of frames: straight lines for the short fades (play,
//! pause, seek, the loop's dip, like the JS player's ramps) and quarter sine and cosine curves
//! for equal-power crossfades (the two always sum to the same loudness).

use std::f32::consts::FRAC_PI_2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shape {
    Linear,
    /// Rises along sin(πx/2): an equal-power fade in.
    SineIn,
    /// Falls along cos(πx/2): an equal-power fade out.
    CosineOut,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Envelope {
    from: f32,
    to: f32,
    len: u32,
    at: u32,
    shape: Shape,
}

impl Envelope {
    pub fn steady(v: f32) -> Envelope {
        Envelope {
            from: v,
            to: v,
            len: 0,
            at: 0,
            shape: Shape::Linear,
        }
    }

    /// From the gain now to `to` over `len` frames (at once when `len` is 0).
    pub fn ramp(&mut self, to: f32, len: u32, shape: Shape) {
        let from = self.value();
        *self = Envelope {
            from,
            to,
            len,
            at: 0,
            shape,
        };
    }

    pub fn set(&mut self, v: f32) {
        *self = Envelope::steady(v);
    }

    #[inline]
    pub fn value(&self) -> f32 {
        self.value_at(self.at)
    }

    #[inline]
    fn value_at(&self, at: u32) -> f32 {
        if at >= self.len {
            return self.to;
        }
        let x = at as f32 / self.len as f32;
        let k = match self.shape {
            Shape::Linear => x,
            Shape::SineIn => (x * FRAC_PI_2).sin(),
            Shape::CosineOut => 1.0 - (x * FRAC_PI_2).cos(),
        };
        self.from + (self.to - self.from) * k
    }

    pub fn target(&self) -> f32 {
        self.to
    }

    pub fn moving(&self) -> bool {
        self.at < self.len
    }

    /// Frames until it arrives.
    pub fn left(&self) -> u32 {
        self.len - self.at.min(self.len)
    }

    pub fn advance(&mut self, frames: usize) {
        self.at = (self.at as usize + frames).min(self.len as usize) as u32;
    }

    /// Adds `src` (stereo) times the gain into `out`, moving the gain along.
    pub fn mix(&mut self, src: &[f32], out: &mut [f32]) {
        let frames = src.len() / 2;
        if !self.moving() {
            let g = self.to;
            if g == 1.0 {
                for (o, s) in out.iter_mut().zip(src) {
                    *o += *s;
                }
            } else if g != 0.0 {
                for (o, s) in out.iter_mut().zip(src) {
                    *o += *s * g;
                }
            }
            return;
        }
        for (i, (o, s)) in out
            .as_chunks_mut::<2>()
            .0
            .iter_mut()
            .zip(src.as_chunks::<2>().0)
            .enumerate()
        {
            let g = self.value_at(self.at + i as u32);
            o[0] += s[0] * g;
            o[1] += s[1] * g;
        }
        self.advance(frames);
    }

    /// Scales `buf` (stereo) by the gain, moving it along.
    pub fn scale(&mut self, buf: &mut [f32]) {
        let frames = buf.len() / 2;
        if !self.moving() {
            let g = self.to;
            if g != 1.0 {
                buf.iter_mut().for_each(|v| *v *= g);
            }
            return;
        }
        for (i, f) in buf.as_chunks_mut::<2>().0.iter_mut().enumerate() {
            let g = self.value_at(self.at + i as u32);
            f[0] *= g;
            f[1] *= g;
        }
        self.advance(frames);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_ramp_lands_on_time() {
        let mut e = Envelope::steady(0.0);
        e.ramp(1.0, 100, Shape::Linear);
        let mut buf = vec![1.0f32; 2 * 150];
        e.scale(&mut buf);
        assert_eq!(buf[0], 0.0);
        assert!((buf[2 * 50] - 0.5).abs() < 1e-6);
        assert_eq!(buf[2 * 100], 1.0);
        assert!(!e.moving());
    }

    #[test]
    fn equal_power_pair_keeps_the_power() {
        let mut a = Envelope::steady(0.0);
        let mut b = Envelope::steady(1.0);
        a.ramp(1.0, 1000, Shape::SineIn);
        b.ramp(0.0, 1000, Shape::CosineOut);
        for _ in 0..1000 {
            let (x, y) = (a.value(), b.value());
            assert!((x * x + y * y - 1.0).abs() < 1e-5);
            a.advance(1);
            b.advance(1);
        }
        assert_eq!((a.value(), b.value()), (1.0, 0.0));
    }
}
