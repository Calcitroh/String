//! Numbers the sound thread writes and the screens read, without locks: a sequence lock over
//! plain atomics (a writer bumps the count before and after; a reader retries if the count moved
//! or is odd). Reading is a handful of atomic loads, cheap enough for every frame at 279 Hz.

use std::sync::atomic::{AtomicU64, Ordering, fence};

pub(crate) struct SeqLock<const N: usize> {
    seq: AtomicU64,
    words: [AtomicU64; N],
}

impl<const N: usize> SeqLock<N> {
    pub fn new() -> Self {
        SeqLock {
            seq: AtomicU64::new(0),
            words: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }

    /// One writer only (the sound thread).
    pub fn write(&self, words: &[u64; N]) {
        let s = self.seq.load(Ordering::Relaxed);
        self.seq.store(s + 1, Ordering::Relaxed);
        fence(Ordering::Release);
        for (w, v) in self.words.iter().zip(words) {
            w.store(*v, Ordering::Relaxed);
        }
        self.seq.store(s + 2, Ordering::Release);
    }

    /// The latest whole write, and its number (0: nothing written yet).
    pub fn read(&self, out: &mut [u64; N]) -> u64 {
        loop {
            let a = self.seq.load(Ordering::Acquire);
            if a % 2 == 1 {
                std::hint::spin_loop();
                continue;
            }
            for (o, w) in out.iter_mut().zip(&self.words) {
                *o = w.load(Ordering::Relaxed);
            }
            fence(Ordering::Acquire);
            if self.seq.load(Ordering::Relaxed) == a {
                return a / 2;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn readers_never_see_half_a_write() {
        let lock = Arc::new(SeqLock::<8>::new());
        let w = lock.clone();
        let writer = std::thread::spawn(move || {
            for i in 1..200_000u64 {
                w.write(&[i; 8]);
            }
        });
        let mut out = [0u64; 8];
        for _ in 0..200_000 {
            lock.read(&mut out);
            assert!(out.iter().all(|v| *v == out[0]));
        }
        writer.join().unwrap();
    }
}
