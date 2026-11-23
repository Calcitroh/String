//! stringplay: plays songs through String's sound engine on the default output, to try it by
//! ear. Gapless by default; `--crossfade <s>` crossfades; `--loop <a> <b>` loops the first song
//! between two times (seconds); `--record <file.wav>` also records what the output plays
//! (WASAPI loopback: everything the device plays, so keep other sound off); `--spectrum` reads
//! the visualizer's bands 279 times a second, as a screen would. While it plays, type
//! a command and Enter: `p` pause or play, `s <secs>` seek, `n` next song, `v <0..1>` volume,
//! `x <secs>` crossfade, `q` quit.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use string_audio::{Config, Event, Host, LoopRange, Player, Track};

struct List {
    tracks: Vec<Track>,
    tell: mpsc::Sender<Event>,
}

impl Host for List {
    fn next_after(&mut self, current: &Track) -> Option<Track> {
        let i = self.tracks.iter().position(|t| t.key == current.key)?;
        self.tracks.get(i + 1).cloned()
    }

    fn event(&mut self, event: Event) {
        let _ = self.tell.send(event);
    }
}

fn usage() -> ! {
    eprintln!(
        "usage: stringplay [--crossfade <secs>] [--loop <a> <b>] [--volume <0..1>] [--seconds <n>] [--record <file.wav>] [--spectrum] <files...>"
    );
    std::process::exit(2)
}

/// Records the default output (loopback) until dropped; the samples, rate and channels.
struct Recorder {
    _stream: cpal::Stream,
    samples: Arc<Mutex<Vec<f32>>>,
    rate: u32,
    channels: u16,
}

fn record() -> Option<Recorder> {
    let device = cpal::default_host().default_output_device()?;
    let config = device.default_output_config().ok()?;
    let (rate, channels) = (config.sample_rate(), config.channels());
    let samples = Arc::new(Mutex::new(Vec::new()));
    let sink = samples.clone();
    let stream = device
        .build_input_stream::<f32, _, _>(
            cpal::StreamConfig {
                channels,
                sample_rate: rate,
                buffer_size: cpal::BufferSize::Default,
            },
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                sink.lock().unwrap().extend_from_slice(data)
            },
            |e| eprintln!("recording: {e}"),
            None,
        )
        .ok()?;
    stream.play().ok()?;
    Some(Recorder {
        _stream: stream,
        samples,
        rate,
        channels,
    })
}

/// A 32-bit float WAV.
fn write_wav(path: &Path, rate: u32, channels: u16, samples: &[f32]) -> std::io::Result<()> {
    let data = (samples.len() * 4) as u32;
    let mut b = Vec::with_capacity(44 + data as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&3u16.to_le_bytes());
    b.extend_from_slice(&channels.to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * channels as u32 * 4).to_le_bytes());
    b.extend_from_slice(&(channels * 4).to_le_bytes());
    b.extend_from_slice(&32u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        b.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, b)
}

fn number(arg: Option<String>) -> f64 {
    arg.and_then(|a| a.parse().ok()).unwrap_or_else(|| usage())
}

fn clock(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

fn main() {
    let mut config = Config::default();
    let mut looped = None;
    let mut limit = None;
    let mut recording = None;
    let mut files = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--crossfade" => config.crossfade = number(args.next()),
            "--loop" => {
                let (a, b) = (number(args.next()), number(args.next()));
                looped = Some(LoopRange {
                    start: a,
                    end: Some(b),
                });
            }
            "--volume" => config.volume = number(args.next()) as f32,
            "--seconds" => limit = Some(number(args.next())),
            "--spectrum" => config.spectrum = true,
            "--record" => recording = Some(PathBuf::from(args.next().unwrap_or_else(|| usage()))),
            "-h" | "--help" => usage(),
            _ => files.push(PathBuf::from(a)),
        }
    }
    if files.is_empty() {
        usage();
    }
    let tracks: Vec<Track> = files
        .iter()
        .enumerate()
        .map(|(i, f)| Track::new(i as u64 + 1, f))
        .collect();
    let mut first = tracks[0].clone();
    first.loop_range = looped;
    let name = |key: u64| {
        tracks
            .iter()
            .find(|t| t.key == key)
            .and_then(|t| t.path.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let recorder = recording.as_ref().map(|_| {
        record().unwrap_or_else(|| {
            eprintln!("can't record the output");
            std::process::exit(1)
        })
    });
    let (tell, news) = mpsc::channel();
    let spectrum = config.spectrum;
    let player = Player::new(
        config,
        List {
            tracks: tracks.clone(),
            tell,
        },
    );
    if spectrum {
        // a screen at 279 Hz: one read of the bands per frame
        let feed = player.spectrum();
        std::thread::spawn(move || {
            let mut bands = [0.0; string_audio::BANDS];
            loop {
                feed.read(&mut bands);
                std::thread::sleep(Duration::from_micros(3_584));
            }
        });
    }
    player.play_track(first, 0.0);
    play(&player, &tracks, news, limit, &name);
    // let the last frames reach the device before the recording stops
    std::thread::sleep(Duration::from_millis(300));
    drop(player);
    if let (Some(path), Some(rec)) = (recording, recorder) {
        let samples = std::mem::take(&mut *rec.samples.lock().unwrap());
        match write_wav(&path, rec.rate, rec.channels, &samples) {
            Ok(()) => eprintln!(
                "recorded {:.1} s to {}",
                samples.len() as f64 / (rec.rate as f64 * rec.channels as f64),
                path.display()
            ),
            Err(e) => eprintln!("can't write {}: {e}", path.display()),
        }
    }
}

fn play(
    player: &Player,
    tracks: &[Track],
    news: mpsc::Receiver<Event>,
    limit: Option<f64>,
    name: &dyn Fn(u64) -> String,
) {
    let (typed_tx, typed) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if typed_tx.send(line).is_err() {
                break;
            }
        }
    });

    let started = std::time::Instant::now();
    let mut paused = false;
    loop {
        while let Ok(e) = news.try_recv() {
            match e {
                Event::Started { track } => eprintln!("\r> {}", name(track.key)),
                Event::Failed { track, error, .. } => eprintln!("\r! {}: {error}", name(track.key)),
                Event::Output { name, rate } => eprintln!("\r~ {name} at {rate} Hz"),
                Event::Stopped => {
                    eprintln!("\rdone.");
                    return;
                }
                _ => {}
            }
        }
        while let Ok(line) = typed.try_recv() {
            let mut words = line.split_whitespace();
            match (
                words.next(),
                words.next().and_then(|w| w.parse::<f64>().ok()),
            ) {
                (Some("p"), _) => {
                    if paused {
                        player.play()
                    } else {
                        player.pause()
                    }
                    paused = !paused;
                }
                (Some("s"), Some(secs)) => player.seek(secs),
                (Some("v"), Some(v)) => player.set_volume(v as f32),
                (Some("x"), Some(x)) => player.set_crossfade(x),
                (Some("n"), _) => {
                    let st = player.status();
                    let next = st
                        .key
                        .and_then(|k| tracks.iter().position(|t| t.key == k))
                        .and_then(|i| tracks.get(i + 1));
                    match next {
                        Some(t) => player.play_track(t.clone(), 0.0),
                        None => return,
                    }
                }
                (Some("q"), _) => return,
                _ => eprintln!(
                    "\rp: pause/play, s <secs>: seek, n: next, v <0..1>: volume, x <secs>: crossfade, q: quit"
                ),
            }
        }
        if limit.is_some_and(|l| started.elapsed().as_secs_f64() >= l) {
            return;
        }
        let st = player.status();
        if let Some(key) = st.key {
            let dur = st.duration.map(clock).unwrap_or_else(|| "?".into());
            eprint!("\r  {} / {dur}  {}   ", clock(st.position), name(key));
            let _ = std::io::stderr().flush();
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}
