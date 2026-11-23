//! One song, opened and read as stereo f32 frames at its own sample rate, gapless: the encoder's
//! delay and padding are trimmed (LAME/Xing and Vorbis by the decoders, Opus pre-skip and
//! granule ends here, MP4 edit lists and iTunSMPB through `mp4`), and a seek lands on the exact
//! frame (decoding starts a little earlier, so lossy decoders settle before the frames kept).

use std::fs::File;
use std::path::Path;

use symphonia::core::codecs::audio::well_known::{
    CODEC_ID_AAC, CODEC_ID_ALAC, CODEC_ID_MP1, CODEC_ID_MP2, CODEC_ID_MP3, CODEC_ID_OPUS,
    CODEC_ID_VORBIS,
};
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::codecs::registry::CodecRegistry;
use symphonia::core::common::Tier;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::Timestamp;

use crate::Error;
use crate::mp4;

/// How long a lossy decoder runs before a seek's target, so its output there is right again.
/// Opus settles slowly (measured: 0.15 s still 1e-2 off, 0.35 s exact).
const LOSSY_PREROLL_SECS: f64 = 0.1;
const OPUS_PREROLL_SECS: f64 = 0.4;

pub(crate) struct Source {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    rate: u32,
    /// The track's timebase as numerator / denominator seconds per tick.
    tb: (u64, u64),
    /// The decoder trims packets' delay and padding itself (MP3, Vorbis).
    trims_itself: bool,
    /// Playable frame 0 is decoded frame `delay` (Opus pre-skip, MP4 priming).
    delay: u64,
    /// The exact playable length, when the decoded stream runs on past it (MP4 padding).
    limit: Option<u64>,
    /// The playable length (exact for most files; an estimate for MP3s without a header).
    frames: Option<u64>,
    /// Seconds decoded before a seek's target and dropped.
    preroll: f64,
    downmix: Downmix,
    /// Playable index of the next decoded frame, once known after an open or a seek.
    cursor: Option<i64>,
    /// Frames before this playable index are dropped (a seek's target).
    skip_to: u64,
    /// Decoded stereo frames waiting to be read.
    pending: Vec<f32>,
    pending_at: usize,
    scratch: Vec<f32>,
    done: bool,
}

/// The facts about an opened song.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Info {
    pub rate: u32,
    pub frames: Option<u64>,
}

fn codecs() -> &'static CodecRegistry {
    use std::sync::OnceLock;
    static REG: OnceLock<CodecRegistry> = OnceLock::new();
    REG.get_or_init(|| {
        let mut reg = CodecRegistry::new();
        symphonia::default::register_enabled_codecs(&mut reg);
        reg.register_audio_decoder_at_tier::<symphonia_opus::OpusDecoder>(Tier::Fallback);
        reg
    })
}

fn err(path: &Path, what: impl std::fmt::Display) -> Error {
    Error::Open(format!("{}: {what}", path.display()))
}

impl Source {
    pub fn open(path: &Path) -> Result<Source, Error> {
        let file = File::open(path).map_err(|e| err(path, e))?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }
        let format = symphonia::default::get_probe()
            .probe(
                &hint,
                mss,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .map_err(|e| err(path, e))?;
        let track = format
            .default_track(TrackType::Audio)
            .ok_or_else(|| err(path, "no sound in this file"))?
            .clone();
        let params = track
            .codec_params
            .as_ref()
            .and_then(|p| p.audio())
            .ok_or_else(|| err(path, "unknown sound format"))?
            .clone();
        let rate = params
            .sample_rate
            .ok_or_else(|| err(path, "no sample rate"))?;
        let channels = params.channels.clone();
        let decoder = codecs()
            .make_audio_decoder(&params, &AudioDecoderOptions::default().gapless(true))
            .map_err(|e| err(path, e))?;
        let tb = track
            .time_base
            .map(|t| (t.numer.get() as u64, t.denom.get() as u64))
            .unwrap_or((1, rate as u64));
        let codec = params.codec;
        let trims_itself =
            [CODEC_ID_MP1, CODEC_ID_MP2, CODEC_ID_MP3, CODEC_ID_VORBIS].contains(&codec);
        let preroll = if codec == CODEC_ID_OPUS {
            OPUS_PREROLL_SECS
        } else if trims_itself || codec == CODEC_ID_AAC {
            LOSSY_PREROLL_SECS
        } else {
            0.0
        };
        let ts_frames =
            |n: u64| -> u64 { ((n as u128 * tb.0 as u128 * rate as u128) / tb.1 as u128) as u64 };
        let mut delay = 0;
        let mut limit = None;
        let mut frames = track
            .num_frames
            .or_else(|| track.duration.map(|d| ts_frames(d.get())));
        if codec == CODEC_ID_OPUS {
            // the reader counts the pre-skip in its timestamps and in its length; the decoder
            // doesn't drop it
            delay = track.delay.unwrap_or(0) as u64;
            frames = frames.map(|f| f.saturating_sub(delay));
        } else if (codec == CODEC_ID_AAC || codec == CODEC_ID_ALAC)
            && let Some(g) = mp4::gapless(path, rate)
        {
            delay = g.delay;
            if let Some(len) = g.length {
                limit = Some(len);
                frames = Some(len);
            } else {
                frames = frames.map(|f| f.saturating_sub(delay));
            }
        }
        let count = channels.as_ref().map(|c| c.count()).unwrap_or(2).max(1);
        Ok(Source {
            format,
            decoder,
            track_id: track.id,
            rate,
            tb,
            trims_itself,
            delay,
            limit,
            frames,
            preroll,
            downmix: Downmix::new(channels.as_ref(), count),
            cursor: None,
            skip_to: 0,
            pending: Vec::new(),
            pending_at: 0,
            scratch: Vec::new(),
            done: false,
        })
    }

    pub fn info(&self) -> Info {
        Info {
            rate: self.rate,
            frames: self.frames,
        }
    }

    /// Appends up to `max` stereo frames to `out`; 0 means the song is over.
    pub fn read(&mut self, out: &mut Vec<f32>, max: usize) -> Result<usize, Error> {
        let mut got = 0;
        while got < max {
            if self.pending_at >= self.pending.len() {
                if self.done {
                    break;
                }
                self.pending.clear();
                self.pending_at = 0;
                self.decode_next()?;
                continue;
            }
            let have = (self.pending.len() - self.pending_at) / 2;
            let n = have.min(max - got);
            out.extend_from_slice(&self.pending[self.pending_at..self.pending_at + n * 2]);
            self.pending_at += n * 2;
            got += n;
        }
        Ok(got)
    }

    /// The next read starts at playable frame `frame`.
    pub fn seek(&mut self, frame: u64) -> Result<(), Error> {
        let preroll = (self.rate as f64 * self.preroll) as u64;
        // the priming frames before playable frame 0 serve as pre-roll too
        let target = if self.trims_itself {
            frame
        } else {
            frame + self.delay
        };
        let decoded = target.saturating_sub(preroll);
        // frames to timebase ticks, rounded down so the seek lands at or before the frame
        let ts =
            (decoded as u128 * self.tb.1 as u128 / (self.tb.0 as u128 * self.rate as u128)) as i64;
        self.pending.clear();
        self.pending_at = 0;
        self.cursor = None;
        self.skip_to = frame;
        self.done = false;
        if let Some(limit) = self.limit
            && frame >= limit
        {
            self.done = true;
            return Ok(());
        }
        match self.format.seek(
            SeekMode::Accurate,
            SeekTo::Timestamp {
                ts: Timestamp::new(ts),
                track_id: self.track_id,
            },
        ) {
            Ok(_) => {}
            Err(SymError::SeekError(_)) => self.done = true,
            Err(e) => return Err(Error::Decode(e.to_string())),
        }
        self.decoder.reset();
        Ok(())
    }

    fn ticks_to_frames(&self, ticks: i64) -> i64 {
        let v = ticks as i128 * self.tb.0 as i128 * self.rate as i128;
        let d = self.tb.1 as i128;
        (if v >= 0 {
            (v + d / 2) / d
        } else {
            (v - d / 2) / d
        }) as i64
    }

    fn decode_next(&mut self) -> Result<(), Error> {
        loop {
            let packet = match self.format.next_packet() {
                Ok(Some(p)) => p,
                Ok(None) => {
                    self.done = true;
                    return Ok(());
                }
                Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    self.done = true;
                    return Ok(());
                }
                // a chained stream changing its tracks: what came before is the song
                Err(SymError::ResetRequired) => {
                    self.done = true;
                    return Ok(());
                }
                Err(SymError::DecodeError(_)) => continue,
                Err(e) => return Err(Error::Decode(e.to_string())),
            };
            if packet.track_id != self.track_id {
                continue;
            }
            let trim_start = self.ticks_to_frames(packet.trim_start.get() as i64).max(0) as usize;
            let trim_end = self.ticks_to_frames(packet.trim_end.get() as i64).max(0) as usize;
            let pts = self.ticks_to_frames(packet.pts.get());
            let buf = match self.decoder.decode(&packet) {
                Ok(b) => b,
                // a damaged packet: skip it and go on, as players do
                Err(SymError::DecodeError(_)) => continue,
                Err(SymError::ResetRequired) => {
                    self.decoder.reset();
                    continue;
                }
                Err(e) => return Err(Error::Decode(e.to_string())),
            };
            let n = buf.frames();
            if n == 0 {
                continue;
            }
            // the decoded frames worth keeping, and the playable index of the first of them
            let (lo, hi, first) = if self.trims_itself {
                (0, n, pts + trim_start as i64)
            } else {
                let lo = trim_start.min(n);
                let hi = n.saturating_sub(trim_end).max(lo);
                (lo, hi, pts + lo as i64 - self.delay as i64)
            };
            let first = *self.cursor.get_or_insert(first);
            let kept = (hi - lo) as i64;
            self.cursor = Some(first + kept);
            // drop what lies before the seek target, or past the real end
            let mut from = lo;
            if first < self.skip_to as i64 {
                from += ((self.skip_to as i64 - first) as usize).min(hi - lo);
            }
            let mut to = hi;
            if let Some(limit) = self.limit {
                let room = (limit as i64 - first).clamp(0, kept) as usize;
                to = to.min(lo + room);
                if first + kept >= limit as i64 {
                    self.done = true;
                }
            }
            if from >= to {
                if self.done {
                    return Ok(());
                }
                continue;
            }
            buf.copy_to_vec_interleaved::<f32>(&mut self.scratch);
            self.downmix
                .apply(&self.scratch, from, to, &mut self.pending);
            return Ok(());
        }
    }
}

/// Any channel layout to stereo: mono to both sides, 5.1 and the like by the usual weights.
struct Downmix {
    channels: usize,
    /// (left weight, right weight) per source channel; empty for mono and stereo.
    weights: Vec<(f32, f32)>,
}

impl Downmix {
    fn new(layout: Option<&symphonia::core::audio::Channels>, count: usize) -> Downmix {
        use symphonia::core::audio::Channels;
        if count <= 2 {
            return Downmix {
                channels: count,
                weights: Vec::new(),
            };
        }
        const H: f32 = std::f32::consts::FRAC_1_SQRT_2;
        let mut weights: Vec<(f32, f32)> = match layout {
            Some(Channels::Positioned(pos)) => (0..64)
                .filter(|bit| pos.bits() & (1u64 << bit) != 0)
                .map(|bit| match bit {
                    0 => (1.0, 0.0),
                    1 => (0.0, 1.0),
                    2 | 8 | 11 | 13 => (H, H),
                    3 => (0.0, 0.0),
                    4 | 6 | 9 | 12 | 15 => (H, 0.0),
                    5 | 7 | 10 | 14 | 17 => (0.0, H),
                    _ => (H * 0.5, H * 0.5),
                })
                .collect(),
            _ => (0..count)
                .map(|i| if i % 2 == 0 { (H, 0.0) } else { (0.0, H) })
                .collect(),
        };
        weights.resize(count, (0.0, 0.0));
        // keep the loudest side from clipping
        let left: f32 = weights.iter().map(|w| w.0).sum();
        let right: f32 = weights.iter().map(|w| w.1).sum();
        let norm = left.max(right).max(1.0);
        for w in &mut weights {
            w.0 /= norm;
            w.1 /= norm;
        }
        Downmix {
            channels: count,
            weights,
        }
    }

    fn apply(&self, src: &[f32], from: usize, to: usize, out: &mut Vec<f32>) {
        let c = self.channels;
        out.reserve((to - from) * 2);
        match c {
            1 => {
                for &s in &src[from..to] {
                    out.push(s);
                    out.push(s);
                }
            }
            2 => out.extend_from_slice(&src[from * 2..to * 2]),
            _ => {
                for frame in src[from * c..to * c].chunks_exact(c) {
                    let (mut l, mut r) = (0.0, 0.0);
                    for (s, w) in frame.iter().zip(&self.weights) {
                        l += s * w.0;
                        r += s * w.1;
                    }
                    out.push(l);
                    out.push(r);
                }
            }
        }
    }
}
