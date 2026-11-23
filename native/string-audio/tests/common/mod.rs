//! Test songs and measuring tools shared by the tests. WAVs are written here; the lossy formats
//! are made by FFmpeg (on the PATH, or at the path in the `FFMPEG` environment variable, as for
//! String's other tests). Tests that need FFmpeg say so and pass without it.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

pub const RATE: u32 = 44_100;
/// The gapless test signal is cut into three songs at these frames.
pub const CUTS: [usize; 4] = [0, 52_000, 110_000, 132_300];

/// The test signal: two tones, a little different on each side, so a gap, a doubled stretch or
/// a click at a cut shows up against it.
pub fn signal(rate: u32, frames: usize) -> Vec<f32> {
    let tau = std::f64::consts::TAU;
    let mut out = Vec::with_capacity(frames * 2);
    for n in 0..frames {
        let t = n as f64 / rate as f64;
        let l = 0.4 * (tau * 441.0 * t).sin() + 0.2 * (tau * 1234.5 * t + 0.3).sin();
        let r = 0.4 * (tau * 441.0 * t + 0.7).sin() + 0.2 * (tau * 1234.5 * t).sin();
        out.push(quantize(l));
        out.push(quantize(r));
    }
    out
}

/// The test signal from `start` seconds on (for a song that continues another at another rate).
pub fn signal_from(rate: u32, start: f64, frames: usize) -> Vec<f32> {
    let tau = std::f64::consts::TAU;
    let mut out = Vec::with_capacity(frames * 2);
    for n in 0..frames {
        let t = start + n as f64 / rate as f64;
        let l = 0.4 * (tau * 441.0 * t).sin() + 0.2 * (tau * 1234.5 * t + 0.3).sin();
        let r = 0.4 * (tau * 441.0 * t + 0.7).sin() + 0.2 * (tau * 1234.5 * t).sin();
        out.push(quantize(l));
        out.push(quantize(r));
    }
    out
}

/// What a 16-bit WAV holds for a sample.
pub fn quantize(v: f64) -> f32 {
    ((v * 32767.0).round().clamp(-32768.0, 32767.0) / 32768.0) as f32
}

/// A steady tone on both sides.
pub fn tone(rate: u32, hz: f64, amp: f64, frames: usize) -> Vec<f32> {
    let tau = std::f64::consts::TAU;
    (0..frames)
        .flat_map(|n| {
            let v = quantize(amp * (tau * hz * n as f64 / rate as f64).sin());
            [v, v]
        })
        .collect()
}

/// A constant level on both sides (a DC "song": gains show up as plain numbers).
pub fn level(value: f64, frames: usize) -> Vec<f32> {
    vec![quantize(value); frames * 2]
}

pub fn write_wav(path: &Path, rate: u32, samples: &[f32]) {
    let data_len = (samples.len() * 2) as u32;
    let mut b = Vec::with_capacity(44 + data_len as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * 4).to_le_bytes());
    b.extend_from_slice(&4u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        let v = (s * 32768.0).round().clamp(-32768.0, 32767.0) as i16;
        b.extend_from_slice(&v.to_le_bytes());
    }
    std::fs::write(path, b).expect("write wav");
}

/// A scratch folder for one test's own files.
pub fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("string-audio")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch folder");
    dir
}

pub fn ffmpeg() -> Option<&'static Path> {
    static FOUND: OnceLock<Option<PathBuf>> = OnceLock::new();
    FOUND
        .get_or_init(|| {
            let exe = std::env::var_os("FFMPEG")
                .map(PathBuf::from)
                .unwrap_or_else(|| "ffmpeg".into());
            let ok = Command::new(&exe)
                .arg("-version")
                .output()
                .map(|o| o.status.success());
            ok.unwrap_or(false).then_some(exe)
        })
        .as_deref()
}

/// The formats the gapless fixtures come in: file suffix, FFmpeg encoder arguments, and whether
/// decoding must give back the samples exactly.
pub const FORMATS: [(&str, &[&str], bool); 7] = [
    ("wav", &["-c:a", "pcm_s16le"], true),
    ("flac", &["-c:a", "flac"], true),
    ("alac.m4a", &["-c:a", "alac"], true),
    ("mp3", &["-c:a", "libmp3lame", "-b:a", "256k"], false),
    ("aac.m4a", &["-c:a", "aac", "-b:a", "256k"], false),
    ("ogg", &["-c:a", "libvorbis", "-q:a", "8"], false),
    ("opus", &["-c:a", "libopus", "-b:a", "256k"], false),
];

/// The sample rate a format's fixtures are made at (Opus only runs at 48 kHz).
pub fn rate_of(suffix: &str) -> u32 {
    if suffix == "opus" { 48_000 } else { RATE }
}

/// The cuts for a format's rate.
pub fn cuts_of(suffix: &str) -> [usize; 4] {
    let r = rate_of(suffix) as u64;
    CUTS.map(|c| (c as u64 * r / RATE as u64) as usize)
}

/// The three-part gapless fixtures, made once per test run. `None` without FFmpeg.
pub fn gapless_parts() -> Option<&'static Path> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        let ffmpeg = ffmpeg()?;
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("string-audio")
            .join("gapless-v1");
        let stamp = dir.join("done");
        if stamp.exists() {
            return Some(dir);
        }
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).ok()?;
        for (suffix, args, _) in FORMATS {
            let rate = rate_of(suffix);
            let cuts = cuts_of(suffix);
            let whole = signal(rate, cuts[3]);
            for part in 0..3 {
                let wav = dir.join(format!("src-{rate}-{part}.wav"));
                if !wav.exists() {
                    write_wav(&wav, rate, &whole[cuts[part] * 2..cuts[part + 1] * 2]);
                }
                let out = dir.join(format!("part{part}.{suffix}"));
                let status = Command::new(ffmpeg)
                    .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
                    .arg(&wav)
                    .args(args)
                    .arg(&out)
                    .status()
                    .ok()?;
                assert!(status.success(), "FFmpeg made {}", out.display());
            }
        }
        std::fs::write(&stamp, b"").ok()?;
        Some(dir)
    })
    .as_deref()
}

pub fn part(dir: &Path, suffix: &str, n: usize) -> PathBuf {
    dir.join(format!("part{n}.{suffix}"))
}

pub fn skip(why: &str) {
    eprintln!("SKIPPED: {why} (set FFMPEG to FFmpeg's path)");
}

/// The left channel of interleaved stereo.
pub fn left(samples: &[f32]) -> Vec<f32> {
    samples.iter().step_by(2).copied().collect()
}

/// The largest jump between neighbouring frames' slopes (the second difference) in a range:
/// a click or a gap shows up as a spike in it.
pub fn roughness(x: &[f32], from: usize, to: usize) -> f32 {
    let from = from.max(1);
    let to = to.min(x.len().saturating_sub(1));
    (from..to)
        .map(|i| (x[i + 1] - 2.0 * x[i] + x[i - 1]).abs())
        .fold(0.0, f32::max)
}

/// Root-mean-square of a range.
pub fn rms(x: &[f32]) -> f32 {
    if x.is_empty() {
        return 0.0;
    }
    (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
}

/// The largest difference between two signals over a range of frames (both interleaved stereo).
pub fn max_diff(a: &[f32], b: &[f32], from: usize, to: usize) -> f32 {
    (from * 2..(to * 2).min(a.len()).min(b.len()))
        .map(|i| (a[i] - b[i]).abs())
        .fold(0.0, f32::max)
}

/// A song whose every frame says where it is: left = thousands of frames, right = the rest, in
/// steps of 32 sample values, so the position of a frame heard at full level reads back exactly.
pub fn numbered(frames: usize) -> Vec<f32> {
    assert!(frames < 1_000_000);
    let mut out = Vec::with_capacity(frames * 2);
    for n in 0..frames {
        out.push(quantize((n / 1000) as f64 / 1024.0));
        out.push(quantize((n % 1000) as f64 / 1024.0));
    }
    out
}

/// The frame a `numbered` song's frame came from (if it was heard at full level).
pub fn number_of(l: f32, r: f32) -> Option<usize> {
    let (cl, cr) = (l as f64 * 1024.0, r as f64 * 1024.0);
    let (coarse, fine) = (cl.round(), cr.round());
    // both halves must sit on their grid, or the frame wasn't at full level
    if coarse < 0.0
        || !(0.0..1000.0).contains(&fine)
        || (cl - coarse).abs() > 0.25
        || (cr - fine).abs() > 0.25
    {
        return None;
    }
    Some(coarse as usize * 1000 + fine as usize)
}

/// A host for tests: a fixed list of songs, played in order, and every event kept.
pub struct ListHost {
    pub tracks: Vec<string_audio::Track>,
    pub events: std::sync::Arc<std::sync::Mutex<Vec<string_audio::Event>>>,
}

impl ListHost {
    pub fn new(
        tracks: Vec<string_audio::Track>,
    ) -> (
        ListHost,
        std::sync::Arc<std::sync::Mutex<Vec<string_audio::Event>>>,
    ) {
        let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        (
            ListHost {
                tracks,
                events: events.clone(),
            },
            events,
        )
    }
}

impl string_audio::Host for ListHost {
    fn next_after(&mut self, current: &string_audio::Track) -> Option<string_audio::Track> {
        let i = self.tracks.iter().position(|t| t.key == current.key)?;
        self.tracks.get(i + 1).cloned()
    }

    fn event(&mut self, event: string_audio::Event) {
        self.events.lock().unwrap().push(event);
    }
}

/// The resampler's ringing after a song's last frame lasts at most this many frames.
pub const RINGING: usize = 400;

/// The first frame from `from` on where the sound is (nearly) silent for good.
pub fn sound_ends(x: &[f32], from: usize) -> usize {
    let frames = x.len() / 2;
    let mut last = from;
    for i in from..frames {
        if x[i * 2].abs() > 1e-4 || x[i * 2 + 1].abs() > 1e-4 {
            last = i + 1;
        }
    }
    last
}
