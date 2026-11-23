//! Checks a recording of the real output against the engine rendered offline: after
//! `stringplay --volume <v> --record <rec.wav> <songs...>`, run
//! `cargo test -p string-audio --test device -- --ignored` with `STRING_RECORDING=<rec.wav>`,
//! `STRING_SONGS=<song;song;...>` and `STRING_VOLUME=<v>`. It finds where the songs start in the
//! recording and says how far the device's sound strays from the engine's, frame by frame: a gap
//! or a glitch on the way to the device shows up as a jump in that difference.

mod common;

use std::path::PathBuf;

use common::*;
use string_audio::{Config, OfflinePlayer, Track};

/// A WAV's samples (16-bit or 32-bit float), rate and channel count.
fn read_wav(path: &std::path::Path) -> (Vec<f32>, u32, usize) {
    let b = std::fs::read(path).expect("read the recording");
    let (mut at, mut fmt, mut rate, mut channels, mut data) = (12, 0u16, 0u32, 0usize, None);
    while at + 8 <= b.len() {
        let id = &b[at..at + 4];
        let len = u32::from_le_bytes(b[at + 4..at + 8].try_into().unwrap()) as usize;
        let body = &b[at + 8..(at + 8 + len).min(b.len())];
        if id == b"fmt " {
            fmt = u16::from_le_bytes([body[0], body[1]]);
            channels = u16::from_le_bytes([body[2], body[3]]) as usize;
            rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
        } else if id == b"data" {
            data = Some(body.to_vec());
        }
        at += 8 + len + (len & 1);
    }
    let data = data.expect("a data chunk");
    let samples = if fmt == 3 {
        data.as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes(*c))
            .collect()
    } else {
        data.as_chunks::<2>()
            .0
            .iter()
            .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
            .collect()
    };
    (samples, rate, channels)
}

#[test]
#[ignore = "needs a recording made with stringplay --record"]
fn the_device_plays_what_the_engine_renders() {
    let rec = PathBuf::from(std::env::var("STRING_RECORDING").expect("STRING_RECORDING"));
    let songs: Vec<PathBuf> = std::env::var("STRING_SONGS")
        .expect("STRING_SONGS")
        .split(';')
        .map(PathBuf::from)
        .collect();
    let volume: f32 = std::env::var("STRING_VOLUME")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1.0);
    let (samples, rate, channels) = read_wav(&rec);
    let heard: Vec<f32> = samples.chunks_exact(channels).map(|f| f[0]).collect();
    let tracks: Vec<Track> = songs
        .iter()
        .enumerate()
        .map(|(i, s)| Track::new(i as u64 + 1, s))
        .collect();
    let (host, _) = ListHost::new(tracks.clone());
    let mut p = OfflinePlayer::new(
        rate,
        Config {
            volume,
            ..Config::default()
        },
        host,
    );
    p.play_track(tracks[0].clone(), 0.0);
    let total: f64 = songs
        .iter()
        .map(|s| {
            let d = string_audio::decode_file(s).unwrap();
            d.samples.len() as f64 / 2.0 / d.rate as f64
        })
        .sum();
    let want = left(&p.render_secs(total + 0.2));
    // where the songs begin in the recording: the first frame with sound, refined by matching
    let onset = |x: &[f32]| x.iter().position(|v| v.abs() > 1e-3).unwrap_or(0);
    let (h0, w0) = (onset(&heard), onset(&want));
    let span = 4_800.min(want.len() - w0);
    let mut best = (0i64, f64::MAX);
    for shift in -200i64..=200 {
        let s = h0 as i64 - w0 as i64 + shift;
        if s < 0 {
            continue;
        }
        let e: f64 = (w0..w0 + span)
            .map(|i| (heard[(s + i as i64) as usize] - want[i]) as f64)
            .map(|d| d * d)
            .sum();
        if e < best.1 {
            best = (s, e);
        }
    }
    let off = best.0 as usize;
    let n = want.len().min(heard.len() - off);
    let peak = want.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    let mut worst = (0usize, 0.0f32);
    for i in 0..n {
        let d = (heard[off + i] - want[i]).abs();
        if d > worst.1 {
            worst = (i, d);
        }
    }
    eprintln!(
        "recording at {rate} Hz: songs start at {off}; over {n} frames the device's sound differs from the engine's by at most {:.6} (the music peaks at {peak:.4}), worst at frame {}",
        worst.1, worst.0
    );
    assert!(
        worst.1 < peak * 0.01 + 1e-4,
        "the device's sound strays at frame {} by {}",
        worst.0,
        worst.1
    );
}
