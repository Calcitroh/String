//! Matroska's demuxer omits BlockGroup/DiscardPadding and rounds timestamps to track
//! ticks. Count Opus samples on their own clock and associate tail padding with its track.
use std::fs::File;
use std::io::{self, BufReader, Read, Seek};

const SEGMENT: u64 = 0x18538067;
const CLUSTER: u64 = 0x1f43b675;
const TRACKS: u64 = 0x1654ae6b;
const CUES: u64 = 0x1c53bb6b;
const SIMPLE_BLOCK: u64 = 0xa3;
const BLOCK_GROUP: u64 = 0xa0;
const BLOCK: u64 = 0xa1;
const DISCARD_PADDING: u64 = 0x75a2;

pub(super) struct Layout {
    pub cues: bool,
    // Laced blocks defer counting to the demuxer, which splits them into packets.
    pub samples: Option<u64>,
    pub delay_ns: Option<u64>,
    pub tail_ns: i64,
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid WebM element")
}

struct Reader {
    r: BufReader<File>,
    pos: u64,
}

impl Reader {
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

    fn block(&mut self, end: u64, track: u32, layout: &mut Layout) -> io::Result<bool> {
        if self.vint(end, false)?.0 != u64::from(track) {
            return Ok(false);
        }
        let mut fields = [0; 3];
        self.bytes(&mut fields, end)?;
        if let Some(total) = layout.samples {
            if fields[2] & 6 == 0 {
                let mut head = [0; 2];
                let count = (end - self.pos).min(2) as usize;
                self.bytes(&mut head[..count], end)?;
                layout.samples = packet_samples(&head[..count]).and_then(|n| total.checked_add(n));
            } else {
                layout.samples = None;
            }
        }
        Ok(true)
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
        let mut audio = false;
        let mut padding = 0;
        while self.pos < end {
            let (id, next, _) = self.element(end)?;
            match id {
                BLOCK => audio = self.block(next, track, layout)?,
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
        if audio {
            layout.tail_ns = padding;
        }
        Ok(())
    }
}

pub(super) fn ns_to_frames(ns: u64, rate: u32) -> u64 {
    ((u128::from(ns) * u128::from(rate) + 500_000_000) / 1_000_000_000) as u64
}

/// Packet duration at the fixed 48 kHz Opus clock (RFC 6716, section 3.1).
pub(super) fn packet_samples(data: &[u8]) -> Option<u64> {
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

pub(super) fn layout(mut file: File, opus_track: Option<u32>) -> io::Result<Layout> {
    let end = file.metadata()?.len();
    file.rewind()?;
    let mut r = Reader {
        r: BufReader::new(file),
        pos: 0,
    };
    let mut layout = Layout {
        cues: false,
        samples: Some(0),
        delay_ns: None,
        tail_ns: 0,
    };
    while r.pos < end {
        let (id, segment_end, _) = r.element(end)?;
        if id != SEGMENT {
            r.to(segment_end)?;
            continue;
        }
        while r.pos < segment_end {
            let (id, next, unknown) = r.element(segment_end)?;
            if id == TRACKS
                && let Some(track) = opus_track
            {
                layout.delay_ns = r.track_delay(next, track)?;
            }
            if id == CUES {
                layout.cues = true;
            }
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
                        SIMPLE_BLOCK if r.block(element_end, track, &mut layout)? => {
                            layout.tail_ns = 0
                        }
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
