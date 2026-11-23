//! Generated regression songs. Set FFMPEG to the encoder executable on machines without it on PATH.

use super::{Info, Source};
use std::path::{Path, PathBuf};
use std::process::Command;

struct Songs(PathBuf);

impl Songs {
    fn new(name: &str) -> Option<Self> {
        if ffmpeg().arg("-version").output().is_err() {
            eprintln!("Skipping generated {name} songs (FFmpeg unavailable)");
            return None;
        }
        let dir = std::env::temp_dir().join(format!("string-audio-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Some(Self(dir))
    }

    fn make(&self, name: &str, signal: &str, options: &[&str]) -> PathBuf {
        let path = self.0.join(name);
        let result = ffmpeg()
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", signal])
            .args(options)
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        path
    }
}

impl Drop for Songs {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn ffmpeg() -> Command {
    Command::new(std::env::var_os("FFMPEG").unwrap_or_else(|| "ffmpeg".into()))
}

fn decode(path: &Path) -> (Info, Vec<f32>) {
    let mut source = Source::open(path).unwrap();
    let info = source.info();
    let mut data = Vec::new();
    while source.read(&mut data, 8192).unwrap() != 0 {}
    (info, data)
}

#[test]
fn webm_and_weba_opus_and_vorbis_play_with_length() {
    let Some(songs) = Songs::new("webm") else {
        return;
    };
    for ext in ["webm", "weba"] {
        for codec in ["libopus", "libvorbis"] {
            let path = songs.make(
                &format!("{codec}.{ext}"),
                "sine=frequency=341:duration=2:sample_rate=48000",
                &["-c:a", codec, "-f", "webm"],
            );
            let (info, data) = decode(&path);
            assert_eq!(info.rate, 48000);
            assert!(info.frames.unwrap().abs_diff(96000) < 1200);
            assert!((data.len() / 2).abs_diff(96000) < 1200);
            assert!(data[170000..].iter().any(|x| x.abs() > 0.03));
        }
    }
}

#[cfg(windows)]
#[test]
fn surround_aac_mp4_and_adts_render_the_center() {
    let Some(songs) = Songs::new("surround") else {
        return;
    };
    for ext in ["m4a", "aac"] {
        let path = songs.make(
            &format!("center.{ext}"),
            "aevalsrc=0|0|0.1*sin(2*PI*480*t)|0|0|0:s=48000:d=2:c=5.1",
            &["-c:a", "aac", "-b:a", "320k"],
        );
        let missing = Source::open_using(&path, |p| {
            super::media_foundation::AacSource::open_with_platform(
                p,
                "absent-test-media-runtime.dll",
            )
        });
        assert!(matches!(missing, Err(super::Error::Open(_))));
        let mut moved = Source::open(&path).unwrap();
        std::thread::spawn(move || assert!(moved.read(&mut Vec::new(), 32768).unwrap() > 0))
            .join()
            .unwrap();
        let (info, data) = decode(&path);
        assert_eq!(info.rate, 48000);
        let frames = data.len() / 2;
        assert!(frames.abs_diff(96000) <= 2048, "{ext}: {frames}");
        assert!(info.frames.unwrap().abs_diff(frames as u64) <= 1024);
        let tail = &data[100000..144000];
        let peak = tail.iter().copied().fold(0.0f32, f32::max);
        assert!((0.064..0.077).contains(&peak), "{ext}: {peak}");
        assert!(tail.chunks_exact(2).all(|lr| (lr[0] - lr[1]).abs() < 0.002));
        let at = crate::decode_from(&path, 60000, 2048).unwrap();
        assert_eq!(at.samples.len(), 4096);
        assert!(
            at.samples
                .iter()
                .zip(&data[120000..])
                .all(|(s, t)| (s - t).abs() < 0.003),
            "{ext} seek"
        );
    }
}

#[test]
fn mp3_end_and_tail_seek_follow_the_data() {
    let Some(songs) = Songs::new("mp3-end") else {
        return;
    };
    let source = "aevalsrc=if(lt(t\\,1)\\,0.001\\,0.3)*sin(2*PI*413*t):s=44100:d=3";
    let tagged = songs.make("tagged.mp3", source, &["-c:a", "libmp3lame", "-q:a", "2"]);
    let headerless = songs.make(
        "vbr.mp3",
        source,
        &["-c:a", "libmp3lame", "-q:a", "2", "-write_xing", "0"],
    );
    let joined = songs.0.join("joined.mp3");
    let part = std::fs::read(&tagged).unwrap();
    std::fs::write(&joined, [part.as_slice(), part.as_slice()].concat()).unwrap();
    for (path, seconds) in [(joined, 6), (headerless, 3), (tagged, 3)] {
        let (info, data) = decode(&path);
        let frames = data.len() / 2;
        assert!(frames.abs_diff(seconds * 44100) < 4096, "{path:?} {frames}");
        assert_eq!(info.frames, Some(frames as u64));
        // Exercise the same deck and published duration used by the app. The following
        // track must start on the first frame after the physical end.
        struct Next(crate::Track);
        impl crate::Host for Next {
            fn next_after(&mut self, current: &crate::Track) -> Option<crate::Track> {
                (current.key == 1).then(|| self.0.clone())
            }
            fn event(&mut self, _: crate::Event) {}
        }
        let mut player = crate::OfflinePlayer::new(
            info.rate,
            Default::default(),
            Next(crate::Track::new(2, &path)),
        );
        player.play_track(crate::Track::new(1, &path), 0.);
        let heard = player.render(frames - 1);
        assert_eq!(
            player.status().duration,
            Some(frames as f64 / f64::from(info.rate))
        );
        assert_eq!(player.status().key, Some(1));
        assert!(
            heard == data[..heard.len()],
            "engine changed PCM before end of {path:?}"
        );
        player.render(2);
        assert_eq!(player.status().key, Some(2));
        let at = (seconds * 44100 - 8000) as u64;
        let tail = crate::decode_from(&path, at, 2048).unwrap();
        assert_eq!(tail.samples.len(), 4096);
        assert!(
            tail.samples.iter().any(|s| s.abs() > 0.05),
            "{path:?} quiet seek, full peak {}",
            data[at as usize * 2..][..4096]
                .iter()
                .copied()
                .fold(0.0f32, f32::max)
        );
        assert!(
            tail.samples
                .iter()
                .zip(&data[at as usize * 2..])
                .all(|(s, t)| (s - t).abs() < 0.005),
            "{path:?} tail seek"
        );
    }
}

#[cfg(windows)]
#[test]
fn he_aac_plays_at_the_extension_sample_rate() {
    let Some(songs) = Songs::new("he-aac") else {
        return;
    };
    let encoders = ffmpeg()
        .args(["-hide_banner", "-encoders"])
        .output()
        .unwrap();
    if !String::from_utf8_lossy(&encoders.stdout).contains("aac_mf") {
        return;
    }
    for name in ["sbr.m4a", "sbr.aac"] {
        let path = songs.make(
            name,
            "sine=frequency=2300:sample_rate=48000:duration=2",
            &[
                "-ac",
                "2",
                "-c:a",
                "aac_mf",
                "-profile:a",
                "4",
                "-b:a",
                "48000",
            ],
        );
        let (info, data) = decode(&path);
        assert_eq!(info.rate, 48000);
        assert!((data.len() / 2).abs_diff(96000) <= 4096);
        assert!(data[140000..].iter().any(|s| s.abs() > 0.02));
    }
}

#[test]
fn surround_speakers_fold_to_the_correct_side() {
    use symphonia::core::audio::{Channels, Position};
    let speakers = Channels::Positioned(Position::from_bits_truncate(0x3f));
    let mix = super::Downmix::new(Some(&speakers), 6);
    let h = std::f32::consts::FRAC_1_SQRT_2;
    for (i, expected) in [[1., 0.], [0., 1.], [h, h], [0., 0.], [h, 0.], [0., h]]
        .into_iter()
        .enumerate()
    {
        let mut frame = [0.; 6];
        frame[i] = 1.;
        let mut stereo = Vec::new();
        mix.apply(&frame, 0, 1, &mut stereo);
        assert_eq!(stereo, expected);
    }
}

#[test]
fn joined_mp3_with_a_new_sample_rate_and_channel_layout_keeps_its_tail() {
    let Some(songs) = Songs::new("mp3-rates") else {
        return;
    };
    let left = songs.make(
        "mono.mp3",
        "sine=frequency=320:duration=2:sample_rate=44100",
        &["-ac", "1", "-c:a", "libmp3lame"],
    );
    let right = songs.make(
        "stereo.mp3",
        "aevalsrc=0.15*sin(2*PI*900*t)|0.15*sin(2*PI*700*t):s=48000:d=2",
        &["-c:a", "libmp3lame"],
    );
    let joined = songs.0.join("mixed.mp3");
    std::fs::write(
        &joined,
        [std::fs::read(left).unwrap(), std::fs::read(right).unwrap()].concat(),
    )
    .unwrap();
    let (info, data) = decode(&joined);
    assert_eq!(info.rate, 44100);
    assert!((data.len() / 2).abs_diff(4 * 44100) < 4096);
    assert_eq!(info.frames, Some(data.len() as u64 / 2));
    assert!(
        data[300000..]
            .chunks_exact(2)
            .any(|lr| (lr[0] - lr[1]).abs() > 0.1)
    );
    let tail = crate::decode_from(&joined, 150000, 2048).unwrap();
    assert_eq!(tail.samples, &data[300000..304096]);
}

#[test]
fn webm_recorded_without_a_duration_opens_with_its_length() {
    let Some(songs) = Songs::new("webm-live") else {
        return;
    };
    for codec in ["libopus", "libvorbis"] {
        let path = songs.make(
            &format!("{codec}.webm"),
            "sine=duration=2:sample_rate=48000",
            &["-c:a", codec, "-live", "1", "-f", "webm"],
        );
        let (info, pcm) = decode(&path);
        assert!(info.frames.unwrap().abs_diff(pcm.len() as u64 / 2) < 1200);
    }
}

#[cfg(windows)]
#[test]
fn he_aac_v2_plays_both_parametric_stereo_channels() {
    let Some(songs) = Songs::new("aac-ps") else {
        return;
    };
    let encoders = ffmpeg()
        .args(["-hide_banner", "-encoders"])
        .output()
        .unwrap();
    if !String::from_utf8_lossy(&encoders.stdout).contains("aac_mf") {
        return;
    }
    let path = songs.make(
        "ps.m4a",
        "aevalsrc=0.15*sin(2*PI*500*t)|0.15*sin(2*PI*950*t):s=24000:d=2",
        &["-c:a", "aac_mf", "-b:a", "12000"],
    );
    let (info, pcm) = decode(&path);
    assert_eq!(info.rate, 24000);
    assert!((pcm.len() / 2).abs_diff(48000) < 4096);
    assert!(
        pcm[40000..80000]
            .chunks_exact(2)
            .any(|lr| (lr[0] - lr[1]).abs() > 0.05)
    );
}

#[test]
fn mp3_byte_count_alone_cannot_validate_a_stale_frame_count() {
    let Some(songs) = Songs::new("mp3-stale") else {
        return;
    };
    let path = songs.make(
        "stale.mp3",
        "sine=duration=3:sample_rate=44100",
        &["-c:a", "libmp3lame", "-q:a", "2"],
    );
    let mut bytes = std::fs::read(&path).unwrap();
    let at = bytes.windows(4).position(|sig| sig == b"Xing").unwrap() + 8;
    let frames = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
    bytes[at..at + 4].copy_from_slice(&(frames / 2).to_be_bytes());
    std::fs::write(&path, bytes).unwrap();
    let (info, samples) = decode(&path);
    assert!(samples.len() / 2 >= 3 * 44100);
    assert_eq!(info.frames, Some(samples.len() as u64 / 2));
}
