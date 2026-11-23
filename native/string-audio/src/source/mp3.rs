//! Count MPEG audio frames before publishing the duration. The first header can describe only
//! the first part of a joined file, or give a stale frame count even with the correct byte size.
#[cfg(windows)]
mod cache;

use super::codecs;
use crate::resample::Resampler;
use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::sync::Arc;
use symphonia::core::audio::Channels;
use symphonia::core::codecs::audio::well_known::CODEC_ID_MP3;
use symphonia::core::codecs::audio::{AudioCodecParameters, AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::{Error, Result};
use symphonia::core::formats::Track;
use symphonia::core::io::MediaSource;
use symphonia::core::packet::Packet;

#[derive(Clone, Copy)]
pub(super) struct Length {
    pub frames: u64,
    pub mixed: bool,
}

/// The demuxer searches for MPEG sync in stray bytes. Give it a seekable stream which
/// omits embedded ID3 tags altogether, including artwork which resembles MPEG audio.
pub(super) struct AudioBytes {
    file: File,
    index: Arc<Index>,
    pos: u64,
}

impl MediaSource for AudioBytes {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        Some(self.index.audio_bytes)
    }
}

impl Seek for AudioBytes {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let pos = match from {
            SeekFrom::Start(n) => Some(n),
            SeekFrom::Current(n) => self.pos.checked_add_signed(n),
            SeekFrom::End(n) => self.index.audio_bytes.checked_add_signed(n),
        }
        .ok_or(io::ErrorKind::InvalidInput)?;
        let cuts = &self.index.cuts;
        let i = cuts.partition_point(|&(at, _)| at <= pos);
        let removed = i.checked_sub(1).map_or(0, |i| cuts[i].1);
        self.file.seek(SeekFrom::Start(
            pos.checked_add(removed)
                .ok_or(io::ErrorKind::InvalidInput)?,
        ))?;
        self.pos = pos;
        Ok(pos)
    }
}

impl Read for AudioBytes {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let cuts = &self.index.cuts;
        let i = cuts.partition_point(|&(at, _)| at <= self.pos);
        let end = cuts.get(i).map_or(self.index.audio_bytes, |c| c.0);
        let count = end.saturating_sub(self.pos).min(buf.len() as u64) as usize;
        let got = self.file.read(&mut buf[..count])?;
        self.pos += got as u64;
        if got > 0 && cuts.get(i).is_some_and(|c| c.0 == self.pos) {
            self.seek(SeekFrom::Start(self.pos))?;
        }
        Ok(got)
    }
}

struct Header {
    spec: Signal,
    samples: u64,
    size: usize,
    side_end: usize,
}

impl Header {
    fn parse(h: &[u8; 4]) -> Option<Self> {
        if h[0] != 0xff || h[1] & 0xe6 != 0xe2 {
            return None;
        }
        let (spec, samples) = signal_header(h).ok()?;
        let index = usize::from(h[2] >> 4);
        let bitrates = if samples == 1152 {
            [
                0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
            ]
        } else {
            [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160]
        };
        let kbps = *bitrates.get(index)?;
        if kbps == 0 {
            return None;
        }
        let size =
            (samples * 125 * kbps / u64::from(spec.rate)) as usize + usize::from((h[2] >> 1) & 1);
        let side_end = 4 + match (samples == 1152, spec.mono) {
            (true, false) => 32,
            (false, true) => 9,
            _ => 17,
        };
        Some(Self {
            spec,
            samples,
            size,
            side_end,
        })
    }
}

struct Index {
    length: Length,
    // Logical offset where each tag was removed and cumulative bytes removed there.
    cuts: Box<[(u64, u64)]>,
    audio_bytes: u64,
}

pub(super) fn length(mut file: File, track: &Track, rate: u32) -> Result<(Length, AudioBytes)> {
    #[cfg(windows)]
    let index = cache::index(&mut file, track, rate)?;
    #[cfg(not(windows))]
    let index = {
        let len = file.metadata()?.len();
        Arc::new(scan(&mut file, len, track, rate)?)
    };
    let mut audio = AudioBytes {
        file,
        index,
        pos: 0,
    };
    audio.rewind()?;
    Ok((audio.index.length, audio))
}

fn scan(file: &mut File, len: u64, track: &Track, rate: u32) -> Result<Index> {
    match scan_headers(file, len, track, rate, 64 * 1024) {
        // A byte-range lock on audio data may overlap read-ahead. Retry with exact
        // header reads so a locked frame body need not be touched.
        #[cfg(windows)]
        Err(Error::IoError(e)) if e.raw_os_error() == Some(33) => {
            scan_headers(file, len, track, rate, 0)
        }
        result => result,
    }
}

fn scan_headers(
    file: &mut File,
    len: u64,
    track: &Track,
    rate: u32,
    capacity: usize,
) -> Result<Index> {
    let mut r = BufReader::with_capacity(capacity, file);
    r.rewind()?;
    let mut pos = 0;
    let mut tags = Vec::new();
    let mut run = None;
    let mut samples = 0;
    let mut converted = 0;
    let mut mixed = false;
    let mut delay = u64::from(track.delay.unwrap_or(0));
    while len - pos >= 4 {
        let mut h = [0; 4];
        r.read_exact(&mut h)?;
        if &h[..3] == b"TAG" && len - pos >= 128 {
            tags.push(pos..pos + 128);
            r.seek_relative(124)?;
            pos += 128;
            continue;
        }
        if &h[..3] == b"ID3" && (2..=4).contains(&h[3]) && len - pos >= 10 {
            let mut tail = [0; 6];
            r.read_exact(&mut tail)?;
            if tail[0] != 255 && tail[2..].iter().all(|b| *b < 128) {
                let size = tail[2..].iter().fold(0u64, |n, b| n * 128 + u64::from(*b));
                let end = (pos
                    + 10
                    + size
                    + if h[3] == 4 && tail[1] & 16 != 0 {
                        10
                    } else {
                        0
                    })
                .min(len);
                tags.push(pos..end);
                r.seek_relative((end - pos - 10) as i64)?;
                pos = end;
                continue;
            }
            r.seek_relative(-6)?;
        }
        let Some(header) = Header::parse(&h).filter(|h| h.size as u64 <= len - pos) else {
            r.seek_relative(-3)?;
            pos += 1;
            continue;
        };
        // The small prefix distinguishes audio from Xing/Info/VBRI. Buffering amortizes
        // disk seeks for tightly spaced headers in streams without a verifiable index.
        let mut prefix = [0u8; 40];
        prefix[..4].copy_from_slice(&h);
        let n = header.size.min(prefix.len());
        r.read_exact(&mut prefix[4..n])?;
        let at = header.side_end;
        let side_start = if h[1] & 1 == 0 { 6 } else { 4 };
        let xing = matches!(prefix.get(at..at + 4), Some(b"Xing" | b"Info"))
            && prefix[side_start..at].iter().all(|&v| v == 0);
        let vbri = prefix[36..] == *b"VBRI" && prefix[side_start..36].iter().all(|&v| v == 0);
        // A Xing/LAME CRC covers only its header. Count the following MPEG headers,
        // even if the advertised byte span ends exactly at EOF.
        let mut metadata = xing;
        if vbri && header.size >= 62 {
            let mut version = [0; 2];
            r.read_exact(&mut version)?;
            metadata = version == [0, 1];
            r.seek_relative((header.size - n - 2) as i64)?;
        } else {
            r.seek_relative((header.size - n) as i64)?;
        }
        pos += header.size as u64;
        if metadata {
            continue;
        }
        if let Some(previous) = run
            && previous != header.spec
        {
            mixed = true;
            converted += rescale(samples, previous, rate);
            samples = 0;
        }
        run = Some(header.spec);
        let n = header.samples;
        let drop = delay.min(n);
        samples += n - drop;
        delay -= drop;
    }
    let spec = run.ok_or(Error::DecodeError("no MP3 audio frames"))?;
    let end = converted + rescale(samples, spec, rate);
    let length = Length { frames: end, mixed };
    let mut removed = 0;
    let cuts = tags
        .into_iter()
        .map(|tag| {
            let at = tag.start - removed;
            removed += tag.end - tag.start;
            (at, removed)
        })
        .collect();
    Ok(Index {
        length,
        cuts,
        audio_bytes: len - removed,
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
    signal_header(h)
}

fn signal_header(h: &[u8]) -> Result<(Signal, u64)> {
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

#[cfg(test)]
mod tests {
    use super::super::Source;
    use std::io::Write;

    // At 48 kHz a stereo 128 kb/s frame is 384 bytes, a 64 kb/s frame is 192.
    fn song(name: &str, tagged: bool) -> std::path::PathBuf {
        use symphonia::core::{checksum::Crc16AnsiLe, io::Monitor};
        let path = std::env::temp_dir().join(format!("string-{}-{name}.mp3", std::process::id()));
        let mut file = std::fs::File::create(&path).unwrap();
        if tagged {
            let mut tag = [0; 384];
            tag[..4].copy_from_slice(&[0xff, 0xfb, 0x94, 0]);
            tag[36..40].copy_from_slice(b"Info");
            tag[43] = 3;
            tag[44..48].copy_from_slice(&100u32.to_be_bytes());
            tag[48..52].copy_from_slice(&(101u32 * 384).to_be_bytes());
            tag[52..61].copy_from_slice(b"LAME3.100");
            let mut crc = Crc16AnsiLe::new(0);
            crc.process_buf_bytes(&tag[..86]);
            tag[86..88].copy_from_slice(&crc.crc().to_be_bytes());
            file.write_all(&tag).unwrap();
        }
        let mut frame = [0; 192];
        frame[..4].copy_from_slice(&[0xff, 0xfb, 0x54, 0]);
        for _ in 0..200 {
            file.write_all(&frame).unwrap();
        }
        path
    }

    #[test]
    fn intact_xing_checksum_and_byte_span_require_a_payload_count() {
        let path = song("same-span", true);
        let mut source = Source::open(&path).unwrap();
        let frames = source.info().frames.unwrap();
        let mut decoded = Vec::new();
        while source.read(&mut decoded, 4096).unwrap() != 0 {}
        assert_eq!(
            frames,
            200 * 1152 - 529,
            "200 audio frames less decoder delay"
        );
        assert_eq!(frames, decoded.len() as u64 / 2);
        drop(source);
        std::fs::remove_file(path).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn finished_mp3_allows_an_exclusive_tag_editor() {
        use std::os::windows::fs::OpenOptionsExt;
        let path = song("exclusive", false);
        let mut source = Source::open(&path).unwrap();
        while source.read(&mut Vec::new(), 4096).unwrap() != 0 {}
        drop(source);
        drop(
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .share_mode(0)
                .open(&path)
                .unwrap(),
        );
        std::fs::remove_file(path).unwrap();
    }
}
