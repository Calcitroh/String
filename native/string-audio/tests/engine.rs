//! The engine rendered offline: songs play whole, fades have their shapes, hand-overs land on
//! the exact frame, loops wrap on time, seeks land where asked, and resampling keeps time.

mod common;

use common::*;
use string_audio::{Config, Event, LoopRange, OfflinePlayer, Track};

const SR: u32 = 48_000;

fn player(
    rate: u32,
    config: Config,
    tracks: Vec<Track>,
) -> (OfflinePlayer, std::sync::Arc<std::sync::Mutex<Vec<Event>>>) {
    let (host, events) = ListHost::new(tracks);
    (OfflinePlayer::new(rate, config, host), events)
}

fn names(events: &std::sync::Arc<std::sync::Mutex<Vec<Event>>>) -> Vec<String> {
    events
        .lock()
        .unwrap()
        .iter()
        .map(|e| match e {
            Event::Started { track } => format!("started {}", track.key),
            Event::Ended { track } => format!("ended {}", track.key),
            Event::NearEnd { track, .. } => format!("near end {}", track.key),
            Event::Failed { track, .. } => format!("failed {}", track.key),
            Event::Stopped => "stopped".into(),
            Event::Output { .. } => "output".into(),
        })
        .collect()
}

#[test]
fn a_song_plays_whole_then_stops() {
    let dir = scratch("whole");
    let song = dir.join("a.wav");
    write_wav(&song, SR, &numbered(SR as usize * 2));
    let t = Track::new(1, &song);
    let (mut p, events) = player(SR, Config::default(), vec![t.clone()]);
    p.play_track(t, 0.0);
    let out = p.render_secs(3.0);
    for (i, f) in out
        .as_chunks::<2>()
        .0
        .iter()
        .take(SR as usize * 2)
        .enumerate()
    {
        assert_eq!(number_of(f[0], f[1]), Some(i), "frame {i}");
    }
    assert_eq!(
        sound_ends(&out, 0),
        SR as usize * 2,
        "nothing more, nothing less"
    );
    assert_eq!(
        names(&events),
        ["started 1", "near end 1", "ended 1", "stopped"]
    );
    assert!(!p.status().playing);
}

#[test]
fn pause_and_play_fade_like_the_js_player() {
    let dir = scratch("fades");
    let song = dir.join("dc.wav");
    write_wav(&song, SR, &level(0.5, SR as usize * 10));
    let t = Track::new(1, &song);
    let (mut p, _) = player(SR, Config::default(), vec![t.clone()]);
    p.play_track(t, 0.0);
    let start = p.render_secs(1.0);
    assert!(
        start.iter().all(|v| (*v - 0.5).abs() < 1e-4),
        "starts at full level, no fade on a song's start"
    );
    p.pause();
    let fall = p.render_secs(0.5);
    // a straight line from 0.5 to 0 over 0.16 s, then silence
    let len = (0.16 * SR as f64) as usize;
    for i in [0, len / 4, len / 2, 3 * len / 4] {
        let want = 0.5 * (1.0 - i as f32 / len as f32);
        assert!(
            (fall[i * 2] - want).abs() < 2e-3,
            "pause fade at {i}: {} vs {want}",
            fall[i * 2]
        );
    }
    assert!(
        fall[len * 2..].iter().all(|v| *v == 0.0),
        "silent after the fade"
    );
    let paused_at = p.status().position;
    p.render_secs(1.0);
    assert_eq!(
        p.status().position,
        paused_at,
        "time stands still while paused"
    );
    p.play();
    let rise = p.render_secs(0.5);
    let len = (0.22 * SR as f64) as usize;
    for i in [0, len / 4, len / 2, 3 * len / 4] {
        let want = 0.5 * i as f32 / len as f32;
        assert!(
            (rise[i * 2] - want).abs() < 2e-3,
            "play fade at {i}: {} vs {want}",
            rise[i * 2]
        );
    }
    assert!((rise[len * 2 + 10] - 0.5).abs() < 1e-4);
}

#[test]
fn volume_follows_its_square_smoothly() {
    let dir = scratch("volume");
    let song = dir.join("dc.wav");
    write_wav(&song, SR, &level(0.5, SR as usize * 4));
    let t = Track::new(1, &song);
    let (mut p, _) = player(SR, Config::default(), vec![t.clone()]);
    p.play_track(t, 0.0);
    p.render_secs(0.5);
    p.set_volume(0.5);
    let out = p.render_secs(0.5);
    // no jump: the first frame barely moved, and after 0.3 s (ten time constants) it's there
    assert!(out[0] > 0.49, "smooth: {}", out[0]);
    assert!((out[(0.3 * SR as f64) as usize * 2] - 0.125).abs() < 1e-3);
    p.set_muted(true);
    let out = p.render_secs(0.5);
    assert!(out[out.len() - 2].abs() < 1e-4);
}

#[test]
fn the_first_song_starts_at_the_set_volume() {
    let dir = scratch("volume-start");
    let song = dir.join("dc.wav");
    write_wav(&song, SR, &level(0.5, SR as usize));
    let t = Track::new(1, &song);
    let (mut p, _) = player(
        SR,
        Config {
            volume: 0.5,
            ..Config::default()
        },
        vec![t.clone()],
    );
    p.play_track(t, 0.0);
    let out = p.render_secs(0.1);
    assert!(
        (out[0] - 0.125).abs() < 1e-4,
        "starts at 0.5 squared: {}",
        out[0]
    );
}

#[test]
fn wav_songs_hand_over_without_a_gap() {
    let dir = scratch("gapless-wav");
    let whole = signal(SR, SR as usize * 3);
    let cuts = [0, 51_234, 99_999, SR as usize * 3];
    let mut tracks = Vec::new();
    for i in 0..3 {
        let path = dir.join(format!("{i}.wav"));
        write_wav(&path, SR, &whole[cuts[i] * 2..cuts[i + 1] * 2]);
        tracks.push(Track::new(i as u64 + 1, path));
    }
    let (mut p, events) = player(SR, Config::default(), tracks.clone());
    p.play_track(tracks[0].clone(), 0.0);
    let out = p.render_secs(4.0);
    assert_eq!(
        max_diff(&out, &whole, 0, cuts[3]),
        0.0,
        "the three songs join into the whole, frame for frame"
    );
    assert_eq!(sound_ends(&out, 0), cuts[3]);
    let names = names(&events);
    assert_eq!(
        names
            .iter()
            .filter(|n| n.starts_with("started") || n.starts_with("ended"))
            .collect::<Vec<_>>(),
        [
            "started 1",
            "ended 1",
            "started 2",
            "ended 2",
            "started 3",
            "ended 3"
        ]
    );
}

/// Plays the three-part gapless fixtures of one format at `sink`: the songs must join with no
/// gap, no overlap and no click of the engine's making.
fn gapless_format(suffix: &str, exact: bool, sink: u32) {
    let dir = gapless_parts().unwrap();
    let src = rate_of(suffix);
    let cuts = cuts_of(suffix);
    let tracks: Vec<Track> = (0..3)
        .map(|i| Track::new(i as u64 + 1, part(dir, suffix, i)))
        .collect();
    let (mut p, events) = player(sink, Config::default(), tracks.clone());
    p.play_track(tracks[0].clone(), 0.0);
    let total = (cuts[3] as u64 * sink as u64 / src as u64) as usize;
    let out = p.render(total + sink as usize / 2);
    let at = |c: usize| (c as u64 * sink as u64 / src as u64) as usize;
    // the last song ends on its frame; only the resampler's faint ringing follows
    let end = sound_ends(&out, 0);
    let ring = rms(&out[total * 2..(total + RINGING) * 2]);
    assert!(
        end >= total - 2 && end <= total + RINGING,
        "{suffix} at {sink}: {end} frames, want {total}"
    );
    assert!(
        ring < 0.02,
        "{suffix} at {sink}: loud after the end ({ring})"
    );
    // what the files hold, decoded one by one and laid end to end
    let mut parts = Vec::new();
    for i in 0..3 {
        parts.extend(
            string_audio::decode_file(&part(dir, suffix, i))
                .unwrap()
                .samples,
        );
    }
    let source = signal(sink, total);
    let edge = sink as usize / 100;
    let lo = left(&out);
    let err: Vec<f32> = lo.iter().zip(left(&source)).map(|(a, b)| a - b).collect();
    let along = rms(&err[edge..total - edge]);
    for c in [cuts[1], cuts[2]] {
        let j = at(c);
        let near = rms(&err[j - edge / 2..j + edge / 2]);
        let rough = roughness(&lo, j - edge / 2, j + edge / 2);
        // the files' own joins, at their own rate
        let pl = left(&parts);
        let src_left = left(&signal(src, cuts[3]));
        let theirs: Vec<f32> = pl.iter().zip(&src_left).map(|(a, b)| a - b).collect();
        let e2 = src as usize / 200;
        let file_near = rms(&theirs[c - e2..c + e2]);
        eprintln!(
            "{suffix} at {sink}: join {j}: error near {near:.5} (along {along:.5}, the files' own {file_near:.5}), roughness {rough:.5}"
        );
        if exact {
            assert!(near < 2e-3, "{suffix}: the join at {j} is off ({near})");
            let base = roughness(&left(&source), edge, total - edge);
            assert!(
                rough < base * 1.2 + 1e-3,
                "{suffix}: a click at the join {j} ({rough} vs {base})"
            );
        } else {
            // a lossy codec can't make a hard cut perfectly; the engine adds nothing to that
            assert!(
                near < file_near * 1.2 + 2e-3,
                "{suffix}: the join at {j} is worse than the files' ({near} vs {file_near})"
            );
        }
    }
    if sink == src {
        // at the files' own rate the engine plays exactly what they hold, frame for frame
        let worst = max_diff(&out, &parts, 0, total);
        assert!(
            worst < 1e-6,
            "{suffix}: differs from the files laid end to end ({worst})"
        );
    }
    assert!(
        along < if exact { 2e-3 } else { 0.02 },
        "{suffix}: drifts ({along})"
    );
    let started = names(&events)
        .into_iter()
        .filter(|n| n.starts_with("started"))
        .count();
    assert_eq!(started, 3);
}

#[test]
fn ffmpeg_songs_hand_over_without_a_gap_or_click() {
    if gapless_parts().is_none() {
        return skip("no FFmpeg");
    }
    for (suffix, _, exact) in FORMATS {
        // at their own rate, and resampled
        gapless_format(suffix, exact, rate_of(suffix));
        gapless_format(
            suffix,
            exact,
            if rate_of(suffix) == 48_000 {
                44_100
            } else {
                48_000
            },
        );
    }
}

#[test]
fn crossfade_is_equal_power_and_ends_on_the_last_frame() {
    let dir = scratch("crossfade");
    let (a, b) = (dir.join("a.wav"), dir.join("b.wav"));
    write_wav(&a, SR, &level(0.5, SR as usize * 10));
    write_wav(&b, SR, &level(0.25, SR as usize * 10));
    let tracks = vec![Track::new(1, &a), Track::new(2, &b)];
    let config = Config {
        crossfade: 2.0,
        ..Config::default()
    };
    let (mut p, events) = player(SR, config, tracks.clone());
    p.play_track(tracks[0].clone(), 0.0);
    let out = p.render_secs(19.0);
    let start = SR as usize * 8;
    let len = SR as usize * 2;
    assert!(
        (out[(start - 1) * 2] - 0.5).abs() < 1e-4,
        "only the first song before"
    );
    for k in 0..=20 {
        let i = start + k * len / 20;
        let x = (i - start) as f32 / len as f32;
        let (fo, fi) = (
            (x * std::f32::consts::FRAC_PI_2).cos(),
            (x * std::f32::consts::FRAC_PI_2).sin(),
        );
        let want = if k == 20 { 0.25 } else { 0.5 * fo + 0.25 * fi };
        assert!(
            (out[i * 2] - want).abs() < 2e-3,
            "crossfade at {x:.2}: {} vs {want}",
            out[i * 2]
        );
    }
    // the second song plays its whole 10 s from the crossfade's start
    assert_eq!(sound_ends(&out, 0), start + SR as usize * 10);
    let names = names(&events);
    let order: Vec<_> = names
        .iter()
        .filter(|n| n.starts_with("started") || n.starts_with("ended"))
        .collect();
    assert_eq!(order, ["started 1", "started 2", "ended 1", "ended 2"]);
}

#[test]
fn short_songs_and_repeats_dont_crossfade() {
    let dir = scratch("crossfade-short");
    let a = dir.join("a.wav");
    write_wav(&a, SR, &numbered(SR as usize * 6));
    // the same place twice (Repeat one): gapless, never crossfaded
    let one = Track::new(7, &a);
    let tracks = vec![one.clone(), one.clone()];
    let config = Config {
        crossfade: 2.0,
        ..Config::default()
    };
    let (mut p, _) = player(SR, config, tracks);
    p.play_track(one, 0.0);
    let out = p.render_secs(12.5);
    for (i, f) in out
        .as_chunks::<2>()
        .0
        .iter()
        .take(SR as usize * 12)
        .enumerate()
    {
        assert_eq!(
            number_of(f[0], f[1]),
            Some(i % (SR as usize * 6)),
            "frame {i}"
        );
    }
}

#[test]
fn loops_wrap_on_the_exact_frame_with_the_js_dip() {
    let dir = scratch("loop");
    let song = dir.join("n.wav");
    write_wav(&song, SR, &numbered(SR as usize * 10));
    let lp = LoopRange {
        start: 1.0,
        end: Some(3.0),
    };
    let t = Track::new(1, &song).with_loop(Some(lp));
    let (mut p, events) = player(SR, Config::default(), vec![t.clone()]);
    p.play_track(t, 0.0);
    let out = p.render_secs(9.5);
    let (a, b) = (SR as usize, SR as usize * 3);
    let (dip_out, dip_in) = ((0.04 * SR as f64) as usize, (0.08 * SR as f64) as usize);
    // first pass: from 0 straight to the dip before b
    for i in 0..b - dip_out {
        assert_eq!(number_of(out[i * 2], out[i * 2 + 1]), Some(i), "frame {i}");
    }
    // each wrap: out over 40 ms to b, then from a, in over 80 ms
    let lap = b - a;
    for k in 0..3 {
        let jump = b + k * lap;
        // the dip out falls in a straight line on the frames just before b
        let mid = jump - dip_out / 2;
        let src = b - dip_out / 2;
        let want = numbered(src + 1)[src * 2] * 0.5;
        assert!(
            (out[mid * 2] - want).abs() < 2e-3,
            "lap {k}: dip out {} vs {want}",
            out[mid * 2]
        );
        // after the dip in, frame for frame from a on
        for i in jump + dip_in..jump + lap - dip_out {
            assert_eq!(
                number_of(out[i * 2], out[i * 2 + 1]),
                Some(a + i - jump),
                "lap {k}, frame {i}"
            );
        }
    }
    assert!(
        !names(&events)
            .iter()
            .any(|n| n.starts_with("ended") || n.starts_with("near")),
        "a looped song never ends"
    );
}

#[test]
fn a_loop_set_and_cleared_while_playing() {
    let dir = scratch("loop-live");
    let song = dir.join("n.wav");
    write_wav(&song, SR, &numbered(SR as usize * 10));
    let t = Track::new(1, &song);
    let (mut p, _) = player(SR, Config::default(), vec![t.clone()]);
    p.play_track(t, 0.0);
    p.render_secs(2.5);
    // set past its end: jumps to the start at once, after the dip
    p.set_loop(Some(LoopRange {
        start: 0.5,
        end: Some(2.0),
    }));
    let out = p.render_secs(1.0);
    let settle = (0.04 * SR as f64) as usize + (0.08 * SR as f64) as usize + 1_000;
    let n = number_of(out[settle * 2], out[settle * 2 + 1]).unwrap();
    assert!(
        n >= SR as usize / 2 && n < SR as usize,
        "jumped to the loop's start: {n}"
    );
    // cleared: plays on through the old end
    p.set_loop(None);
    let out = p.render_secs(3.0);
    let last = out
        .as_chunks::<2>()
        .0
        .iter()
        .rev()
        .find_map(|f| number_of(f[0], f[1]))
        .unwrap();
    assert!(
        last > SR as usize * 3,
        "went on past the loop's end: {last}"
    );
}

#[test]
fn seeks_land_on_the_frame_asked_for() {
    let dir = scratch("seek");
    let song = dir.join("n.wav");
    write_wav(&song, SR, &numbered(SR as usize * 20));
    let t = Track::new(1, &song);
    let (mut p, _) = player(SR, Config::default(), vec![t.clone()]);
    p.play_track(t, 0.0);
    p.render_secs(1.0);
    for target in [12.3456, 2.0001, 7.5] {
        p.seek(target);
        let out = p.render_secs(1.0);
        let (fo, fi) = ((0.06 * SR as f64) as usize, (0.16 * SR as f64) as usize);
        let first = (target * SR as f64) as usize;
        // after the fade out (60 ms) and at most one engine round of waiting for the decoder,
        // the target comes in over 160 ms, then plays on frame by frame
        let mut seen = 0;
        let mut start = None;
        for i in fo + fi + 600..out.len() / 2 {
            let n = number_of(out[i * 2], out[i * 2 + 1]).expect("full level after the fades");
            let s = *start.get_or_insert(i - (n - first));
            assert_eq!(n, first + (i - s), "seek to {target}: frame {i}");
            seen += 1;
        }
        assert!(seen > 30_000);
        let s = start.unwrap();
        assert!(
            s >= fo && s <= fo + 600,
            "seek to {target}: came in {} frames after the fade out",
            s - fo
        );
        // the fade in is a straight line
        let i = s + fi / 2;
        let want = numbered(first + fi / 2 + 1)[(first + fi / 2) * 2] * 0.5;
        assert!(
            (out[i * 2] - want).abs() < 2e-3,
            "fade in halfway: {} vs {want}",
            out[i * 2]
        );
    }
    // paused: the seek is at once, and play fades in from there
    p.pause();
    p.render_secs(0.5);
    p.seek(4.0);
    p.render_secs(0.2);
    assert!(
        (p.status().position - 4.0).abs() < 1e-6,
        "paused seek: {}",
        p.status().position
    );
    p.play();
    let out = p.render_secs(0.5);
    let i = (0.3 * SR as f64) as usize;
    assert_eq!(
        number_of(out[i * 2], out[i * 2 + 1]),
        Some(SR as usize * 4 + i)
    );
}

#[test]
fn resampling_keeps_pitch_and_length() {
    let dir = scratch("resample");
    let song = dir.join("t.wav");
    write_wav(&song, 44_100, &tone(44_100, 1000.0, 0.5, 44_100 * 2));
    let t = Track::new(1, &song);
    let (mut p, _) = player(SR, Config::default(), vec![t.clone()]);
    p.play_track(t, 0.0);
    let out = p.render_secs(2.5);
    let end = sound_ends(&out, 0);
    assert!(
        end >= SR as usize * 2 - 2 && end <= SR as usize * 2 + RINGING,
        "2 s at 44.1 kHz is 2 s at 48 kHz: {end}"
    );
    let want = tone(SR, 1000.0, 0.5, SR as usize * 2);
    let err = max_diff(&out, &want, 1_000, SR as usize * 2 - 1_000);
    assert!(
        err < 2e-3,
        "a 1 kHz tone stays a 1 kHz tone in phase: {err}"
    );
}

#[test]
fn the_spectrum_shows_the_music() {
    let dir = scratch("spectrum");
    let song = dir.join("t.wav");
    write_wav(&song, SR, &tone(SR, 440.0, 0.5, SR as usize * 3));
    let t = Track::new(1, &song);
    let config = Config {
        spectrum: true,
        ..Config::default()
    };
    let (mut p, _) = player(SR, config, vec![t.clone()]);
    let spec = p.spectrum();
    p.play_track(t, 0.0);
    p.render_secs(1.0);
    let mut bands = [0.0; string_audio::BANDS];
    assert!(spec.read(&mut bands) > 200, "about 280 frames a second");
    let loudest = (0..bands.len())
        .max_by(|a, b| bands[*a].total_cmp(&bands[*b]))
        .unwrap();
    let want = (64.0 * (440.0f64 / 35.0).ln() / (15000.0f64 / 35.0).ln()) as usize;
    assert!(
        loudest.abs_diff(want) <= 1,
        "440 Hz lights band {loudest}, want {want}"
    );
    // muted, the picture goes on (it's measured before the volume)
    p.set_muted(true);
    p.render_secs(0.5);
    spec.read(&mut bands);
    assert!(bands[loudest] > 0.5);
}

#[test]
fn a_broken_song_is_skipped() {
    let dir = scratch("broken");
    let good = dir.join("good.wav");
    write_wav(&good, SR, &level(0.5, SR as usize));
    let bad = dir.join("bad.mp3");
    std::fs::write(&bad, b"not a song at all").unwrap();
    let tracks = vec![Track::new(1, &bad), Track::new(2, &good)];
    let (mut p, events) = player(SR, Config::default(), tracks.clone());
    p.play_track(tracks[0].clone(), 0.0);
    let out = p.render_secs(1.5);
    assert!((out[1_000] - 0.5).abs() < 1e-4, "the next song plays");
    let names = names(&events);
    assert_eq!(names[0], "failed 1");
    assert_eq!(names[1], "started 2");
}

#[test]
fn clicking_missing_songs_cuts_the_previous_song_and_reports_the_stop() {
    let dir = scratch("failed-click");
    let good = Track::new(1, dir.join("good.wav"));
    write_wav(&good.path, SR, &level(0.5, SR as usize * 3));
    for count in [1, 6] {
        let mut list = vec![good.clone()];
        list.extend((2..2 + count).map(|key| Track::new(key, dir.join(format!("{key}.wav")))));
        if count == 6 {
            list.push(Track::new(10, &good.path));
        }
        let (mut p, events) = player(
            SR,
            Config {
                ask_ahead: 0.0,
                ..Config::default()
            },
            list.clone(),
        );
        p.play_track(good.clone(), 0.);
        p.render_secs(0.25);
        events.lock().unwrap().clear();
        p.play_track(list[1].clone(), 0.);
        assert!(
            p.render_secs(0.5).iter().all(|&s| s == 0.0),
            "old song still audible"
        );
        assert!(!p.status().playing && p.status().key.is_none());
        let mut want: Vec<_> = (2..2 + count.min(5))
            .map(|key| format!("failed {key}"))
            .collect();
        want.push("stopped".into());
        assert_eq!(names(&events), want);
        p.play();
        assert!(p.render_secs(0.1).iter().all(|&s| s == 0.0));
    }
}

#[test]
fn transport_during_a_crossfade_releases_the_outgoing_song() {
    let dir = scratch("crossfade-transport");
    let tracks: Vec<_> = [0.5, 0.25, 0.125]
        .into_iter()
        .enumerate()
        .map(|(i, v)| {
            let path = dir.join(format!("{i}.wav"));
            write_wav(&path, SR, &level(v, SR as usize * 15));
            Track::new(i as u64, path)
        })
        .collect();
    for pause in [false, true] {
        let (mut p, _) = player(
            SR,
            Config {
                crossfade: 4.0,
                ..Config::default()
            },
            tracks.clone(),
        );
        p.play_track(tracks[0].clone(), 10.);
        p.render_secs(2.);
        assert_eq!(p.status().key, Some(1));
        if pause {
            p.pause();
            let fall = p.render_secs(0.25);
            assert!((fall[0] - 0.25).abs() < 1e-4, "pause began at {}", fall[0]);
            p.play();
        } else {
            p.seek(4.);
        }
        let out = p.render_secs(0.5);
        assert!(
            (out[out.len() - 1] - 0.25).abs() < 1e-4,
            "outgoing song still audible"
        );
        p.seek(15.);
        p.render_secs(1.);
        assert_eq!(p.status().key, Some(2));
    }
}
