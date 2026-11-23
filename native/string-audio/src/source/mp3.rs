//! Count MPEG audio frames before publishing the duration. The first header can describe only
//! the first part of a joined file, or give a stale frame count even with the correct byte size.
use super::codecs;
use crate::resample::Resampler;
use symphonia::core::audio::Channels;
use symphonia::core::codecs::audio::well_known::CODEC_ID_MP3;
use symphonia::core::codecs::audio::{AudioCodecParameters, AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::{Error, Result};
use symphonia::core::formats::{FormatReader, SeekMode, SeekTo, Track};
use symphonia::core::packet::Packet;
use symphonia::core::units::Timestamp;

pub(super) struct Length {
    pub frames: u64,
    /// Symphonia based its packet trims and seek bounds on a different end.
    pub corrected: bool,
    pub mixed: bool,
}

pub(super) fn length(reader: &mut dyn FormatReader, track: &Track, rate: u32) -> Result<Length> {
    let mut end = 0;
    let mut spec = None;
    let mut run_frames = 0;
    let mut converted = 0;
    let mut mixed = false;
    loop {
        match reader.next_packet() {
            Ok(Some(packet)) => {
                // Layer III has 1152 samples per MPEG-1 frame and 576 for MPEG-2/2.5.
                // The reader has already skipped ID3, Xing and VBRI packets, even mid-file.
                let (next, samples) = signal(&packet)?;
                if let Some(previous) = spec
                    && previous != next
                {
                    mixed = true;
                    converted += rescale(run_frames, previous, rate);
                    run_frames = 0;
                }
                spec = Some(next);
                run_frames += samples.saturating_sub(packet.trim_start.get());
                end = (packet.pts.get() + samples as i64).max(0) as u64;
            }
            Ok(None) => break,
            Err(Error::DecodeError(_)) => continue,
            Err(e) => return Err(e),
        }
    }
    if end == 0 {
        return Err(Error::DecodeError("no MP3 audio frames"));
    }
    reader.seek(
        SeekMode::Accurate,
        SeekTo::Timestamp {
            ts: Timestamp::new(-i64::from(track.delay.unwrap_or(0))),
            track_id: track.id,
        },
    )?;
    if mixed {
        return Ok(Length {
            frames: converted + rescale(run_frames, spec.unwrap(), rate),
            corrected: true,
            mixed: true,
        });
    }
    let agrees = track
        .num_frames
        .and_then(|n| n.checked_add(u64::from(track.padding.unwrap_or(0))))
        == Some(end);
    Ok(Length {
        frames: if agrees {
            track.num_frames.unwrap_or(end)
        } else {
            end
        },
        corrected: !agrees,
        mixed: false,
    })
}

#[derive(Clone, Copy, PartialEq)]
struct Signal {
    rate: u32,
    mono: bool,
}

fn signal(packet: &Packet) -> Result<(Signal, u64)> {
    let h = packet
        .data
        .get(..4)
        .ok_or(Error::DecodeError("incomplete MP3 header"))?;
    let base = *[44100, 48000, 32000]
        .get(usize::from((h[2] >> 2) & 3))
        .ok_or(Error::DecodeError("invalid MP3 sample rate"))?;
    let version = (h[1] >> 3) & 3;
    let divisor = match version {
        3 => 1,
        2 => 2,
        0 => 4,
        _ => return Err(Error::DecodeError("invalid MPEG version")),
    };
    Ok((
        Signal {
            rate: base / divisor,
            mono: h[3] >> 6 == 3,
        },
        if version == 3 { 1152 } else { 576 },
    ))
}

fn rescale(frames: u64, spec: Signal, rate: u32) -> u64 {
    ((u128::from(frames) * u128::from(rate) + u128::from(spec.rate / 2)) / u128::from(spec.rate))
        as u64
}

/// A byte-joined file may switch from mono to stereo, or even to another clock. Start a fresh
/// Layer III decoder at each such boundary and deliver one continuous stereo clock to the deck.
pub(super) struct Mixed {
    output_rate: u32,
    run: Option<Run>,
    cursor: u64,
    start_ts: i64,
    pcm: Vec<f32>,
    stereo: Vec<f32>,
    converted: Vec<f32>,
}

struct Run {
    spec: Signal,
    decoder: Box<dyn AudioDecoder>,
    resampler: Resampler,
    lead: usize,
}

impl Mixed {
    pub fn new(rate: u32, delay: u32) -> Self {
        Self {
            output_rate: rate,
            run: None,
            cursor: 0,
            start_ts: -i64::from(delay),
            pcm: Vec::new(),
            stereo: Vec::new(),
            converted: Vec::new(),
        }
    }

    pub fn rewind(&mut self) -> i64 {
        self.run = None;
        self.cursor = 0;
        self.start_ts
    }

    pub fn packet(&mut self, packet: &Packet, skip: u64, out: &mut Vec<f32>) -> Result<()> {
        let (spec, _) = signal(packet)?;
        if self.run.as_ref().is_none_or(|run| run.spec != spec) {
            self.finish(skip, out);
            let mut params = AudioCodecParameters::new();
            params
                .for_codec(CODEC_ID_MP3)
                .with_sample_rate(spec.rate)
                .with_channels(Channels::Discrete(if spec.mono { 1 } else { 2 }));
            let decoder = codecs()
                .make_audio_decoder(&params, &AudioDecoderOptions::default().gapless(true))?;
            let resampler = Resampler::new(spec.rate, self.output_rate);
            let lead = resampler.lead_in();
            self.run = Some(Run {
                spec,
                decoder,
                resampler,
                lead,
            });
        }
        let run = self.run.as_mut().unwrap();
        run.decoder
            .decode(packet)?
            .copy_to_vec_interleaved::<f32>(&mut self.pcm);
        let stereo = if spec.mono {
            self.stereo.clear();
            self.stereo.extend(self.pcm.iter().flat_map(|&v| [v, v]));
            &self.stereo
        } else {
            &self.pcm
        };
        self.converted.clear();
        run.resampler.process(stereo, &mut self.converted);
        self.emit(skip, out);
        Ok(())
    }

    pub fn finish(&mut self, skip: u64, out: &mut Vec<f32>) {
        if let Some(run) = &mut self.run {
            self.converted.clear();
            run.resampler.finish(0, &mut self.converted);
            self.emit(skip, out);
            self.run = None;
        }
    }

    fn emit(&mut self, skip: u64, out: &mut Vec<f32>) {
        let run = self.run.as_mut().unwrap();
        let drop = run.lead.min(self.converted.len() / 2);
        run.lead -= drop;
        let frames = self.converted.len() / 2 - drop;
        let before_target = skip.saturating_sub(self.cursor).min(frames as u64) as usize;
        out.extend_from_slice(&self.converted[(drop + before_target) * 2..]);
        self.cursor += frames as u64;
    }
}
