//! Retrieve the movie clock and MPEG frame timings at the precision used in the shared cache.

use std::{
    fs::File,
    io::{self, BufReader, Read, Seek, SeekFrom},
};

fn u32_at(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(i..i + 4)?.try_into().ok()?))
}
fn atom(
    r: &mut BufReader<File>,
    mut at: u64,
    end: u64,
    kind: &[u8; 4],
) -> io::Result<Option<(u64, u64)>> {
    let mut h = [0; 8];
    for _ in 0..512 {
        if end.saturating_sub(at) < 8 {
            break;
        }
        r.seek(SeekFrom::Start(at))?;
        r.read_exact(&mut h)?;
        let found = h[4..] == *kind;
        let (len, header) = match u32_at(&h, 0).unwrap() {
            0 => (end - at, 8),
            1 if end - at >= 16 => {
                r.read_exact(&mut h)?;
                (u64::from_be_bytes(h), 16)
            }
            n => (u64::from(n), 8),
        };
        if len < header || len > end - at {
            break;
        }
        if found {
            return Ok(Some((at + header, at + len)));
        }
        at += len;
    }
    Ok(None)
}
pub(super) fn movie(r: &mut BufReader<File>) -> io::Result<Option<f64>> {
    let end = r.get_ref().metadata()?.len();
    let Some((start, end)) = atom(r, 0, end, b"moov")? else {
        return Ok(None);
    };
    let Some((start, end)) = atom(r, start, end, b"mvhd")? else {
        return Ok(None);
    };
    let mut b = [0; 32];
    let n = (end - start).min(32) as usize;
    r.seek(SeekFrom::Start(start))?;
    r.read_exact(&mut b[..n])?;
    let (ticks, scale) = match b[0] {
        1 if n >= 32 => (
            u64::from_be_bytes(b[24..32].try_into().unwrap()),
            u32_at(&b, 20).unwrap(),
        ),
        0 if n >= 20 => (u64::from(u32_at(&b, 16).unwrap()), u32_at(&b, 12).unwrap()),
        _ => return Ok(None),
    };
    Ok((scale != 0).then(|| ticks as f64 / scale as f64))
}

struct Frame {
    samples: u32,
    rate: u32,
    kbps: u32,
    length: usize,
    side: usize,
}
fn frame(b: &[u8]) -> Option<Frame> {
    let [sync, flags, info, mode, ..] = *b else {
        return None;
    };
    let version = (flags >> 3) & 3;
    let layer = 4 - ((flags >> 1) & 3);
    let index = (info >> 4) as usize;
    let freq = ((info >> 2) & 3) as usize;
    if sync != 255
        || flags & 0xe0 != 0xe0
        || version == 1
        || layer == 4
        || !(1..15).contains(&index)
        || freq == 3
    {
        return None;
    }
    let v1 = version == 3;
    let rates = match (v1, layer) {
        (true, 1) => [
            32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448,
        ],
        (true, 2) => [
            32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384,
        ],
        (true, 3) => [
            32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
        ],
        (false, 1) => [
            32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256,
        ],
        _ => [8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160],
    };
    let kbps = rates[index - 1];
    let rate = [44100, 48000, 32000][freq]
        / match version {
            3 => 1,
            2 => 2,
            _ => 4,
        };
    let samples = if layer == 1 {
        384
    } else if v1 || layer == 2 {
        1152
    } else {
        576
    };
    let pad = u32::from((info >> 1) & 1);
    let length = if layer == 1 {
        (12 * kbps * 1000 / rate + pad) * 4
    } else {
        samples / 8 * kbps * 1000 / rate + pad
    };
    let side = match (v1, mode >> 6 == 3) {
        (true, false) => 32,
        (false, true) => 9,
        _ => 17,
    };
    Some(Frame {
        samples,
        rate,
        kbps,
        length: length as usize,
        side,
    })
}
pub(super) fn mpeg(r: &mut BufReader<File>, tag_seconds: Option<f64>) -> io::Result<Option<f64>> {
    let size = r.get_ref().metadata()?.len();
    if size < 10 {
        return Ok(None);
    }
    let mut h = [0; 10];
    r.rewind()?;
    r.read_exact(&mut h)?;
    let start = if &h[..3] == b"ID3" {
        10 + h[6..10]
            .iter()
            .fold(0u64, |n, x| (n << 7) | u64::from(x & 127))
            + u64::from(h[3] == 4 && h[5] & 16 != 0) * 10
    } else {
        0
    };
    r.seek(SeekFrom::Start(start))?;
    let mut b = Vec::new();
    r.take(4096).read_to_end(&mut b)?;
    if !frame(&b).is_some_and(|f| b.get(f.length..).and_then(frame).is_some()) {
        r.take(65536 - 4096).read_to_end(&mut b)?;
    }
    for (i, bytes) in b.windows(4).enumerate() {
        let Some(f) = frame(bytes) else {
            continue;
        };
        if f.length < 24
            || b.get(i + f.length..)
                .is_some_and(|s| s.len() >= 4 && frame(s).is_none())
        {
            continue;
        }
        let x = i + 4 + f.side;
        let frames = if matches!(b.get(x..x + 4), Some(b"Xing" | b"Info"))
            && u32_at(&b, x + 4).is_some_and(|n| n & 1 != 0)
        {
            u32_at(&b, x + 8)
        } else if b.get(i + 36..i + 40) == Some(b"VBRI") {
            u32_at(&b, i + 50)
        } else {
            None
        };
        if let Some(n) = frames.filter(|&n| n > 0) {
            return Ok(Some(
                f64::from(n) * f64::from(f.samples) / f64::from(f.rate),
            ));
        }
        if tag_seconds.is_some() {
            return Ok(tag_seconds);
        }
        let mut tail = 0;
        if size >= 128 {
            r.seek(SeekFrom::End(-128))?;
            r.read_exact(&mut h[..3])?;
            if &h[..3] == b"TAG" {
                tail = 128;
            }
        }
        return Ok(Some(
            size.saturating_sub(start + i as u64 + tail) as f64 * 8. / f64::from(f.kbps * 1000),
        ));
    }
    Ok(tag_seconds)
}
