use super::*;
use crate::{
    Config, Host, Track, deck, engine, mixer::Position, spectrum::Spectrum, types::Command,
};

struct HostEvents(Arc<Mutex<Vec<Event>>>);
impl Host for HostEvents {
    fn next_after(&mut self, _: &Track) -> Option<Track> {
        None
    }
    fn event(&mut self, event: Event) {
        self.0.lock().unwrap().push(event);
    }
}

struct Song(std::path::PathBuf);
impl Song {
    fn new(name: &str) -> Self {
        // Four seconds of nonzero mono PCM. Only an in-memory mixer sees these samples.
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-runs/device-change");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{name}-{}.wav", std::process::id()));
        let rate = 8000u32;
        let data_bytes = rate * 4 * 2;
        let mut wav = b"RIFF".to_vec();
        wav.extend((36 + data_bytes).to_le_bytes());
        wav.extend(b"WAVEfmt ");
        wav.extend(16u32.to_le_bytes());
        wav.extend(1u16.to_le_bytes()); // PCM
        wav.extend(1u16.to_le_bytes()); // mono
        wav.extend(rate.to_le_bytes());
        wav.extend((rate * 2).to_le_bytes());
        wav.extend(2u16.to_le_bytes());
        wav.extend(16u16.to_le_bytes());
        wav.extend(b"data");
        wav.extend(data_bytes.to_le_bytes());
        wav.extend([0, 32].into_iter().cycle().take(data_bytes as usize));
        std::fs::write(&path, wav).unwrap();
        Self(path)
    }
}
impl Drop for Song {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
fn loss_retries_freezes_time_and_restores_playing_or_paused_at_the_new_rate() {
    let epoch = Instant::now();
    let position = Arc::new(Position::new());
    let (a, ar) = deck::rings(48_000);
    let (b, br) = deck::rings(48_000);
    let (mixer, link) = Mixer::new(48_000, [ar, br], Spectrum::new().shared, position.clone());
    let mixer = Arc::new(Mutex::new(mixer));
    let events = Arc::new(Mutex::new(vec![]));
    let mut engine = Engine::new(
        Box::new(HostEvents(events.clone())),
        Config {
            muted: true,
            ..Default::default()
        },
        [a, b],
        link,
        position.clone(),
    );
    let mut out = Output::<()>::new(mixer.clone(), epoch);
    let song = Song::new("loss");
    let open = |out: &mut Output<()>, engine: &mut Engine, ms, rate| {
        out.update_with(
            epoch + Duration::from_millis(ms),
            engine,
            |_| Ok(((), rate)),
            |_| {
                assert_eq!(mixer.lock().unwrap().rate(), rate);
                Ok(format!("Output {rate}"))
            },
        );
    };
    let render = |engine: &mut Engine, blocks| {
        for _ in 0..blocks {
            engine.pump();
            let mut block = [1.0; 960];
            mixer.lock().unwrap().render(&mut block, 0);
            assert!(block.iter().all(|&sample| sample == 0.0));
        }
        engine.pump();
        engine::status(&position, None)
    };
    engine.command(Command::Play {
        track: Track::new(71, &song.0),
        from: 1.0,
        start: true,
    });
    open(&mut out, &mut engine, 0, 48_000);
    let before = render(&mut engine, 20);
    assert!(before.playing && before.key == Some(71) && before.position > 1.15);

    out.broken.store(true, Ordering::Relaxed); // the WASAPI error callback
    out.update_with(
        epoch + Duration::from_millis(200),
        &mut engine,
        |_| Err(Error::Output("no device".into())),
        |_| panic!("started a missing device"),
    );
    assert!(out.stream.is_none());
    let frozen = engine::status(&position, Some(9_000_000_000));
    assert!(frozen.playing && frozen.key == before.key);
    assert_eq!(frozen.position, before.position);
    out.update_with(
        epoch + Duration::from_millis(1199),
        &mut engine,
        |_| panic!("retried too soon"),
        |_| unreachable!(),
    );
    open(&mut out, &mut engine, 1200, 44_100);
    let after = render(&mut engine, 12);
    assert!(after.playing && after.key == before.key);
    assert!(
        (after.position - before.position - 5760. / 44_100.).abs() < 0.03,
        "{before:?} -> {after:?}"
    );

    engine.command(Command::Pause);
    let paused = render(&mut engine, 22);
    assert!(!paused.playing && paused.key == before.key);
    out.broken.store(true, Ordering::Relaxed);
    open(&mut out, &mut engine, 1500, 48_000);
    assert!(out.stream.is_none());
    let restored = engine::status(&position, None);
    assert!(!restored.playing && restored.key == paused.key);
    assert!(
        (restored.position - paused.position).abs() < 0.002,
        "{paused:?} -> {restored:?}"
    );
    engine.command(Command::Resume);
    open(&mut out, &mut engine, 1501, 48_000);
    assert!(out.stream.is_some());
    let resumed = render(&mut engine, 10);
    assert!(resumed.playing && resumed.key == paused.key);
    assert!(
        (resumed.position - restored.position - 0.1).abs() < 0.002,
        "{restored:?} -> {resumed:?}"
    );
    assert_eq!(
        events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                Event::Output { rate, .. } => Some(*rate),
                _ => None,
            })
            .collect::<Vec<_>>(),
        [48_000, 44_100, 48_000]
    );

    // An explicit error is not the only way a device can disappear.
    out.beats.fetch_add(1, Ordering::Relaxed);
    assert!(!out.needs_reopen(epoch + Duration::from_secs(9)));
    assert!(!out.needs_reopen(epoch + Duration::from_millis(10_500)));
    assert!(out.needs_reopen(epoch + Duration::from_millis(10_501)));

    // A seek made with no device hasn't reached a mixer callback yet. Its requested
    // position and play intent must take precedence over the retired stream's snapshot.
    out.broken.store(true, Ordering::Relaxed);
    out.update_with(
        epoch + Duration::from_secs(11),
        &mut engine,
        |_| Err(Error::Output("no device".into())),
        |_| unreachable!(),
    );
    engine.command(Command::Seek(2.5));
    engine.command(Command::Resume);
    open(&mut out, &mut engine, 12_000, 44_100);
    let sought = render(&mut engine, 12);
    assert!(sought.playing && sought.key == Some(71));
    assert!(
        (sought.position - 2.5 - 5760. / 44_100.).abs() < 0.03,
        "{sought:?}"
    );

    // A pause during the outage should not play its fade on the replacement speakers.
    out.broken.store(true, Ordering::Relaxed);
    out.update_with(
        epoch + Duration::from_secs(13),
        &mut engine,
        |_| Err(Error::Output("no device".into())),
        |_| unreachable!(),
    );
    engine.command(Command::Pause);
    out.update_with(
        epoch + Duration::from_millis(13_500),
        &mut engine,
        |_| panic!("retried too soon"),
        |_| unreachable!(),
    );
    assert!(!engine::status(&position, Some(13_500_000_000)).playing);
    open(&mut out, &mut engine, 14_000, 44_100);
    assert!(out.stream.is_none());
    let same_rate = engine::status(&position, None);
    assert!(!same_rate.playing && same_rate.key == sought.key);
    assert_eq!(same_rate.position, sought.position);
}

#[test]
fn stream_sleeps_after_pause_and_queue_end_and_wakes_on_play() {
    let epoch = Instant::now();
    let position = Arc::new(Position::new());
    let rate = 8_000;
    let (a, ar) = deck::rings(rate);
    let (b, br) = deck::rings(rate);
    let (mixer, link) = Mixer::new(rate, [ar, br], Spectrum::new().shared, position.clone());
    let mixer = Arc::new(Mutex::new(mixer));
    let events = Arc::new(Mutex::new(vec![]));
    let mut engine = Engine::new(
        Box::new(HostEvents(events.clone())),
        Config::default(),
        [a, b],
        link,
        position.clone(),
    );
    let mut out = Output::<()>::new(mixer.clone(), epoch);
    let mut tick = |engine: &mut Engine, ms, expect_stream| {
        engine.pump();
        out.update_with(
            epoch + Duration::from_millis(ms),
            engine,
            |_| Ok(((), rate)),
            |_| Ok("Test".into()),
        );
        assert_eq!(out.stream.is_some(), expect_stream, "at {ms} ms");
    };
    let song = Song::new("sleep");
    tick(&mut engine, 0, false);
    engine.command(Command::Play {
        track: Track::new(12, &song.0),
        from: 1.,
        start: false,
    });
    tick(&mut engine, 1, false);
    assert_eq!(engine::status(&position, None).position, 1.);
    engine.command(Command::Resume);
    tick(&mut engine, 2, true);
    let mut buf = vec![0.; 1600];
    mixer.lock().unwrap().render(&mut buf, 2_000_000);
    assert!(buf[0].abs() < 1e-6 && buf[1599] > 0.05);
    engine.command(Command::Pause);
    tick(&mut engine, 102, true);
    mixer.lock().unwrap().render(&mut buf, 102_000_000);
    tick(&mut engine, 202, true);
    mixer.lock().unwrap().render(&mut buf, 202_000_000);
    assert!(buf[0] > 0. && buf[1599] == 0.);
    tick(&mut engine, 261, true); // the final millisecond is still in the device buffer
    tick(&mut engine, 303, false);
    engine.command(Command::Seek(3.5));
    tick(&mut engine, 304, false);
    assert_eq!(engine::status(&position, None).position, 3.5);
    engine.command(Command::Resume);
    tick(&mut engine, 305, true);
    for block in 0..6 {
        engine.pump();
        mixer
            .lock()
            .unwrap()
            .render(&mut buf, (305 + block * 100) * 1_000_000);
        if block == 0 {
            assert!(buf[0].abs() < 1e-6 && buf[1599] > 0.05);
        }
    }
    tick(&mut engine, 906, false);
    assert!(events.lock().unwrap().contains(&Event::Stopped));
    let cued = engine::status(&position, None);
    assert!(!cued.playing && cued.key == Some(12) && cued.position == 0.);

    // A different default device can appear while the stream is asleep. A resume
    // from zero must still ramp up on its first callback at the new rate.
    engine.command(Command::Resume);
    out.update_with(
        epoch + Duration::from_millis(907),
        &mut engine,
        |_| Ok(((), 16_000)),
        |_| Ok("Replacement".into()),
    );
    engine.pump();
    mixer.lock().unwrap().render(&mut buf, 907_000_000);
    let halfway = buf[800];
    assert!(
        buf[0].abs() < 1e-5 && halfway > 0.0001 && halfway < buf[1599] && buf[1599] < 0.15,
        "wake at new rate: {}, {halfway}, {}",
        buf[0],
        buf[1599]
    );
}

#[test]
fn seek_after_the_last_callback_supersedes_its_queued_finish() {
    let position = Arc::new(Position::new());
    let (a, ar) = deck::rings(8_000);
    let (b, br) = deck::rings(8_000);
    let (mut mixer, link) = Mixer::new(8_000, [ar, br], Spectrum::new().shared, position.clone());
    let events = Arc::new(Mutex::new(vec![]));
    let mut engine = Engine::new(
        Box::new(HostEvents(events.clone())),
        Config::default(),
        [a, b],
        link,
        position.clone(),
    );
    let song = Song::new("last-callback");
    engine.command(Command::Play {
        track: Track::new(1, &song.0),
        from: 3.75,
        start: true,
    });
    engine.pump();
    mixer.render(&mut vec![0.; 4_200], 0);
    // The callback already exhausted the file; the engine receives the user's seek
    // before it reads the callback's events, just as it can on the device thread.
    engine.command(Command::Seek(1.0));
    engine.pump();
    let mut samples = [0.; 1_600];
    mixer.render(&mut samples, 300_000_000);
    engine.pump();
    let st = engine::status(&position, None);
    assert!(
        st.playing && st.key == Some(1) && (st.position - 1.1).abs() < 0.001,
        "{st:?}"
    );
    assert!(samples[1599] > 0.05);
    let events = events.lock().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, Event::Started { .. }))
            .count(),
        1
    );
    assert!(!events.contains(&Event::Stopped));
}
