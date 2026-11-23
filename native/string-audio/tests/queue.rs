//! The engine and a queue that changes: the next song made ready is dropped for the new answer
//! (before and after it was decoded), a seek after the next song was chained, songs at
//! different rates handing over, and the spectrum's read cost.

mod common;

use std::sync::{Arc, Mutex};

use common::*;
use string_audio::{Config, Event, Host, OfflinePlayer, Track};

const SR: u32 = 48_000;

/// A queue the test can change while it plays.
#[derive(Clone)]
struct Shared {
    list: Arc<Mutex<Vec<Track>>>,
    events: Arc<Mutex<Vec<Event>>>,
}

impl Host for Shared {
    fn next_after(&mut self, current: &Track) -> Option<Track> {
        let list = self.list.lock().unwrap();
        let i = list.iter().position(|t| t.key == current.key)?;
        list.get(i + 1).cloned()
    }

    fn event(&mut self, event: Event) {
        self.events.lock().unwrap().push(event);
    }
}

fn started(events: &Arc<Mutex<Vec<Event>>>) -> Vec<u64> {
    events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            Event::Started { track } => Some(track.key),
            _ => None,
        })
        .collect()
}

#[test]
fn editing_or_seeking_in_the_last_milliseconds_keeps_the_right_successor() {
    let dir = scratch("late-handover");
    let tracks: Vec<_> = [44_100, SR, 32_000]
        .into_iter()
        .enumerate()
        .map(|(i, rate)| {
            let path = dir.join(format!("{i}.wav"));
            write_wav(&path, rate, &level(0.25, rate as usize * 4));
            Track::new(i as u64, path)
        })
        .collect();
    for op in ["queue", "seek", "loop"] {
        let host = Shared {
            list: Arc::new(Mutex::new(tracks.clone())),
            events: Arc::new(Mutex::new(Vec::new())),
        };
        let mut p = OfflinePlayer::new(SR, Config::default(), host.clone());
        p.play_track(tracks[0].clone(), 0.);
        if op == "loop" {
            p.set_loop(Some(string_audio::LoopRange {
                start: 0.5,
                end: None,
            }));
        }
        p.render_secs(3.997);
        let successor = match op {
            "queue" => {
                host.list.lock().unwrap().remove(1);
                p.queue_changed();
                2
            }
            "seek" => {
                p.seek(2.);
                p.render_secs(0.5);
                assert_eq!(
                    p.status().key,
                    Some(0),
                    "seek handed over before taking effect"
                );
                assert!((2.35..2.51).contains(&p.status().position));
                p.render_secs(1.6);
                1
            }
            _ => {
                p.set_loop(None);
                1
            }
        };
        p.render_secs(0.25);
        assert_eq!(p.status().key, Some(successor), "{op}");
        assert!(
            p.status().playing && started(&host.events) == [0, successor],
            "{op}"
        );
        assert!(!host.events.lock().unwrap().contains(&Event::Stopped));
    }
}

/// Song A (3 s, numbered), B (1 s at 0.3), C (1 s at 0.6).
fn songs(name: &str) -> [Track; 3] {
    let dir = scratch(name);
    let (a, b, c) = (dir.join("a.wav"), dir.join("b.wav"), dir.join("c.wav"));
    write_wav(&a, SR, &numbered(SR as usize * 3));
    write_wav(&b, SR, &level(0.3, SR as usize));
    write_wav(&c, SR, &level(0.6, SR as usize));
    [Track::new(1, a), Track::new(2, b), Track::new(3, c)]
}

fn run_change(name: &str, change_after: f64) {
    let [a, b, c] = songs(name);
    let host = Shared {
        list: Arc::new(Mutex::new(vec![a.clone(), b.clone()])),
        events: Arc::new(Mutex::new(Vec::new())),
    };
    let mut p = OfflinePlayer::new(SR, Config::default(), host.clone());
    p.play_track(a.clone(), 0.0);
    let mut out = p.render_secs(change_after);
    *host.list.lock().unwrap() = vec![a, c];
    p.queue_changed();
    out.extend(p.render_secs(4.5 - change_after));
    let end_a = SR as usize * 3;
    for i in 0..end_a {
        assert_eq!(
            number_of(out[i * 2], out[i * 2 + 1]),
            Some(i),
            "{name}: A, frame {i}"
        );
    }
    // straight after A, C (never B)
    for i in end_a..end_a + SR as usize {
        assert!(
            (out[i * 2] - 0.6).abs() < 1e-4,
            "{name}: frame {i} is {} (C is 0.6)",
            out[i * 2]
        );
    }
    assert_eq!(started(&host.events), [1, 3], "{name}");
}

#[test]
fn a_changed_queue_is_followed_before_the_next_song_is_decoded() {
    // at 0.5 s, A isn't decoded to its end yet: B is only promised
    run_change("change-early", 0.5);
}

#[test]
fn a_changed_queue_is_followed_after_the_next_song_was_decoded() {
    // at 2 s, A was decoded to its end and B carried on after it in the ring
    run_change("change-late", 2.0);
}

#[test]
fn a_seek_after_the_next_song_was_chained() {
    let [a, b, _] = songs("seek-chained");
    let host = Shared {
        list: Arc::new(Mutex::new(vec![a.clone(), b.clone()])),
        events: Arc::new(Mutex::new(Vec::new())),
    };
    let mut p = OfflinePlayer::new(SR, Config::default(), host.clone());
    p.play_track(a.clone(), 0.0);
    p.render_secs(2.0);
    p.seek(1.0);
    let out = p.render_secs(3.5);
    // after the seek's fades, A from 1 s on, then B
    let fades = (0.06 * SR as f64) as usize + (0.16 * SR as f64) as usize + 600;
    let mut start = None;
    for i in fades..SR as usize * 2 - 100 {
        let n = number_of(out[i * 2], out[i * 2 + 1]).expect("A at full level");
        let s = *start.get_or_insert(i + SR as usize - n);
        assert_eq!(n, SR as usize + i - s, "frame {i}");
    }
    let s = start.unwrap();
    let b_at = s + SR as usize * 2;
    assert!(
        (out[(b_at + 100) * 2] - 0.3).abs() < 1e-4,
        "B follows A's end"
    );
    assert_eq!(started(&host.events), [1, 2]);
}

#[test]
fn songs_at_different_rates_hand_over_without_a_gap() {
    let dir = scratch("cross-rate");
    let (a, b) = (dir.join("a.wav"), dir.join("b.wav"));
    // one recording, cut in two, the second half at another rate
    let n1 = 52_000;
    write_wav(&a, 44_100, &signal(44_100, n1));
    let t1 = n1 as f64 / 44_100.0;
    write_wav(&b, SR, &signal_from(SR, t1, SR as usize));
    let tracks = vec![Track::new(1, &a), Track::new(2, &b)];
    let (host, events) = ListHost::new(tracks.clone());
    let mut p = OfflinePlayer::new(SR, Config::default(), host);
    p.play_track(tracks[0].clone(), 0.0);
    let out = p.render_secs(3.0);
    let j = (n1 as f64 * SR as f64 / 44_100.0).round() as usize;
    let total = j + SR as usize;
    let end = sound_ends(&out, 0);
    assert!(
        end.abs_diff(total) <= 2,
        "no gap, no overlap: {end} vs {total}"
    );
    let want = signal(SR, total);
    let err = max_diff(&out, &want, j - 2_000, j + 2_000);
    eprintln!("join across rates: worst error {err:.4}");
    // the two halves' sample grids sit up to half a frame apart: close, not exact
    assert!(err < 0.1, "the join across rates is off ({err})");
    assert_eq!(
        names(&events),
        ["started 1", "ended 1", "started 2", "ended 2"]
    );
}

fn names(events: &Arc<Mutex<Vec<Event>>>) -> Vec<String> {
    events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            Event::Started { track } => Some(format!("started {}", track.key)),
            Event::Ended { track } => Some(format!("ended {}", track.key)),
            _ => None,
        })
        .collect()
}

#[test]
fn reading_the_spectrum_is_cheap() {
    let dir = scratch("spectrum-cost");
    let song = dir.join("t.wav");
    write_wav(&song, SR, &tone(SR, 440.0, 0.5, SR as usize * 2));
    let t = Track::new(1, &song);
    let (host, _) = ListHost::new(vec![t.clone()]);
    let mut p = OfflinePlayer::new(
        SR,
        Config {
            spectrum: true,
            ..Config::default()
        },
        host,
    );
    let spec = p.spectrum();
    p.play_track(t, 0.0);
    p.render_secs(1.0);
    let mut bands = [0.0; string_audio::BANDS];
    let reads = 200_000;
    let t0 = std::time::Instant::now();
    let mut sum = 0.0;
    for _ in 0..reads {
        spec.read(&mut bands);
        sum += bands[30];
    }
    let each = t0.elapsed().as_nanos() as f64 / reads as f64;
    eprintln!("one spectrum read: {each:.0} ns ({sum:.0})");
    // a frame at 279 Hz lasts 3.6 ms; a read must be a speck of that even unoptimised
    assert!(each < 5_000.0, "a read took {each} ns");
}
