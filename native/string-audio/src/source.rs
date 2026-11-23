//! One song, opened and read as stereo f32 frames at its own sample rate, gapless: the encoder's
//! delay and padding are trimmed (LAME/Xing and Vorbis by the decoders, Opus pre-skip and
//! granule ends here, MP4 edit lists and iTunSMPB through `mp4`), and a seek lands on the exact
//! frame (decoding starts a little earlier, so lossy decoders settle before the frames kept).

use std::fs::File;
use std::io::Seek;
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
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::packet::Packet;
use symphonia::core::units::Timestamp;

use crate::Error;
use crate::mp4;
use symphonia::core::formats::well_known::{FORMAT_ID_MKV, FORMAT_ID_MP3};

#[cfg(windows)]
mod aac_profile;
#[cfg(windows)]
mod media_foundation;
mod mp3;
mod webm;

#[cfg(test)]
mod format_tests;

/// How long a lossy decoder runs before a seek's target, so its output there is right again.
/// Opus settles slowly (measured: 0.15 s still 1e-2 off, 0.35 s exact).
const LOSSY_PREROLL_SECS: f64 = 0.1;
const OPUS_PREROLL_SECS: f64 = 0.4;

pub(crate) enum Source {
    Native(NativeSource),
    #[cfg(windows)]
    Windows(media_foundation::AacSource),
}

pub(crate) struct NativeSource {
    #[cfg(windows)]
    aac_check: Option<aac_profile::ProfileCheck>,
    webm: Option<WebmSeek>,
    corrected_mp3: bool,
    mixed_mp3: Option<Box<mp3::Mixed>>,
    // When an MP3 header describes only part of the stream, seek by the packets actually read.
    packet_preroll: i64,
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
    /// The playable length reported to the deck at open.
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

struct WebmSeek {
    file: File,
    cues: bool,
    opus_delay: Option<u64>,
}

fn probe(path: &Path, media: Box<dyn MediaSource>) -> Result<Box<dyn FormatReader>, Error> {
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    probe_media(&hint, media).map_err(|e| err(path, e))
}

fn probe_media(
    hint: &Hint,
    media: Box<dyn MediaSource>,
) -> Result<Box<dyn FormatReader>, SymError> {
    let mss = MediaSourceStream::new(media, Default::default());
    symphonia::default::get_probe().probe(
        hint,
        mss,
        FormatOptions::default(),
        MetadataOptions::default(),
    )
}

fn reprobe(path: &Path, file: &mut File) -> Result<Box<dyn FormatReader>, Error> {
    file.rewind().map_err(|e| err(path, e))?;
    probe(path, Box::new(file.try_clone().map_err(|e| err(path, e))?))
}

// An open-ended container can signal its physical end either way. Damaged packets have
// already advanced the reader and can be skipped without losing the rest of the song.
fn next_packet(reader: &mut dyn FormatReader) -> Result<Option<Packet>, SymError> {
    loop {
        match reader.next_packet() {
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Ok(None);
            }
            Err(SymError::DecodeError(_)) => continue,
            result => return result,
        }
    }
}

// Live WebM writers can omit Segment/Info/Duration. Only the last two audio packets need
// decoding to get the end timestamp (Vorbis overlaps the preceding block).
fn webm_end(
    format: &mut dyn FormatReader,
    decoder: &mut dyn AudioDecoder,
    track_id: u32,
    frames_at: impl Fn(u64) -> u64,
) -> Result<Option<u64>, SymError> {
    let (mut previous, mut last) = (None, None);
    while let Some(packet) = next_packet(format)? {
        if packet.track_id == track_id {
            previous = last.replace(packet);
        }
    }
    let Some(last) = last else { return Ok(None) };
    if let Some(previous) = previous {
        decoder.decode(&previous)?;
    }
    let count = decoder.decode(&last)?.frames() as u64;
    decoder.reset();
    Ok(Some(frames_at(last.pts.get().max(0) as u64) + count))
}

impl Source {
    pub fn open(path: &Path) -> Result<Source, Error> {
        Self::open_using(
            path,
            #[cfg(windows)]
            media_foundation::AacSource::open,
        )
    }

    fn open_using(
        path: &Path,
        #[cfg(windows)] open_aac: fn(
            &Path,
            Option<f64>,
        ) -> Result<media_foundation::AacSource, Error>,
    ) -> Result<Self, Error> {
        let mut file = File::open(path).map_err(|e| err(path, e))?;
        let clone = |file: &File| file.try_clone().map_err(|e| err(path, e));
        let mut format = probe(path, Box::new(clone(&file)?))?;
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
            .filter(|&hz| hz != 0)
            .ok_or_else(|| err(path, "no sample rate"))?;
        let decoder =
            codecs().make_audio_decoder(&params, &AudioDecoderOptions::default().gapless(true));
        #[cfg(windows)]
        let mut aac_check = None;
        #[cfg(windows)]
        if params.codec == CODEC_ID_AAC {
            match media_foundation::AacSource::fallback(
                path,
                format.as_mut(),
                rate,
                decoder.as_deref().ok(),
                open_aac,
            )
            .map_err(|e| err(path, e))?
            {
                media_foundation::Fallback::Platform(source) => return Ok(Source::Windows(source)),
                media_foundation::Fallback::Native => {}
                media_foundation::Fallback::Check(mut check) => {
                    // Inspect the opening access units before committing this selection.
                    // Continue checking during decode in case the profile changes later.
                    let mut seen = 0;
                    while seen < 4 {
                        let Some(p) = next_packet(format.as_mut()).map_err(|e| err(path, e))?
                        else {
                            break;
                        };
                        if p.track_id == track.id {
                            check.check(&p.data).map_err(|e| err(path, e))?;
                            seen += 1;
                        }
                    }
                    format = reprobe(path, &mut file)?;
                    aac_check = Some(check);
                }
            }
        }
        let mut decoder = decoder.map_err(|e| err(path, e))?;
        let rate = decoder.codec_params().sample_rate.unwrap_or(rate);
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
        let mut corrected_mp3 = false;
        let mut mixed_mp3 = None;
        if format.format_info().format == FORMAT_ID_MP3 {
            let (length, audio) =
                mp3::length(clone(&file)?, &track, rate).map_err(|e| err(path, e))?;
            format = probe(path, Box::new(audio))?;
            let filtered = format
                .default_track(TrackType::Audio)
                .ok_or_else(|| err(path, "no MP3 track"))?;
            let header_end = filtered
                .num_frames
                .and_then(|n| n.checked_add(u64::from(filtered.padding.unwrap_or(0))));
            corrected_mp3 = length.mixed || header_end != Some(length.frames);
            frames = if corrected_mp3 {
                Some(length.frames)
            } else {
                filtered.num_frames
            };
            if length.mixed {
                mixed_mp3 = Some(Box::new(mp3::Mixed::new(rate, track.delay.unwrap_or(0))));
            }
        }
        let mut webm = None;
        if format.format_info().format == FORMAT_ID_MKV {
            let media = format.media_info();
            frames = frames.or_else(|| {
                let tb = media.time_base?;
                Some(
                    (u128::from(media.duration?.get())
                        * u128::from(tb.numer.get())
                        * u128::from(rate)
                        / u128::from(tb.denom.get())) as u64,
                )
            });
            let opus = codec == CODEC_ID_OPUS;
            let layout =
                webm::layout(clone(&file)?, opus.then_some(track.id)).map_err(|e| err(path, e))?;
            format = reprobe(path, &mut file)?;
            if opus {
                let header = params.extra_data.as_deref().unwrap_or_default();
                let pre_skip = header
                    .strip_prefix(b"OpusHead")
                    .and_then(|h| h.get(2..4))
                    .map_or(0, |b| u64::from(u16::from_le_bytes([b[0], b[1]])));
                delay = layout
                    .delay_ns
                    .map(|n| webm::ns_to_frames(n, rate))
                    .unwrap_or(pre_skip * u64::from(rate) / 48000);
                let samples = match layout.samples {
                    Some(n) => n,
                    None => {
                        let mut total = 0;
                        while let Some(p) =
                            next_packet(format.as_mut()).map_err(|e| err(path, e))?
                        {
                            if p.track_id == track.id {
                                total += webm::packet_samples(&p.data)
                                    .ok_or_else(|| err(path, "invalid Opus packet"))?;
                            }
                        }
                        format = reprobe(path, &mut file)?;
                        total
                    }
                };
                let tail = webm::ns_to_frames(layout.tail_ns.max(0) as u64, rate);
                frames = Some(
                    (samples * u64::from(rate) / 48000)
                        .saturating_sub(delay)
                        .saturating_sub(tail),
                );
                limit = frames;
            } else if frames.is_none() {
                frames = webm_end(format.as_mut(), decoder.as_mut(), track.id, ts_frames)
                    .map_err(|e| err(path, e))?;
                format = reprobe(path, &mut file)?;
            }
            webm = Some(WebmSeek {
                file,
                cues: layout.cues,
                opus_delay: opus.then_some(delay),
            });
        } else if codec == CODEC_ID_OPUS {
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
        let channels = decoder
            .codec_params()
            .channels
            .as_ref()
            .or(params.channels.as_ref());
        let count = channels.map(|c| c.count()).unwrap_or(2).max(1);
        let downmix = Downmix::new(channels, count);
        let start = webm.as_ref().map(|w| -(w.opus_delay.unwrap_or(0) as i64));
        Ok(Source::Native(NativeSource {
            #[cfg(windows)]
            aac_check,
            webm,
            corrected_mp3,
            mixed_mp3,
            packet_preroll: i64::MIN,
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
            downmix,
            cursor: start,
            skip_to: 0,
            pending: Vec::new(),
            pending_at: 0,
            scratch: Vec::new(),
            done: false,
        }))
    }

    pub fn info(&self) -> Info {
        match self {
            Self::Native(s) => s.info(),
            #[cfg(windows)]
            Self::Windows(s) => s.info,
        }
    }

    pub fn read(&mut self, out: &mut Vec<f32>, max: usize) -> Result<usize, Error> {
        match self {
            Self::Native(s) => s.read(out, max),
            #[cfg(windows)]
            Self::Windows(s) => s.read(out, max),
        }
    }

    pub fn seek(&mut self, frame: u64) -> Result<(), Error> {
        match self {
            Self::Native(s) => s.seek(frame),
            #[cfg(windows)]
            Self::Windows(s) => s.seek(frame),
        }
    }
}

impl NativeSource {
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
        if let Some(webm) = &mut self.webm
            && (!webm.cues || decoded == 0)
        {
            // Without Cues Matroska only searches forwards from its current position.
            // Reprobe the same open file so a rename cannot substitute a different song.
            webm.file
                .rewind()
                .map_err(|e| Error::Decode(e.to_string()))?;
            let file = webm
                .file
                .try_clone()
                .map_err(|e| Error::Decode(e.to_string()))?;
            self.format = probe_media(&Hint::new(), Box::new(file))
                .map_err(|e| Error::Decode(e.to_string()))?;
            self.decoder.reset();
            self.cursor = Some(-(webm.opus_delay.unwrap_or(0) as i64));
            return Ok(());
        }
        let ts = if let Some(mixed) = &mut self.mixed_mp3 {
            mixed.rewind()
        } else if self.corrected_mp3 {
            self.packet_preroll = ts.saturating_sub(i64::from(self.rate) / 2);
            0
        } else {
            ts
        };
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
            let mut packet = match next_packet(self.format.as_mut()) {
                Ok(Some(p)) => p,
                Ok(None) => {
                    if let Some(mixed) = &mut self.mixed_mp3 {
                        mixed.finish(self.skip_to, &mut self.pending);
                    }
                    self.done = true;
                    return Ok(());
                }
                // a chained stream changing its tracks: what came before is the song
                Err(SymError::ResetRequired) => {
                    self.done = true;
                    return Ok(());
                }
                Err(e) => return Err(Error::Decode(e.to_string())),
            };
            if packet.track_id != self.track_id {
                continue;
            }
            if self.corrected_mp3 {
                if packet.pts.get() < self.packet_preroll {
                    continue;
                }
                packet.trim_end = symphonia::core::units::Duration::ZERO;
            }
            if let Some(mixed) = &mut self.mixed_mp3 {
                match mixed.packet(&packet, self.skip_to, &mut self.pending) {
                    Ok(()) => return Ok(()),
                    Err(SymError::DecodeError(_)) => continue,
                    Err(e) => return Err(Error::Decode(e.to_string())),
                }
            }
            let trim_start = self.ticks_to_frames(packet.trim_start.get() as i64).max(0) as usize;
            let trim_end = self.ticks_to_frames(packet.trim_end.get() as i64).max(0) as usize;
            let pts = self.ticks_to_frames(packet.pts.get());
            #[cfg(windows)]
            if let Some(check) = &mut self.aac_check {
                check.check(&packet.data)?;
            }
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
            let first = *self.cursor.get_or_insert_with(|| {
                if self.webm.as_ref().is_some_and(|w| w.opus_delay.is_some()) {
                    // Opus packet starts lie on a 2.5 ms grid. Undo the container tick
                    // rounding after a cue seek. Its timestamps already subtract CodecDelay.
                    let quantum = i64::from(self.rate) / 400;
                    ((pts + self.delay as i64 + quantum / 2).div_euclid(quantum)) * quantum
                        - self.delay as i64
                } else {
                    first
                }
            });
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
        // Browser 5.1 speaker mixing keeps the front channels at unity and drops LFE.
        // Preserve the existing headroom for other layouts.
        let standard_51 =
            matches!(layout,Some(Channels::Positioned(p)) if matches!(p.bits(), 0x3f|0x60f));
        if !standard_51 {
            let left: f32 = weights.iter().map(|w| w.0).sum();
            let right: f32 = weights.iter().map(|w| w.1).sum();
            let norm = left.max(right).max(1.0);
            for w in &mut weights {
                w.0 /= norm;
                w.1 /= norm;
            }
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
