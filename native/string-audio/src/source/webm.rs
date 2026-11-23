//! Matroska's demuxer omits BlockGroup/DiscardPadding and rounds timestamps to track
//! ticks. Count Opus samples on their own clock and trim each padded block on its track.
use super::{Error, NativeSource, next_packet, probe_media};
use std::fs::File;
use std::io::{self, BufReader, Read, Seek};
use symphonia::core::codecs::audio::well_known::{CODEC_ID_OPUS, CODEC_ID_VORBIS};
use symphonia::core::formats::{SeekMode, SeekTo, probe::Hint};
use symphonia::core::units::Timestamp;

const SEGMENT: u64 = 0x18538067;
const CLUSTER: u64 = 0x1f43b675;
const TRACKS: u64 = 0x1654ae6b;
const CUES: u64 = 0x1c53bb6b;
const SEEK_HEAD: u64 = 0x114d9b74;
const SIMPLE_BLOCK: u64 = 0xa3;
const BLOCK_GROUP: u64 = 0xa0;
const BLOCK: u64 = 0xa1;
const DISCARD_PADDING: u64 = 0x75a2;

fn decoding(err: impl std::fmt::Display) -> Error {
    Error::Decode(err.to_string())
}

pub(super) fn seek(
    s: &mut NativeSource,
    previous: Option<i64>,
    preroll_start: u64,
    ts: i64,
) -> Result<(), Error> {
    let w = s.webm.as_ref().unwrap();
    let codec = s.decoder.codec_params().codec;
    let start = -(w.opus_delay.unwrap_or(0) as i64);
    let target = preroll_start as i64;
    let mut cursor = previous;
    // A nearby forward seek already has the decoder history it needs.
    if let Some(at) = cursor
        && (target..=s.skip_to as i64).contains(&at)
        && s.decoder.last_decoded().frames() != 0
    {
        s.cursor = Some(at);
        return Ok(());
    }
    // Cues may be present after the first cluster without a SeekHead pointing to
    // them. Trust the demuxer's actual landing and retry a failed or late seek.
    // After an interior trim the playable frame clock can differ from the cue timestamps.
    if w.cues && !w.padding.interior && ts > 0 {
        let landing = s.format.seek(
            SeekMode::Accurate,
            SeekTo::Timestamp {
                ts: Timestamp::new(ts),
                track_id: s.track_id,
            },
        );
        if let Ok(at) = landing {
            let delta = s.ticks_to_frames(at.actual_ts.get().saturating_sub(ts));
            if delta.unsigned_abs() <= u64::from(s.rate) / 8 {
                s.decoder.reset();
                return Ok(());
            }
        }
        cursor = None;
    }
    if cursor.is_none_or(|at| at > target) || (preroll_start == 0 && start < 0) {
        let w = s.webm.as_mut().unwrap();
        let file = &mut w.file;
        file.rewind().map_err(decoding)?;
        s.format = probe_media(&Hint::new(), Box::new(file.try_clone().map_err(decoding)?))
            .map_err(decoding)?;
        s.decoder.reset();
        w.padding.raw = 0;
        cursor = Some(start);
    }
    let mut at = cursor.unwrap();
    // Keep the decoder and its overlap if the current packet is already inside
    // pre-roll. At a fresh start the first packet also carries the codec delay.
    if preroll_start == 0 || at >= target {
        s.cursor = Some(at);
        return Ok(());
    }
    if codec != CODEC_ID_OPUS && codec != CODEC_ID_VORBIS {
        s.format
            .seek(
                SeekMode::Accurate,
                SeekTo::Timestamp {
                    ts: Timestamp::new(ts),
                    track_id: s.track_id,
                },
            )
            .map_err(decoding)?;
        s.decoder.reset();
        return Ok(());
    }
    // Vorbis overlaps a quarter of each adjacent window. Its mode occupies at
    // most six bits after the audio flag. Measure a previously unseen prefix
    // with two identical windows; subsequent packets need only their first byte.
    let mut half_windows = [0; 128];
    let mut previous_half = None;
    let mut primer = None;
    while at < target {
        let Some(packet) = next_packet(s.format.as_mut()).map_err(decoding)? else {
            s.done = true;
            return Ok(());
        };
        if packet.track_id != s.track_id {
            continue;
        }
        if codec == CODEC_ID_OPUS {
            let n = opus_frames(&packet.data, s.rate)?;
            let (lo, hi) = s.webm.as_mut().unwrap().padding.keep(n);
            at += (hi - lo) as i64;
        } else {
            let first = packet
                .data
                .first()
                .ok_or_else(|| decoding("empty Vorbis packet"))?;
            let half = &mut half_windows[usize::from(first & 127)];
            if previous_half.is_none() {
                at += s.decoder.decode(&packet).map_err(decoding)?.frames() as i64;
            }
            if *half == 0 {
                s.decoder.reset();
                s.decoder.decode(&packet).map_err(decoding)?;
                *half = s.decoder.decode(&packet).map_err(decoding)?.frames() as i64;
            }
            if let Some(prev) = previous_half {
                at += (prev + *half) / 2;
            }
            previous_half = Some(*half);
            primer = Some(packet);
        }
    }
    s.decoder.reset();
    if let Some(packet) = primer {
        s.decoder.decode(&packet).map_err(decoding)?;
    }
    s.cursor = Some(at);
    Ok(())
}

pub(super) struct Layout {
    pub cues: bool,
    pub samples: u64,
    pub delay_ns: Option<u64>,
    // Ranges on the untrimmed 48 kHz Opus clock, including laced packets.
    discard: Vec<BlockPadding>,
}

impl Layout {
    /// Convert each discard separately to the output frame clock. The
    /// prefix and suffix of a laced block can span more than one decoded packet.
    pub(super) fn padding(&self, rate: u32) -> Padding {
        let scale = |sample: u64| (u128::from(sample) * u128::from(rate) / 48000) as u64;
        let cuts: Vec<std::ops::Range<u64>> = self
            .discard
            .iter()
            .filter_map(|r| {
                let count = ns_to_frames(r.nanoseconds.unsigned_abs(), rate);
                let block = scale(r.first)..scale(r.end);
                let length = count.min(block.end - block.start);
                let cut = if r.nanoseconds > 0 {
                    block.end - length..block.end
                } else {
                    block.start..block.start + length
                };
                (cut.start < cut.end).then_some(cut)
            })
            .collect();
        let interior = match cuts.as_slice() {
            [] => false,
            [cut] => scale(self.samples) != cut.end,
            _ => true,
        };
        Padding {
            cuts,
            raw: 0,
            interior,
        }
    }
}

struct BlockPadding {
    first: u64,
    end: u64,
    nanoseconds: i64,
}

pub(super) struct Padding {
    cuts: Vec<std::ops::Range<u64>>,
    raw: u64,
    pub(super) interior: bool,
}

impl Padding {
    pub(super) fn total(&self) -> u64 {
        self.cuts.iter().map(|r| r.end - r.start).sum()
    }

    pub(super) fn before(&self, frame: u64) -> u64 {
        self.cuts
            .iter()
            .take_while(|r| r.start < frame)
            .map(|r| r.end.min(frame) - r.start)
            .sum()
    }

    /// Discard padding belongs to one block. Each packet can therefore overlap
    /// at most one cut, including when the cut consumes an entire lace.
    pub(super) fn keep(&mut self, frames: usize) -> (usize, usize) {
        // The source length already trims a lone terminal cut, including after
        // a cue seek where the raw packet cursor need not be reconstructed.
        if !self.interior {
            return (0, frames);
        }
        let start = self.raw;
        self.raw += frames as u64;
        let i = self.cuts.partition_point(|r| r.end <= start);
        match self.cuts.get(i) {
            Some(r) if r.start <= start => ((r.end.min(self.raw) - start) as usize, frames),
            Some(r) if r.start < self.raw => (0, (r.start - start) as usize),
            _ => (0, frames),
        }
    }
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid WebM element")
}

struct Reader {
    r: BufReader<File>,
    pos: u64,
}

impl Reader {
    fn uint(&mut self, end: u64) -> io::Result<u64> {
        let n = (end - self.pos) as usize;
        if n > 8 {
            return Err(invalid());
        }
        let mut bytes = [0; 8];
        self.bytes(&mut bytes[8 - n..], end)?;
        Ok(u64::from_be_bytes(bytes))
    }

    fn cues_position(&mut self, end: u64) -> io::Result<Option<u64>> {
        while self.pos < end {
            let (id, entry, _) = self.element(end)?;
            if id == 0x4dbb {
                let (mut target, mut position) = (None, None);
                while self.pos < entry {
                    let (id, next, _) = self.element(entry)?;
                    match id {
                        0x53ab if (1..=4).contains(&(next - self.pos)) => {
                            target = Some(self.uint(next)?)
                        }
                        0x53ac if (1..=8).contains(&(next - self.pos)) => {
                            position = Some(self.uint(next)?)
                        }
                        _ => {}
                    }
                    self.to(next)?;
                }
                if target == Some(CUES) && position.is_some() {
                    return Ok(position);
                }
            }
            self.to(entry)?;
        }
        Ok(None)
    }
    fn bytes(&mut self, buf: &mut [u8], end: u64) -> io::Result<()> {
        if buf.len() as u64 > end.saturating_sub(self.pos) {
            return Err(invalid());
        }
        self.r.read_exact(buf)?;
        self.pos += buf.len() as u64;
        Ok(())
    }

    fn to(&mut self, pos: u64) -> io::Result<()> {
        let delta = i64::try_from(i128::from(pos) - i128::from(self.pos)).map_err(|_| invalid())?;
        self.r.seek_relative(delta)?;
        self.pos = pos;
        Ok(())
    }

    fn vint(&mut self, end: u64, id: bool) -> io::Result<(u64, u32)> {
        let mut buf = [0; 1];
        self.bytes(&mut buf, end)?;
        let width = buf[0].leading_zeros() + 1;
        if width > if id { 4 } else { 8 } {
            return Err(invalid());
        }
        let mut n = u64::from(if id {
            buf[0]
        } else {
            buf[0] & (0xffu16 >> width) as u8
        });
        for _ in 1..width {
            self.bytes(&mut buf, end)?;
            n = (n << 8) | u64::from(buf[0]);
        }
        Ok((n, width))
    }

    fn element(&mut self, end: u64) -> io::Result<(u64, u64, bool)> {
        let (id, _) = self.vint(end, true)?;
        let (size, width) = self.vint(end, false)?;
        let unknown = size == (1u64 << (7 * width)) - 1;
        if unknown {
            return Ok((id, end, true));
        }
        if size > end - self.pos {
            return Err(invalid());
        }
        Ok((id, self.pos + size, false))
    }

    fn count_packet(&mut self, end: u64, samples: &mut u64) -> io::Result<()> {
        let mut head = [0; 2];
        let n = (end - self.pos).min(2) as usize;
        self.bytes(&mut head[..n], end)?;
        let count = packet_samples(&head[..n]).ok_or_else(invalid)?;
        *samples = samples.checked_add(count).ok_or_else(invalid)?;
        Ok(())
    }

    fn block(&mut self, end: u64, track: u32, layout: &mut Layout) -> io::Result<()> {
        if self.vint(end, false)?.0 != u64::from(track) {
            return Ok(());
        }
        let mut fields = [0; 3];
        self.bytes(&mut fields, end)?;
        let lacing = (fields[2] >> 1) & 3;
        if lacing == 0 {
            return self.count_packet(end, &mut layout.samples);
        }
        // The lace header specifies all packet sizes except the last.
        let mut sizes = [0u64; 255];
        let mut n = [0];
        self.bytes(&mut n, end)?;
        let sizes = &mut sizes[..usize::from(n[0])];
        let count = sizes.len() as u64 + 1;
        if lacing == 2 && !(end - self.pos).is_multiple_of(count) {
            return Err(invalid());
        }
        let mut previous = 0u64;
        for size in sizes.iter_mut() {
            *size = match lacing {
                1 => {
                    let mut total = 0;
                    loop {
                        self.bytes(&mut n, end)?;
                        total += u64::from(n[0]);
                        if n[0] != 255 {
                            break total;
                        }
                    }
                }
                2 => (end - self.pos) / count,
                _ => {
                    let (v, width) = self.vint(end, false)?;
                    if previous == 0 {
                        v
                    } else {
                        let bias = (1i64 << (7 * width - 1)) - 1;
                        previous
                            .checked_add_signed(v as i64 - bias)
                            .ok_or_else(invalid)?
                    }
                }
            };
            if *size == 0 {
                return Err(invalid());
            }
            previous = *size;
        }
        for &size in sizes.iter() {
            let next = self
                .pos
                .checked_add(size)
                .filter(|&n| n <= end)
                .ok_or_else(invalid)?;
            self.count_packet(next, &mut layout.samples)?;
            self.to(next)?;
        }
        self.count_packet(end, &mut layout.samples)
    }

    fn track_delay(&mut self, end: u64, track: u32) -> io::Result<Option<u64>> {
        while self.pos < end {
            let (id, entry_end, _) = self.element(end)?;
            if id == 0xae {
                let (mut number, mut delay) = (None, None);
                while self.pos < entry_end {
                    let (id, next, _) = self.element(entry_end)?;
                    let width = next - self.pos;
                    if matches!(id, 0xd7 | 0x56aa) && width <= 8 {
                        let mut bytes = [0; 8];
                        self.bytes(&mut bytes[8 - width as usize..], next)?;
                        let value = u64::from_be_bytes(bytes);
                        if id == 0xd7 {
                            number = Some(value)
                        } else {
                            delay = Some(value)
                        }
                    }
                    self.to(next)?;
                }
                if number == Some(u64::from(track)) {
                    return Ok(delay);
                }
            }
            self.to(entry_end)?;
        }
        Ok(None)
    }

    fn group(&mut self, end: u64, track: u32, layout: &mut Layout) -> io::Result<()> {
        let first = layout.samples;
        let mut padding = 0;
        while self.pos < end {
            let (id, next, _) = self.element(end)?;
            match id {
                BLOCK => self.block(next, track, layout)?,
                DISCARD_PADDING if (1..=8).contains(&(next - self.pos)) => {
                    let width = (next - self.pos) as usize;
                    let mut bytes = [0; 8];
                    self.bytes(&mut bytes[..width], next)?;
                    padding = i64::from_be_bytes(bytes) >> (8 * (8 - width));
                }
                _ => {}
            }
            self.to(next)?;
        }
        if padding != 0 && first != layout.samples {
            layout.discard.push(BlockPadding {
                first,
                end: layout.samples,
                nanoseconds: padding,
            });
        }
        Ok(())
    }
}

pub(super) fn ns_to_frames(ns: u64, rate: u32) -> u64 {
    ((u128::from(ns) * u128::from(rate) + 500_000_000) / 1_000_000_000) as u64
}

/// Packet duration at the fixed 48 kHz Opus clock (RFC 6716, section 3.1).
fn packet_samples(data: &[u8]) -> Option<u64> {
    let toc = *data.first()?;
    let config = toc >> 3;
    let frame = match config {
        0..12 => [480, 960, 1920, 2880][usize::from(config & 3)],
        12..16 => [480, 960][usize::from(config & 1)],
        _ => 120 << (config & 3),
    };
    let count = match toc & 3 {
        0 => 1,
        1 | 2 => 2,
        _ => u64::from(data.get(1)? & 63),
    };
    let samples = frame * count;
    (1..=5760).contains(&samples).then_some(samples)
}

pub(super) fn opus_frames(data: &[u8], rate: u32) -> Result<usize, Error> {
    let samples = packet_samples(data).ok_or_else(|| decoding("invalid Opus packet"))?;
    Ok((samples * u64::from(rate) / 48000) as usize)
}

pub(super) fn layout(mut file: File, opus_track: Option<u32>) -> io::Result<Layout> {
    let end = file.metadata()?.len();
    file.rewind()?;
    let mut r = Reader {
        r: BufReader::new(file),
        pos: 0,
    };
    let mut layout = Layout {
        cues: false,
        samples: 0,
        delay_ns: None,
        discard: Vec::new(),
    };
    while r.pos < end {
        let (id, segment_end, _) = r.element(end)?;
        if id != SEGMENT {
            r.to(segment_end)?;
            continue;
        }
        let segment_data = r.pos;
        let mut first_cluster_seen = false;
        let mut cues_position = None;
        while r.pos < segment_end {
            let element_start = r.pos;
            let (id, next, unknown) = r.element(segment_end)?;
            if id == SEEK_HEAD && !first_cluster_seen {
                if let Some(pos) = r.cues_position(next)? {
                    cues_position = segment_data.checked_add(pos);
                }
            }
            if id == TRACKS
                && let Some(track) = opus_track
            {
                layout.delay_ns = r.track_delay(next, track)?;
            }
            if id == CUES {
                layout.cues |=
                    next > r.pos && (!first_cluster_seen || Some(element_start) == cues_position);
            }
            first_cluster_seen |= id == CLUSTER;
            if id != CLUSTER || (opus_track.is_none() && !unknown) {
                r.to(next)?;
                continue;
            }
            while r.pos < next {
                let start = r.pos;
                let (id, element_end, _) = r.element(next)?;
                if unknown
                    && matches!(
                        id,
                        CLUSTER
                            | CUES
                            | 0x114d9b74
                            | 0x1549a966
                            | TRACKS
                            | 0x1941a469
                            | 0x1043a770
                            | 0x1254c367
                    )
                {
                    r.to(start)?;
                    break;
                }
                if let Some(track) = opus_track {
                    match id {
                        SIMPLE_BLOCK => r.block(element_end, track, &mut layout)?,
                        BLOCK_GROUP => r.group(element_end, track, &mut layout)?,
                        _ => {}
                    }
                }
                r.to(element_end)?;
            }
        }
        break;
    }
    Ok(layout)
}

#[cfg(test)]
mod tests {
    use super::super::{Source, next_packet};
    use std::path::{Path, PathBuf};
    use std::process::Command;

    struct Song(PathBuf);

    impl Drop for Song {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn song(name: &str, options: &[&str]) -> Option<Song> {
        let path = std::env::temp_dir().join(format!("string-{}-{name}.webm", std::process::id()));
        let encoder = std::env::var_os("FFMPEG").unwrap_or_else(|| "ffmpeg".into());
        let result = Command::new(encoder)
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=417:duration=4:sample_rate=48000",
                "-c:a",
                "libopus",
            ])
            .args(options)
            .arg(&path)
            .output()
            .ok()?;
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        Some(Song(path))
    }

    fn pcm(path: &Path) -> Vec<f32> {
        let mut s = Source::open(path).unwrap();
        let mut out = Vec::new();
        while s.read(&mut out, 8192).unwrap() != 0 {}
        out
    }

    #[test]
    fn cueless_seek_skips_compressed_prefix_before_decoding_preroll() {
        let Some(song) = song("skip-packets", &["-live", "1"]) else {
            return;
        };
        let mut s = Source::open(&song.0).unwrap();
        s.seek(3 * 48000).unwrap();
        let Source::Native(s) = &mut s else {
            panic!("expected native source")
        };
        let packet = next_packet(s.format.as_mut()).unwrap().unwrap();
        assert!(
            (2100..=2700).contains(&packet.pts.get()),
            "first decoder packet at {} ms",
            packet.pts.get()
        );
    }

    #[test]
    fn trailing_unreferenced_cues_can_seek_back_after_eof() {
        let Some(song) = song("unreferenced-cues", &[]) else {
            return;
        };
        // Keep the muxer's real Cues and all offsets intact. Remove only its
        // SeekID from the leading SeekHead, as on a streaming recorder.
        let mut bytes = std::fs::read(&song.0).unwrap();
        let id = [0x53, 0xab, 0x84, 0x1c, 0x53, 0xbb, 0x6b];
        let at = bytes.windows(id.len()).position(|v| v == id).unwrap();
        bytes[at + 6] = 0x6c;
        std::fs::write(&song.0, bytes).unwrap();
        assert!(
            !super::layout(std::fs::File::open(&song.0).unwrap(), Some(1))
                .unwrap()
                .cues
        );
        let mut s = Source::open(&song.0).unwrap();
        let mut reference = Vec::new();
        while s.read(&mut reference, 8192).unwrap() != 0 {}
        s.seek(97003).unwrap();
        let mut sought = Vec::new();
        assert_eq!(s.read(&mut sought, 1024).unwrap(), 1024);
        let expected = &reference[194006..196054];
        let max_error = sought
            .iter()
            .zip(expected)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(
            sought == expected,
            "first {} vs {}, max error {max_error}",
            sought[0],
            expected[0]
        );
    }

    #[test]
    fn discard_padding_removes_the_declaring_blocks_pcm() {
        let Some(song) = song("block-trim", &[]) else {
            return;
        };
        let reference = pcm(&song.0);
        let list = Song(song.0.with_extension("ffconcat"));
        let joined = Song(song.0.with_extension("joined.webm"));
        let filename = song.0.file_name().unwrap().to_str().unwrap();
        std::fs::write(&list.0, format!("file '{filename}'\nfile '{filename}'\n")).unwrap();
        let encoder = std::env::var_os("FFMPEG").unwrap_or_else(|| "ffmpeg".into());
        let result = Command::new(encoder)
            .args(["-v", "error", "-y", "-f", "concat", "-i"])
            .arg(&list.0)
            .args(["-c", "copy"])
            .arg(&joined.0)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );

        let heard = pcm(&joined.0);
        let joint = reference.len() / 2;
        let mut source = Source::open(&joined.0).unwrap();
        assert_eq!(source.info().frames, Some(384312));
        for at in [joint - 1, joint + 96000, joint + 1, 384311, joint, 0] {
            source.seek(at as u64).unwrap();
            let mut sought = Vec::new();
            let frames = source.read(&mut sought, 512).unwrap();
            assert_eq!(frames, 512.min(384312 - at));
            assert!(sought == heard[at * 2..(at + frames) * 2], "seek to {at}");
        }
        drop(source);
        let mut bytes = std::fs::read(&joined.0).unwrap();
        // Remove only the first 13.5 ms DiscardPadding, leaving the final trim
        // and all other bytes intact. Each Opus packet in the fixture is 960 frames.
        let pad = [0x75, 0xa2, 0x84, 0x00, 0xcd, 0xfe, 0x60];
        let groups: Vec<_> = bytes
            .windows(pad.len())
            .enumerate()
            .filter(|(_, w)| *w == pad)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(groups.len(), 2);
        bytes[groups[0] + 3..groups[0] + 7].fill(0);
        std::fs::write(&joined.0, bytes).unwrap();
        let mut expected = pcm(&joined.0);
        expected.drain(joint * 2..(joint + 648) * 2);
        assert_eq!(heard.len(), expected.len());
        assert!(
            heard == expected,
            "interior padding was removed from the wrong packet"
        );
    }

    #[test]
    fn signed_padding_spans_the_laces_of_its_block_only() {
        let path =
            Song(std::env::temp_dir().join(format!("string-{}-lace.webm", std::process::id())));
        // Each block contains 960, 480 and 960 frames. Xiph specifies the
        // first two byte lengths directly (3, 2), EBML encodes 3 then -1.
        for lace in [
            &[4, 2, 0xf8, 0xf0, 0xf8][..],
            &[2, 2, 3, 2, 0xf8, 0, 0, 0xf0, 0, 0xf8],
            &[6, 2, 0x83, 0xbe, 0xf8, 0, 0, 0xf0, 0, 0xf8],
        ] {
            let mut bytes = vec![0x18, 0x53, 0x80, 0x67, 0xff, 0x1f, 0x43, 0xb6, 0x75, 0xff];
            let block_len = 3 + lace.len() as u8;
            for ns in [-25_000_000i32, 25_000_000] {
                bytes.extend([
                    0xa0,
                    0x80 | (block_len + 9),
                    0xa1,
                    0x80 | block_len,
                    0x81,
                    0,
                    0,
                ]);
                bytes.extend_from_slice(lace);
                bytes.extend([0x75, 0xa2, 0x84]);
                bytes.extend(ns.to_be_bytes());
            }
            std::fs::write(&path.0, bytes).unwrap();
            let layout = super::layout(std::fs::File::open(&path.0).unwrap(), Some(1)).unwrap();
            assert_eq!(layout.samples, 4800);
            let mut trim = layout.padding(48000);
            assert_eq!(trim.total(), 2400);
            assert_eq!(trim.before(312), 312);
            assert_eq!(
                [960, 480, 960, 960, 480, 960].map(|frames| trim.keep(frames)),
                [
                    (960, 960),
                    (240, 480),
                    (0, 960),
                    (0, 960),
                    (0, 240),
                    (960, 960)
                ]
            );
        }
    }
}
