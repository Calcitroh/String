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
        let missing = Source::open_using(&path, |p, seconds| {
            super::media_foundation::AacSource::open_with_platform(
                p,
                seconds,
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
    let marker = songs.make(
        "next.wav",
        "aevalsrc=0.5:s=44100:d=1",
        &["-c:a", "pcm_s16le"],
    );
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
            Next(crate::Track::new(2, &marker)),
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
        let joint = player.render(2);
        assert_eq!(&joint[..2], &data[data.len() - 2..]);
        assert_eq!(
            &joint[2..],
            &[0.5, 0.5],
            "first successor frame after {path:?}"
        );
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
    assert_eq!(info.frames, Some(pcm.len() as u64 / 2));
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

#[cfg(windows)]
fn lock_middle(file: &std::fs::File) {
    use std::os::windows::io::AsRawHandle;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LockFile(
            handle: *mut std::ffi::c_void,
            lo: u32,
            hi: u32,
            len: u32,
            high_len: u32,
        ) -> i32;
    }
    // A length check which touches the middle of the song gets a sharing error.
    let offset = file.metadata().unwrap().len() / 2;
    assert_ne!(
        unsafe {
            LockFile(
                file.as_raw_handle(),
                offset as u32,
                (offset >> 32) as u32,
                1,
                0,
            )
        },
        0
    );
}

#[cfg(windows)]
#[test]
fn reopening_an_unindexed_mp3_reuses_the_verified_length() {
    let Some(songs) = Songs::new("mp3-reopen") else {
        return;
    };
    let path = std::env::var_os("STRING_TEST_UNINDEXED_MP3")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            songs.make(
                "unindexed.mp3",
                "sine=duration=20:sample_rate=48000",
                &["-c:a", "libmp3lame", "-b:a", "128k", "-write_xing", "0"],
            )
        });
    let file = std::fs::File::open(&path).unwrap();
    let start = std::time::Instant::now();
    let expected = Source::open(&path).unwrap().info().frames.unwrap();
    eprintln!("Unindexed MP3 first open: {:?}", start.elapsed());
    lock_middle(&file);
    let start = std::time::Instant::now();
    assert_eq!(Source::open(&path).unwrap().info().frames, Some(expected));
    eprintln!("Unindexed MP3 reopen: {:?}", start.elapsed());
}

#[cfg(windows)]
#[test]
fn tagged_mp3_open_skips_audio_bodies() {
    let Some(songs) = Songs::new("mp3-io") else {
        return;
    };
    let path = std::env::var_os("STRING_TEST_LONG_MP3")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            songs.make(
                "long.mp3",
                "anullsrc=r=48000:cl=stereo",
                &["-t", "15", "-c:a", "libmp3lame", "-b:a", "128k"],
            )
        });
    let file = std::fs::File::open(&path).unwrap();
    let size = file.metadata().unwrap().len();
    lock_middle(&file);
    for _ in 0..3 {
        let start = std::time::Instant::now();
        let frames = Source::open(&path).unwrap().info().frames.unwrap();
        eprintln!(
            "MP3 open, {size} bytes, {frames} frames: {:?}",
            start.elapsed()
        );
    }
}

#[test]
fn embedded_id3_audio_like_payload_is_inaudible() {
    let Some(songs) = Songs::new("id3-join") else {
        return;
    };
    let part = songs.make(
        "song.mp3",
        "sine=duration=1:sample_rate=44100",
        &["-c:a", "libmp3lame", "-id3v2_version", "0"],
    );
    let art = songs.make(
        "payload.mp3",
        "sine=frequency=800:duration=0.25:sample_rate=48000",
        &[
            "-c:a",
            "libmp3lame",
            "-write_xing",
            "0",
            "-id3v2_version",
            "0",
        ],
    );
    let part = std::fs::read(part).unwrap();
    let payload = std::fs::read(art).unwrap();
    let mut tag = b"PRIV".to_vec();
    tag.extend_from_slice(&((payload.len() + 2) as u32).to_be_bytes());
    tag.extend_from_slice(b"\0\0x\0");
    tag.extend(payload);
    let mut id3 = b"ID3\x03\0\0".to_vec();
    for shift in [21, 14, 7, 0] {
        id3.push(((tag.len() >> shift) & 127) as u8);
    }
    id3.extend(tag);
    let clean = songs.0.join("clean.mp3");
    let tagged = songs.0.join("art.mp3");
    std::fs::write(&clean, [&part[..], &part].concat()).unwrap();
    std::fs::write(&tagged, [&part[..], &id3, &part].concat()).unwrap();
    let (expected, a) = decode(&clean);
    let (heard, b) = decode(&tagged);
    assert_eq!(heard.frames, expected.frames);
    assert!(a == b, "metadata changed the decoded PCM");
}

#[test]
fn webm_successor_starts_on_exact_frame() {
    let Some(songs) = Songs::new("opus-joint") else {
        return;
    };
    let signal = "sine=frequency=413:duration=2.00027:sample_rate=48000";
    let ogg = songs.make("reference.opus", signal, &["-c:a", "libopus"]);
    let (_, reference) = decode(&ogg);
    assert_eq!(reference.len(), 96013 * 2);
    let marker = songs.make(
        "marker.wav",
        "aevalsrc=0.5:s=48000:d=1",
        &["-c:a", "pcm_s16le"],
    );
    struct Next(PathBuf);
    impl crate::Host for Next {
        fn next_after(&mut self, t: &crate::Track) -> Option<crate::Track> {
            (t.key == 1).then(|| crate::Track::new(2, &self.0))
        }
        fn event(&mut self, _: crate::Event) {}
    }
    for live in ["0", "1"] {
        let path = songs.make("audio.webm", signal, &["-c:a", "libopus", "-live", live]);
        // CodecDelay is the Matroska timing field. Some muxers leave OpusHead's
        // redundant pre-skip empty.
        let mut bytes = std::fs::read(&path).unwrap();
        let head = bytes.windows(8).position(|w| w == b"OpusHead").unwrap();
        bytes[head + 10..head + 12].fill(0);
        std::fs::write(&path, bytes).unwrap();
        let mut player = crate::OfflinePlayer::new(48000, Default::default(), Next(marker.clone()));
        player.play_track(crate::Track::new(1, &path), 0.);
        let heard = player.render(96014);
        assert_eq!(
            &heard[192026..],
            &[0.5, 0.5],
            "first successor frame, live={live}"
        );
        assert!(
            heard[..192026] == reference,
            "Opus pre-skip or tail, live={live}"
        );
        assert_eq!(Source::open(&path).unwrap().info().frames, Some(96013));
    }
}

#[test]
fn cueless_webm_seeks_back_from_eof() {
    let Some(songs) = Songs::new("webm-rewind") else {
        return;
    };
    for codec in ["libopus", "libvorbis"] {
        let path = songs.make(
            "live.webm",
            "sine=frequency=337:duration=3:sample_rate=48000",
            &["-c:a", codec, "-live", "1"],
        );
        let mut s = Source::open(&path).unwrap();
        let mut pcm = Vec::new();
        while s.read(&mut pcm, 8192).unwrap() > 0 {}
        for target in [0, 70003, 15007, 0] {
            s.seek(target).unwrap();
            let mut actual = Vec::new();
            assert_eq!(
                s.read(&mut actual, 2048).unwrap(),
                2048,
                "{codec} seek {target}"
            );
            let at = target as usize * 2;
            assert!(actual == pcm[at..at + 4096], "{codec} seek {target}");
        }
    }
}

#[cfg(windows)]
#[test]
fn implicit_he_aac_needs_media_foundation() {
    let Some(songs) = Songs::new("adts-sbr") else {
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
        "implicit.aac",
        "sine=sample_rate=48000:duration=1",
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
    fn missing(
        p: &Path,
        seconds: Option<f64>,
    ) -> Result<super::media_foundation::AacSource, super::Error> {
        super::media_foundation::AacSource::open_with_platform(
            p,
            seconds,
            "absent-test-media-runtime.dll",
        )
    }
    assert!(matches!(
        Source::open_using(&path, missing),
        Err(super::Error::Open(_))
    ));
    let ps = songs.make(
        "implicit-ps.aac",
        "aevalsrc=0.15*sin(2*PI*500*t)|0.15*sin(2*PI*950*t):s=24000:d=1",
        &["-c:a", "aac_mf", "-b:a", "12000"],
    );
    assert!(matches!(
        Source::open_using(&ps, missing),
        Err(super::Error::Open(_))
    ));
    for name in ["lc.m4a", "lc.aac"] {
        let lc = songs.make(
            name,
            "sine=sample_rate=48000:duration=1",
            &["-ac", "2", "-c:a", "aac"],
        );
        let (_, expected) = decode(&lc);
        let mut source = Source::open_using(&lc, missing).unwrap();
        let mut pcm = Vec::new();
        while source.read(&mut pcm, 4096).unwrap() != 0 {}
        assert!(pcm == expected, "LC fallback changed {name}");
    }
}

#[cfg(windows)]
#[test]
fn rewriting_an_unindexed_mp3_invalidates_its_cached_offsets() {
    let Some(songs) = Songs::new("mp3-file-change") else {
        return;
    };
    let opts = [
        "-c:a",
        "libmp3lame",
        "-write_xing",
        "0",
        "-id3v2_version",
        "0",
    ];
    let a = songs.make("a.mp3", "sine=duration=1:sample_rate=48000", &opts);
    let b = songs.make("b.mp3", "sine=duration=2:sample_rate=48000", &opts);
    let bytes = std::fs::read(&b).unwrap();
    let expected = Source::open(&b).unwrap().info().frames;
    let mut old = std::fs::read(&a).unwrap();
    old.resize(bytes.len(), 0);
    std::fs::write(&a, old).unwrap();
    let modified = std::fs::metadata(&a).unwrap().modified().unwrap();
    assert_ne!(Source::open(&a).unwrap().info().frames, expected);
    std::fs::write(&a, bytes).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&a)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    assert_eq!(Source::open(&a).unwrap().info().frames, expected);
}

#[test]
fn truncated_first_mp3_cannot_claim_bytes_from_the_following_segment() {
    let Some(songs) = Songs::new("mp3-short-join") else {
        return;
    };
    let path = songs.make(
        "part.mp3",
        "sine=duration=2:sample_rate=48000",
        &[
            "-ac",
            "2",
            "-c:a",
            "libmp3lame",
            "-b:a",
            "128k",
            "-id3v2_version",
            "0",
        ],
    );
    let part = std::fs::read(&path).unwrap();
    assert_eq!(&part[36..40], b"Info");
    // 128 kb/s at 48 kHz gives 384-byte packets. The first copy loses its end.
    let joined = [&part[..30 * 384], &part[..]].concat();
    std::fs::write(&path, joined).unwrap();
    let (info, pcm) = decode(&path);
    assert_eq!(info.frames, Some(pcm.len() as u64 / 2));
}
