//! Gapless facts from an MP4/M4A file that the decoder library leaves out: the encoder delay and
//! the real length, from the sound track's edit list (`elst`, how FFmpeg and most encoders write
//! it) or from iTunes' `iTunSMPB` comment. Only the boxes on the way to those are read; big ones
//! (cover art, the sample tables) are skipped with a seek.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

/// Where the playable audio sits in the decoded stream, in frames at the track's sample rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Gapless {
    /// Decoded frames to drop at the start (encoder priming).
    pub delay: u64,
    /// Playable frames after the delay, when the file says.
    pub length: Option<u64>,
}

/// Reads the gapless facts of the first sound track. `None` when the file has none (or isn't MP4).
pub(crate) fn gapless(path: &Path, sample_rate: u32) -> Option<Gapless> {
    let mut file = File::open(path).ok()?;
    let end = file.seek(SeekFrom::End(0)).ok()?;
    let mut found = Found::default();
    walk(&mut file, 0, end, 0, &mut found, &mut None).ok()?;
    found.result(sample_rate)
}

#[derive(Default)]
struct Found {
    movie_scale: Option<u32>,
    /// The first sound track: its media timescale and duration, and first edit.
    track: Option<(u32, Option<u64>, Option<Edit>)>,
    smpb: Option<Gapless>,
}

/// An edit: segment duration (in the movie's timescale), media time (in the track's).
type Edit = (u64, i64);

/// What one `trak` has said so far: its facts sit at different depths (`hdlr` and `mdhd` under
/// `mdia`, `elst` under `edts`).
#[derive(Default)]
struct Trak {
    sound: bool,
    scale: Option<u32>,
    duration: Option<u64>,
    edit: Option<(u64, i64)>,
}

impl Found {
    fn result(&self, rate: u32) -> Option<Gapless> {
        if let Some(g) = self.smpb {
            return Some(g);
        }
        let (media_scale, media_duration, edit) = self.track?;
        let (segment, media_time) = edit?;
        let movie_scale = self.movie_scale.filter(|s| *s > 0)?;
        if media_time < 0 || media_scale == 0 {
            return None;
        }
        let delay = rescale(media_time as u64, media_scale, rate);
        let mut length = rescale(segment, movie_scale, rate);
        // the edit's length is only as fine as the movie's timescale (often milliseconds); the
        // track's own duration (priming plus the playable frames, as encoders write it) is exact,
        // so it wins when the two agree within that coarseness
        if let Some(d) = media_duration
            && d >= media_time as u64
        {
            let exact = rescale(d - media_time as u64, media_scale, rate);
            let grain = (rate as u64).div_ceil(movie_scale as u64) + 1;
            if exact.abs_diff(length) <= grain {
                length = exact;
            }
        }
        if delay == 0 {
            // no priming: the edit only says the length, which the container says too
            return None;
        }
        Some(Gapless {
            delay,
            length: (length > 0).then_some(length),
        })
    }
}

fn rescale(value: u64, from: u32, to: u32) -> u64 {
    ((value as u128 * to as u128 + from as u128 / 2) / from as u128) as u64
}

/// Boxes worth opening on the way to the facts (`trak` and `meta` are handled on their own).
const CONTAINERS: [&[u8; 4]; 8] = [
    b"moov", b"mdia", b"minf", b"stbl", b"edts", b"udta", b"ilst", b"----",
];

/// Leaf boxes read whole: none of the ones wanted is big.
const LEAF_LIMIT: u64 = 64 * 1024;

fn walk(
    f: &mut File,
    start: u64,
    end: u64,
    depth: u32,
    found: &mut Found,
    trak: &mut Option<Trak>,
) -> io::Result<()> {
    if depth > 8 {
        return Ok(());
    }
    let mut at = start;
    // inside a `----` box: its name and its value
    let mut name: Option<String> = None;
    let mut value: Option<String> = None;
    while at + 8 <= end {
        f.seek(SeekFrom::Start(at))?;
        let mut head = [0u8; 8];
        f.read_exact(&mut head)?;
        let mut size = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as u64;
        let kind: [u8; 4] = [head[4], head[5], head[6], head[7]];
        let mut body = at + 8;
        if size == 1 {
            let mut big = [0u8; 8];
            f.read_exact(&mut big)?;
            size = u64::from_be_bytes(big);
            body += 8;
        } else if size == 0 {
            size = end - at;
        }
        if size < body - at || at + size > end {
            break;
        }
        let box_end = at + size;
        if &kind == b"trak" {
            let mut this = Some(Trak::default());
            walk(f, body, box_end, depth + 1, found, &mut this)?;
            if let Some(t) = this
                && t.sound
                && found.track.is_none()
                && let Some(scale) = t.scale
            {
                found.track = Some((scale, t.duration, t.edit));
            }
        } else if CONTAINERS.contains(&&kind) {
            walk(f, body, box_end, depth + 1, found, trak)?;
        } else if &kind == b"meta" {
            // an MP4 `meta` is a full box (4 bytes of version and flags); QuickTime's isn't
            let mut peek = [0u8; 8];
            f.read_exact(&mut peek)?;
            let skip = if &peek[4..8] == b"hdlr" || &peek[4..8] == b"ilst" {
                0
            } else {
                4
            };
            walk(f, body + skip, box_end, depth + 1, found, trak)?;
        } else if box_end - body <= LEAF_LIMIT {
            let mut data = vec![0u8; (box_end - body) as usize];
            f.read_exact(&mut data)?;
            match (&kind, trak.as_mut()) {
                (b"mvhd", _) => found.movie_scale = timescale(&data),
                (b"mdhd", Some(t)) => {
                    t.scale = timescale(&data);
                    t.duration = media_duration(&data);
                }
                (b"hdlr", Some(t)) if data.len() >= 12 => t.sound = &data[8..12] == b"soun",
                (b"elst", Some(t)) => t.edit = first_edit(&data),
                (b"name", _) if data.len() > 4 => {
                    name = Some(String::from_utf8_lossy(&data[4..]).into_owned())
                }
                (b"data", _) if data.len() > 8 => {
                    value = Some(String::from_utf8_lossy(&data[8..]).into_owned())
                }
                _ => {}
            }
        }
        at = box_end;
    }
    if let (Some(n), Some(v)) = (name, value)
        && n.trim_matches(char::from(0)).trim() == "iTunSMPB"
    {
        found.smpb = itunsmpb(&v);
    }
    Ok(())
}

fn timescale(data: &[u8]) -> Option<u32> {
    // version 0: version/flags, created, modified (4 bytes each), timescale; version 1: 8-byte times
    let at = if *data.first()? == 1 { 20 } else { 12 };
    let b = data.get(at..at + 4)?;
    Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

/// A media header's duration, in its own timescale.
fn media_duration(data: &[u8]) -> Option<u64> {
    if *data.first()? == 1 {
        Some(u64::from_be_bytes(data.get(24..32)?.try_into().ok()?))
    } else {
        Some(u32::from_be_bytes(data.get(16..20)?.try_into().ok()?) as u64)
    }
}

/// The first edit that shows media (an empty edit, media time -1, only delays the start).
fn first_edit(data: &[u8]) -> Option<(u64, i64)> {
    let version = *data.first()?;
    let count = u32::from_be_bytes(data.get(4..8)?.try_into().ok()?) as usize;
    let entry = if version == 1 { 20 } else { 12 };
    for i in 0..count.min(16) {
        let e = data.get(8 + i * entry..8 + (i + 1) * entry)?;
        let (segment, media_time) = if version == 1 {
            (
                u64::from_be_bytes(e[0..8].try_into().ok()?),
                i64::from_be_bytes(e[8..16].try_into().ok()?),
            )
        } else {
            (
                u32::from_be_bytes(e[0..4].try_into().ok()?) as u64,
                i32::from_be_bytes(e[4..8].try_into().ok()?) as i64,
            )
        };
        if media_time >= 0 {
            return Some((segment, media_time));
        }
    }
    None
}

/// iTunes' `iTunSMPB`: " 00000000 00000840 000001CA 00000000003F31F6 ..." — zero, delay,
/// padding, playable length, in hexadecimal.
pub(crate) fn itunsmpb(text: &str) -> Option<Gapless> {
    let mut parts = text.split_whitespace();
    parts.next()?;
    let delay = u64::from_str_radix(parts.next()?, 16).ok()?;
    let _padding = u64::from_str_radix(parts.next()?, 16).ok()?;
    let length = u64::from_str_radix(parts.next()?, 16).ok()?;
    if delay == 0 && length == 0 {
        return None;
    }
    Some(Gapless {
        delay,
        length: (length > 0).then_some(length),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_itunes_gapless_comment() {
        let g = itunsmpb(" 00000000 00000840 000001CA 00000000003F31F6 00000000 00000000").unwrap();
        assert_eq!(
            g,
            Gapless {
                delay: 0x840,
                length: Some(0x3F31F6)
            }
        );
        assert_eq!(
            itunsmpb(" 00000000 00000000 00000000 0000000000000000"),
            None
        );
        assert_eq!(itunsmpb("nonsense"), None);
    }

    #[test]
    fn reads_edit_lists_of_both_versions() {
        // version 0: an empty edit first (media time -1), then the real one
        let mut v0 = vec![0, 0, 0, 0, 0, 0, 0, 2];
        v0.extend_from_slice(&100u32.to_be_bytes());
        v0.extend_from_slice(&(-1i32).to_be_bytes());
        v0.extend_from_slice(&0x0001_0000u32.to_be_bytes());
        v0.extend_from_slice(&52_000u32.to_be_bytes());
        v0.extend_from_slice(&1024i32.to_be_bytes());
        v0.extend_from_slice(&0x0001_0000u32.to_be_bytes());
        assert_eq!(first_edit(&v0), Some((52_000, 1024)));
        let mut v1 = vec![1, 0, 0, 0, 0, 0, 0, 1];
        v1.extend_from_slice(&52_000u64.to_be_bytes());
        v1.extend_from_slice(&2112i64.to_be_bytes());
        v1.extend_from_slice(&0x0001_0000u32.to_be_bytes());
        assert_eq!(first_edit(&v1), Some((52_000, 2112)));
    }
}
