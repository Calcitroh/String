//! Count MPEG audio frames before publishing the duration. The first header can describe only
//! the first part of a joined file, or give a stale frame count even with the correct byte size.
use super::codecs;
use crate::resample::Resampler;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::sync::Arc;
use symphonia::core::audio::Channels;
use symphonia::core::checksum::Crc16AnsiLe;
use symphonia::core::codecs::audio::well_known::CODEC_ID_MP3;
use symphonia::core::codecs::audio::{AudioCodecParameters, AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::{Error, Result};
use symphonia::core::formats::Track;
use symphonia::core::io::{MediaSource, Monitor};
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
    // Logical offset where each tag was removed and cumulative bytes removed there.
    cuts: Arc<[(u64, u64)]>,
    pos: u64,
    len: u64,
}

impl MediaSource for AudioBytes {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        Some(self.len)
    }
}

impl Seek for AudioBytes {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let pos = match from {
            SeekFrom::Start(n) => Some(n),
            SeekFrom::Current(n) => self.pos.checked_add_signed(n),
            SeekFrom::End(n) => self.len.checked_add_signed(n),
        }
        .ok_or(io::ErrorKind::InvalidInput)?;
        let i = self.cuts.partition_point(|&(at, _)| at <= pos);
        let removed = i.checked_sub(1).map_or(0, |i| self.cuts[i].1);
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
        let i = self.cuts.partition_point(|&(at, _)| at <= self.pos);
        let end = self.cuts.get(i).map_or(self.len, |c| c.0);
        let count = end.saturating_sub(self.pos).min(buf.len() as u64) as usize;
        let got = self.file.read(&mut buf[..count])?;
        self.pos += got as u64;
        if got > 0 && self.cuts.get(i).is_some_and(|c| c.0 == self.pos) {
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

// A valid checksum describes the original segment. If other bytes follow it, verify
// the claimed join too: a truncated first copy can otherwise borrow audio packets
// from the second while its original header checksum is still intact.
fn segment_boundary(
    r: &mut BufReader<&mut File>,
    next: u64,
    len: u64,
    resume: u64,
) -> io::Result<bool> {
    if next == len {
        return Ok(true);
    }
    r.seek(SeekFrom::Start(next))?;
    let mut prefix = [0; 40];
    let n = (len - next).min(40) as usize;
    r.read_exact(&mut prefix[..n])?;
    r.seek(SeekFrom::Start(resume))?;
    if matches!(prefix.get(..3), Some(b"ID3" | b"TAG")) {
        return Ok(true);
    }
    let Some(header) = Header::parse(prefix[..4].try_into().unwrap()) else {
        return Ok(false);
    };
    Ok(matches!(
        prefix.get(header.side_end..header.side_end + 4),
        Some(b"Xing" | b"Info")
    ) || &prefix[36..40] == b"VBRI")
}

fn be32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

// Only jump over an entire segment when the encoder checksum covers both the frame
// count and byte span. A byte count alone cannot detect a stale duration.
fn verified_span(frame: &[u8], at: usize) -> Option<(u64, u64)> {
    let flags = be32(frame, at + 4)?;
    if flags & 3 != 3 || flags & !15 != 0 {
        return None;
    }
    let frames = be32(frame, at + 8)?;
    let bytes = be32(frame, at + 12)?;
    let ext = at + 16 + if flags & 4 != 0 { 100 } else { 0 } + if flags & 8 != 0 { 4 } else { 0 };
    if !matches!(frame.get(ext..ext + 4), Some(b"LAME" | b"Lavc" | b"Lavf")) {
        return None;
    }
    let crc_at = ext + 34;
    let expected = u16::from_be_bytes(frame.get(crc_at..crc_at + 2)?.try_into().ok()?);
    let mut crc = Crc16AnsiLe::new(0);
    crc.process_buf_bytes(&frame[..crc_at]);
    (frames != 0 && bytes as usize > frame.len() && expected != 0 && crc.crc() == expected)
        .then_some((u64::from(bytes), u64::from(frames)))
}

#[derive(Clone)]
struct Index {
    length: Length,
    cuts: Arc<[(u64, u64)]>,
    audio_bytes: u64,
}

#[derive(PartialEq)]
struct Stamp {
    volume_and_id: [u64; 3],
    change_time: i64,
    write_time: i64,
    bytes: u64,
}

#[cfg(windows)]
fn stamp(file: &File, bytes: u64) -> Option<Stamp> {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFileInformationByHandleEx(
            file: *mut c_void,
            class: i32,
            buffer: *mut c_void,
            len: u32,
        ) -> i32;
    }
    // FILE_BASIC_INFO is four LARGE_INTEGERs followed by attributes and alignment
    // padding. FILE_ID_INFO is a volume serial and a 128-bit file ID.
    let mut basic = [0i64; 5];
    let mut id = [0u64; 3];
    let ok = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            0,
            basic.as_mut_ptr().cast(),
            size_of_val(&basic) as u32,
        ) != 0
            && GetFileInformationByHandleEx(
                file.as_raw_handle(),
                18,
                id.as_mut_ptr().cast(),
                size_of_val(&id) as u32,
            ) != 0
    };
    (ok && basic[3] != 0 && id[1..] != [0, 0]).then_some(Stamp {
        volume_and_id: id,
        change_time: basic[3],
        write_time: basic[2],
        bytes,
    })
}

#[cfg(not(windows))]
fn stamp(_: &File, _: u64) -> Option<Stamp> {
    None
}

// Loop and successor preparation reuse the most recent verified scans on this feeder.
// Key by the open file identity and change time. Replacing a path or restoring its
// modification time cannot reuse offsets from an older file.
thread_local! {
    static RECENT: RefCell<VecDeque<(Stamp, Index)>> = const { RefCell::new(VecDeque::new()) };
}

pub(super) fn length(mut file: File, track: &Track, rate: u32) -> Result<(Length, AudioBytes)> {
    let bytes = file.metadata()?.len();
    let before = stamp(&file, bytes);
    let cached = before.as_ref().and_then(|key| {
        RECENT.with_borrow(|items| {
            items
                .iter()
                .find(|(s, _)| s == key)
                .map(|(_, index)| index.clone())
        })
    });
    let index = if let Some(index) = cached {
        index
    } else {
        let (index, walked) = scan(&mut file, bytes, track, rate)?;
        if walked
            && let Some(key) = before
            && stamp(&file, file.metadata()?.len()).as_ref() == Some(&key)
        {
            RECENT.with_borrow_mut(|items| {
                items.truncate(3);
                items.push_front((key, index.clone()));
            });
        }
        index
    };
    let mut audio = AudioBytes {
        file,
        cuts: index.cuts,
        pos: 0,
        len: index.audio_bytes,
    };
    audio.rewind()?;
    Ok((index.length, audio))
}

fn scan(file: &mut File, len: u64, track: &Track, rate: u32) -> Result<(Index, bool)> {
    let mut r = BufReader::with_capacity(64 * 1024, file);
    r.rewind()?;
    let mut pos = 0;
    let mut tags = Vec::new();
    let mut walked = false;
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
        let mut span = header.size as u64;
        let mut count = 1;
        if xing || vbri {
            let mut frame = vec![0; header.size];
            frame[..n].copy_from_slice(&prefix[..n]);
            r.read_exact(&mut frame[n..])?;
            if xing {
                count = 0;
                if let Some((bytes, frames)) =
                    verified_span(&frame, at).filter(|(b, _)| *b <= len - pos)
                    && segment_boundary(&mut r, pos + bytes, len, pos + header.size as u64)?
                {
                    span = bytes;
                    count = frames;
                }
            } else if frame.len() >= 62 && frame[40..42] == [0, 1] {
                count = 0;
            }
            r.seek_relative((span - header.size as u64) as i64)?;
        } else {
            walked = true;
            // Adjacent audio headers are usually less than 1 KiB apart. Advance
            // through the read-ahead buffer without copying the body or breaking
            // sequential I/O every time a frame straddles two buffers.
            let mut skip = header.size - n;
            while skip != 0 {
                let chunk = skip.min(r.fill_buf()?.len());
                if chunk == 0 {
                    return Err(io::Error::from(io::ErrorKind::UnexpectedEof).into());
                }
                r.consume(chunk);
                skip -= chunk;
            }
        }
        pos += span;
        if count == 0 {
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
        let n = count * header.samples;
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
    Ok((
        Index {
            length,
            cuts,
            audio_bytes: len - removed,
        },
        walked,
    ))
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
