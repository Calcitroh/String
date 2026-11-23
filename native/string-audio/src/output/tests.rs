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
    fn new() -> Self {
        // Four seconds of nonzero mono PCM. Only an in-memory mixer sees these samples.
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-runs/device-change");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("string-device-{}.wav", std::process::id()));
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
    let song = Song::new();
    let open = |out: &mut Output<()>, engine: &mut Engine, ms, rate| {
        out.update_with(
            epoch + Duration::from_millis(ms),
            engine,
            |_| Ok(((), format!("Output {rate}"), rate)),
            |_| {
                assert_eq!(mixer.lock().unwrap().rate(), rate);
                Ok(())
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
    open(&mut out, &mut engine, 0, 48_000);
    engine.command(Command::Play {
        track: Track::new(71, &song.0),
        from: 1.0,
        start: true,
    });
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
    let restored = render(&mut engine, 20);
    assert!(!restored.playing && restored.key == paused.key);
    assert!(
        (restored.position - paused.position).abs() < 0.002,
        "{paused:?} -> {restored:?}"
    );
    assert_eq!(render(&mut engine, 10).position, restored.position);
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
    assert!(!out.needs_reopen(epoch + Duration::from_secs(8), false));
    out.beats.fetch_add(1, Ordering::Relaxed);
    assert!(!out.needs_reopen(epoch + Duration::from_secs(9), true));
    assert!(!out.needs_reopen(epoch + Duration::from_millis(10_500), true));
    assert!(out.needs_reopen(epoch + Duration::from_millis(10_501), true));

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
    let same_rate = render(&mut engine, 24);
    assert!(!same_rate.playing && same_rate.key == sought.key);
    assert_eq!(same_rate.position, sought.position);
}
