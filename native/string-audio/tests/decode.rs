//! Decoding each format gives back the song's exact length (encoder delay and padding trimmed),
//! and a seek lands on the exact frame.

mod common;

use common::*;
use string_audio::{decode_file, decode_from};

#[test]
fn every_format_decodes_to_its_exact_length() {
    let Some(dir) = gapless_parts() else {
        return skip("no FFmpeg");
    };
    for (suffix, _, exact) in FORMATS {
        let rate = rate_of(suffix);
        let cuts = cuts_of(suffix);
        let whole = signal(rate, cuts[3]);
        let mut joined = Vec::new();
        for n in 0..3 {
            let d = decode_file(&part(dir, suffix, n)).unwrap();
            assert_eq!(d.rate, rate, "{suffix} rate");
            assert_eq!(
                d.samples.len() / 2,
                cuts[n + 1] - cuts[n],
                "{suffix} part {n}: decoded length"
            );
            joined.extend_from_slice(&d.samples);
        }
        let worst = max_diff(&joined, &whole, 0, cuts[3]);
        let mid = rms(&joined
            .iter()
            .zip(&whole)
            .map(|(a, b)| a - b)
            .collect::<Vec<_>>());
        eprintln!("{suffix}: worst difference {worst:.5}, rms difference {mid:.6}");
        if exact {
            assert!(worst < 1e-6, "{suffix}: lossless parts differ by {worst}");
        } else {
            assert!(
                mid < 0.01,
                "{suffix}: parts sit wrong (rms difference {mid})"
            );
        }
    }
}

#[test]
fn seeks_land_on_the_exact_frame() {
    let Some(dir) = gapless_parts() else {
        return skip("no FFmpeg");
    };
    for (suffix, _, exact) in FORMATS {
        let rate = rate_of(suffix);
        let cuts = cuts_of(suffix);
        let whole = signal(rate, cuts[3]);
        let path = part(dir, suffix, 1);
        let full = decode_file(&path).unwrap();
        for target in [0usize, 1, 4_410, 12_345, 33_333] {
            let d = decode_from(&path, target as u64, 2_048).unwrap();
            assert_eq!(d.samples.len() / 2, 2_048, "{suffix} from {target}");
            // against the song decoded straight through, and against the source signal
            let vs_full = max_diff(&d.samples, &full.samples[target * 2..], 0, 2_048);
            let src = &whole[(cuts[1] + target) * 2..];
            let vs_src = rms(&d
                .samples
                .iter()
                .zip(src)
                .map(|(a, b)| a - b)
                .collect::<Vec<_>>());
            eprintln!(
                "{suffix} seek {target}: vs straight {vs_full:.6}, rms vs source {vs_src:.6}"
            );
            // a lossy decoder restarted before the target settles to (nearly) the same samples;
            // the source comparison only shows the encoder's own error near the song's start
            let limit = if exact { 1e-6 } else { 0.02 };
            assert!(
                vs_full < limit,
                "{suffix}: seek to {target} is off ({vs_full})"
            );
            assert!(
                vs_src < 0.05,
                "{suffix}: seek to {target} is off (rms {vs_src} vs source)"
            );
        }
    }
}
