//! Inspect AAC-LC raw_data_block elements on the missing-platform fallback. Walk the
//! channel syntax to reach FIL without searching inside spectral or ancillary data.
use super::Error;
use std::io;
use std::sync::LazyLock;
use symphonia::core::codecs::audio::AudioCodecParameters;
use symphonia::core::io::vlc::{BitOrder, Codebook, CodebookBuilder, Entry8x16};
use symphonia::core::io::{BitReaderLtr, ReadBitsLtr};

pub(super) struct ProfileCheck {
    long: &'static [u16],
    short: &'static [u16],
}

static BOOKS: LazyLock<[Codebook<Entry8x16>; 12]> = LazyLock::new(|| {
    SHAPES.map(|(counts, extra)| {
        let (mut codes, mut lengths, mut values) = (Vec::new(), Vec::new(), Vec::new());
        let mut code = 0;
        for (n, &count) in counts.iter().enumerate() {
            code <<= 1;
            for _ in 0..count {
                codes.push(code);
                lengths.push((n + 1) as u8);
                values.push(extra.get(values.len()).copied().unwrap_or(0));
                code += 1;
            }
        }
        CodebookBuilder::new(BitOrder::Verbatim)
            .make(&codes, &lengths, &values)
            .expect("AAC codebook")
    })
});

fn invalid() -> io::Error {
    io::ErrorKind::InvalidData.into()
}

struct Window {
    bands: usize,
    groups: usize,
    sizes: [u8; 8],
    short: bool,
}

impl Window {
    fn read(b: &mut BitReaderLtr<'_>) -> io::Result<Self> {
        if b.read_bool()? {
            return Err(invalid());
        }
        let short = b.read_bits_leq32(2)? == 2;
        b.ignore_bits(1)?; // shape
        let bands = b.read_bits_leq32(if short { 4 } else { 6 })? as usize;
        let mut sizes = [1; 8];
        let mut groups = 1;
        if short {
            for _ in 1..8 {
                if b.read_bool()? {
                    sizes[groups - 1] += 1;
                } else {
                    groups += 1;
                }
            }
        } else if b.read_bool()? {
            return Err(invalid()); // LC has no predictor
        }
        Ok(Self {
            bands,
            groups,
            sizes,
            short,
        })
    }
}

impl ProfileCheck {
    pub fn new(params: &AudioCodecParameters) -> Option<Self> {
        let (long, short) = match params.sample_rate? {
            75132.. => (BANDS_96K_LONG, BANDS_64K_SHORT),
            55426.. => (BANDS_64K_LONG, BANDS_64K_SHORT),
            37566.. => (BANDS_48K_LONG, BANDS_48K_SHORT),
            27713.. => (BANDS_32K_LONG, BANDS_48K_SHORT),
            18783.. => (BANDS_24K_LONG, BANDS_24K_SHORT),
            9391.. => (BANDS_16K_LONG, BANDS_16K_SHORT),
            _ => (BANDS_8K_LONG, BANDS_8K_SHORT),
        };
        Some(Self { long, short })
    }

    pub fn check(&mut self, data: &[u8]) -> Result<(), Error> {
        if data.len() <= 8191 {
            match self.has_sbr(&mut BitReaderLtr::new(data)) {
                Ok(false) => return Ok(()),
                Ok(true) => {
                    return Err(Error::Decode(
                        "HE-AAC requires the Windows media decoder".into(),
                    ));
                }
                Err(_) => {}
            }
        }
        Err(Error::Decode("cannot verify AAC profile".into()))
    }

    fn has_sbr(&self, b: &mut BitReaderLtr<'_>) -> io::Result<bool> {
        #[cfg(test)]
        PARSES.set(PARSES.get() + 1);
        loop {
            match b.read_bits_leq32(3)? {
                0 | 3 => {
                    b.ignore_bits(4)?;
                    self.channel(b, None)?;
                }
                1 => {
                    b.ignore_bits(4)?;
                    let common = if b.read_bool()? {
                        let w = Window::read(b)?;
                        match b.read_bits_leq32(2)? {
                            1 => b.ignore_bits((w.groups * w.bands) as u32)?,
                            3 => return Err(invalid()),
                            _ => {}
                        }
                        Some(w)
                    } else {
                        None
                    };
                    self.channel(b, common.as_ref())?;
                    self.channel(b, common.as_ref())?;
                }
                4 => {
                    b.ignore_bits(4)?;
                    let align = b.read_bool()?;
                    let mut n = b.read_bits_leq32(8)?;
                    if n == 255 {
                        n += b.read_bits_leq32(8)?;
                    }
                    if align {
                        b.realign();
                    }
                    b.ignore_bits(n * 8)?;
                }
                6 => {
                    let mut n = b.read_bits_leq32(4)?;
                    if n == 15 {
                        n = 14 + b.read_bits_leq32(8)?;
                    }
                    if n > 0 {
                        let ext = b.read_bits_leq32(4)?;
                        b.ignore_bits(8 * n - 4)?;
                        if matches!(ext, 13 | 14) {
                            return Ok(true);
                        }
                    }
                }
                7 => return Ok(false),
                _ => return Err(invalid()), // The native LC decoder rejects CCE and PCE.
            }
        }
    }

    fn channel(&self, b: &mut BitReaderLtr<'_>, common: Option<&Window>) -> io::Result<()> {
        b.ignore_bits(8)?; // global gain
        let local;
        let w = match common {
            Some(w) => w,
            None => {
                local = Window::read(b)?;
                &local
            }
        };
        let offsets = if w.short { self.short } else { self.long };
        if w.bands >= offsets.len() {
            return Err(invalid());
        }
        let mut books = [[0u8; 64]; 8];
        let width = if w.short { 3 } else { 5 };
        let escape = (1 << width) - 1;
        for group in &mut books[..w.groups] {
            let mut start = 0;
            while start < w.bands {
                let book = b.read_bits_leq32(4)? as u8;
                let mut end = start;
                loop {
                    let n = b.read_bits_leq32(width)?;
                    end += n as usize;
                    if end > w.bands {
                        return Err(invalid());
                    }
                    if n != escape {
                        break;
                    }
                }
                if start == end || book == 12 {
                    return Err(invalid());
                }
                group[start..end].fill(book);
                start = end;
            }
        }
        let mut first_noise = true;
        for group in &books[..w.groups] {
            for &book in &group[..w.bands] {
                if book == 13 && first_noise {
                    first_noise = false;
                    b.ignore_bits(9)?;
                } else if book != 0 {
                    b.read_codebook(&BOOKS[0])?;
                }
            }
        }
        if b.read_bool()? {
            // pulse data
            let pulses = b.read_bits_leq32(2)? + 1;
            b.ignore_bits(6 + 9 * pulses)?;
        }
        if b.read_bool()? {
            // temporal noise shaping
            for _ in 0..if w.short { 8 } else { 1 } {
                let filters = b.read_bits_leq32(if w.short { 1 } else { 2 })?;
                if filters == 0 {
                    continue;
                }
                let resolution = 3 + b.read_bits_leq32(1)?;
                for _ in 0..filters {
                    b.ignore_bits(if w.short { 4 } else { 6 })?;
                    let order = b.read_bits_leq32(if w.short { 3 } else { 5 })?;
                    if order > 0 {
                        b.ignore_bits(1)?;
                        let compress = b.read_bits_leq32(1)?;
                        b.ignore_bits(order * (resolution - compress))?;
                    }
                }
            }
        }
        if b.read_bool()? {
            return Err(invalid());
        } // gain control is outside LC
        for (group, &windows) in books.iter().zip(&w.sizes).take(w.groups) {
            for (band, &book) in group[..w.bands].iter().enumerate() {
                if !(1..=11).contains(&book) {
                    continue;
                }
                let dimension = if book <= 4 { 4 } else { 2 };
                let words = (offsets[band + 1] - offsets[band]) / dimension * u16::from(windows);
                for _ in 0..words {
                    let (extra, _) = b.read_codebook(&BOOKS[usize::from(book)])?;
                    b.ignore_bits(u32::from(extra & 7))?;
                    for _ in 0..extra >> 3 {
                        let width = 4 + b.read_unary_ones()?;
                        b.ignore_bits(width)?;
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
thread_local! {
    static PARSES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[test]
fn checks_one_access_unit_once_even_with_sbr_patterns_in_ancillary_data() {
    use symphonia::core::audio::Channels;
    use symphonia::core::codecs::audio::well_known::CODEC_ID_AAC;
    let mut params = AudioCodecParameters::new();
    params
        .for_codec(CODEC_ID_AAC)
        .with_sample_rate(48000)
        .with_channels(Channels::Discrete(2));
    let mut check = ProfileCheck::new(&params).unwrap();
    // Byte-aligned ID_DSE with 240 opaque bytes, then ID_END.
    let mut data = vec![0x81, 240];
    for _ in 0..120 {
        data.extend([0xc3, 0xa0]);
    }
    data.push(0xe0);
    PARSES.set(0);
    assert!(check.check(&data).is_ok());
    assert_eq!(PARSES.get(), 1);
    // Replace ID_END by an actual one-byte ID_FIL with EXT_SBR_DATA, then ID_END.
    data.pop();
    data.extend([0xc3, 0xa1, 0xc0]);
    assert!(matches!(check.check(&data), Err(Error::Decode(msg)) if msg.contains("HE-AAC")));
}

#[test]
fn high_bitrate_stereo_lc_is_checked_once_per_packet() {
    use super::{next_packet, probe};
    use std::fs::File;
    use std::process::Command;
    use symphonia::core::formats::TrackType;
    let path = std::env::temp_dir().join(format!("string-{}-lc-profile.aac", std::process::id()));
    let encoder = std::env::var_os("FFMPEG").unwrap_or_else(|| "ffmpeg".into());
    let Ok(result) = Command::new(encoder)
        .args([
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "aevalsrc=sin(0.0001*n*n)|0.2*sin(300*t):s=48000:d=1",
            "-c:a",
            "aac",
            "-b:a",
            "320k",
        ])
        .arg(&path)
        .output()
    else {
        return;
    };
    assert!(result.status.success());
    let mut format = probe(&path, Box::new(File::open(&path).unwrap())).unwrap();
    let t = format.default_track(TrackType::Audio).unwrap();
    let mut check = ProfileCheck::new(t.codec_params.as_ref().unwrap().audio().unwrap()).unwrap();
    PARSES.set(0);
    let mut packets = 0;
    while let Some(p) = next_packet(format.as_mut()).unwrap() {
        check.check(&p.data).unwrap();
        packets += 1;
    }
    assert!(packets >= 47);
    assert_eq!(PARSES.get(), packets);
    drop(format);
    std::fs::remove_file(path).unwrap();
}

// ISO/IEC 14496-3 AAC Huffman tables and scale-factor band offsets, derived from
// Symphonia 0.6.1 (Mozilla Public License 2.0). For each canonical code length
// store its symbol count. Only unsigned spectral books need a per-symbol value:
// low three bits count sign bits, upper bits count escape words.
type BookShape = (&'static [u8], &'static [u8]);
const SHAPES: [BookShape; 12] = [
    (
        &[1, 0, 1, 3, 2, 4, 3, 5, 4, 6, 6, 6, 5, 8, 4, 7, 3, 7, 46],
        &[],
    ),
    (&[1, 0, 0, 0, 8, 0, 24, 0, 24, 8, 16], &[]),
    (&[0, 0, 1, 1, 7, 24, 15, 19, 14], &[]),
    (
        &[1, 0, 0, 4, 2, 6, 3, 5, 15, 15, 8, 9, 3, 3, 5, 2],
        &[
            0, 1, 1, 1, 1, 2, 2, 2, 2, 2, 3, 2, 3, 4, 3, 3, 1, 1, 2, 2, 3, 2, 3, 2, 3, 3, 1, 2, 2,
            1, 2, 2, 4, 3, 3, 4, 3, 2, 3, 2, 4, 4, 3, 3, 2, 3, 3, 3, 2, 2, 3, 2, 4, 3, 3, 4, 4, 2,
            3, 3, 3, 3, 3, 3, 4, 4, 4, 4, 2, 2, 3, 3, 4, 3, 4, 4, 4, 3, 2, 3, 3,
        ],
    ),
    (
        &[0, 0, 0, 10, 6, 0, 9, 21, 8, 14, 11, 2],
        &[
            4, 3, 3, 3, 3, 1, 2, 0, 2, 2, 2, 2, 1, 2, 1, 1, 4, 4, 4, 4, 3, 3, 3, 3, 3, 3, 3, 3, 3,
            3, 3, 3, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 1, 1, 1, 1, 4, 4, 4, 4, 4, 4, 3, 3, 3, 3,
            3, 3, 3, 3, 3, 4, 3, 3, 3, 4, 4, 4, 2, 2, 2, 2, 2, 2, 4, 3, 3, 3, 3,
        ],
    ),
    (&[1, 0, 0, 4, 4, 0, 4, 12, 12, 12, 18, 10, 4], &[]),
    (&[0, 0, 0, 9, 0, 16, 13, 8, 23, 8, 4], &[]),
    (
        &[1, 0, 2, 1, 0, 4, 5, 10, 14, 15, 8, 4],
        &[
            0, 1, 1, 2, 2, 2, 1, 1, 2, 2, 2, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 1, 1, 2, 2, 2, 1, 2, 1,
            2, 2, 2, 2, 2, 2, 2, 2, 2, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 1, 1, 2, 2, 2, 2,
            2, 2, 2, 2, 2, 2,
        ],
    ),
    (
        &[0, 0, 1, 5, 7, 10, 14, 15, 8, 4],
        &[
            2, 2, 1, 2, 1, 2, 0, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2,
            2, 1, 2, 1, 2, 2, 2, 2, 2, 2, 2, 1, 2, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 1, 2, 1, 2,
            2, 2, 2, 1, 1, 2,
        ],
    ),
    (
        &[1, 0, 2, 1, 0, 4, 3, 8, 11, 20, 31, 38, 32, 14, 4],
        &[
            0, 1, 1, 2, 2, 2, 1, 1, 2, 2, 2, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 1, 1, 2, 2, 2, 2, 2, 2,
            2, 1, 2, 1, 2, 2, 2, 2, 2, 2, 2, 2, 1, 2, 2, 2, 2, 2, 2, 2, 2, 1, 2, 2, 2, 1, 2, 2, 2,
            2, 1, 2, 2, 2, 2, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 1, 2, 2, 2, 1, 2, 2, 2,
            2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 1, 2, 2, 1, 2, 2, 2, 2, 2, 2, 2, 1,
            2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 1, 2, 2, 1, 2, 2, 2, 2, 1, 2, 2, 2, 2, 2,
            2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
        ],
    ),
    (
        &[0, 0, 0, 3, 8, 14, 17, 25, 31, 41, 22, 8],
        &[
            2, 2, 2, 2, 1, 1, 2, 2, 2, 2, 2, 1, 1, 2, 2, 2, 2, 0, 2, 2, 1, 1, 2, 2, 2, 2, 2, 2, 2,
            2, 2, 2, 2, 2, 1, 2, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 1, 2, 1, 2, 2, 2, 2, 2, 2,
            2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 1, 2, 2, 2, 2, 2, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
            2, 2, 2, 2, 2, 2, 2, 1, 2, 1, 2, 2, 2, 2, 2, 2, 2, 1, 2, 2, 2, 2, 2, 2, 1, 2, 2, 2, 2,
            2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 1, 2, 1, 2, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
            2, 2, 2, 2, 1, 2, 1, 2, 2, 2, 1, 2, 2, 2, 1, 2, 2, 2, 2, 2, 2, 1, 2, 2,
        ],
    ),
    (
        &[0, 0, 0, 2, 6, 7, 16, 59, 55, 95, 43, 6],
        &[
            0, 2, 18, 1, 1, 2, 2, 2, 2, 2, 2, 1, 2, 1, 2, 2, 2, 2, 2, 2, 2, 1, 1, 2, 2, 2, 2, 2, 2,
            2, 2, 2, 2, 2, 2, 2, 2, 2, 10, 10, 10, 10, 10, 2, 10, 1, 2, 1, 10, 2, 10, 10, 10, 10,
            2, 2, 2, 2, 2, 10, 10, 2, 2, 10, 10, 10, 2, 2, 10, 2, 10, 10, 10, 2, 10, 2, 2, 10, 10,
            2, 2, 2, 1, 10, 10, 2, 2, 2, 2, 1, 2, 10, 10, 2, 2, 10, 2, 10, 2, 2, 2, 2, 2, 2, 2, 10,
            2, 2, 1, 2, 2, 2, 2, 2, 2, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 1, 2, 2, 2, 2,
            2, 2, 2, 2, 2, 2, 2, 9, 2, 2, 2, 2, 2, 2, 9, 2, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
            2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
            2, 2, 2, 2, 2, 2, 2, 2, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
            2, 2, 2, 2, 2, 2, 2, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
            2, 2, 2, 2, 2, 2, 1, 2, 2, 1, 2, 1, 2, 2, 2, 1, 2, 2, 2, 2, 2, 1, 1, 2, 2, 2, 2, 1, 2,
            2, 1, 2, 1, 1, 2, 1, 1, 2,
        ],
    ),
];
const BANDS_48K_LONG: &[u16] = &[
    0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 48, 56, 64, 72, 80, 88, 96, 108, 120, 132, 144, 160,
    176, 196, 216, 240, 264, 292, 320, 352, 384, 416, 448, 480, 512, 544, 576, 608, 640, 672, 704,
    736, 768, 800, 832, 864, 896, 928, 1024,
];
const BANDS_48K_SHORT: &[u16] = &[0, 4, 8, 12, 16, 20, 28, 36, 44, 56, 68, 80, 96, 112, 128];
const BANDS_32K_LONG: &[u16] = &[
    0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 48, 56, 64, 72, 80, 88, 96, 108, 120, 132, 144, 160,
    176, 196, 216, 240, 264, 292, 320, 352, 384, 416, 448, 480, 512, 544, 576, 608, 640, 672, 704,
    736, 768, 800, 832, 864, 896, 928, 960, 992, 1024,
];
const BANDS_8K_LONG: &[u16] = &[
    0, 12, 24, 36, 48, 60, 72, 84, 96, 108, 120, 132, 144, 156, 172, 188, 204, 220, 236, 252, 268,
    288, 308, 328, 348, 372, 396, 420, 448, 476, 508, 544, 580, 620, 664, 712, 764, 820, 880, 944,
    1024,
];
const BANDS_8K_SHORT: &[u16] = &[
    0, 4, 8, 12, 16, 20, 24, 28, 36, 44, 52, 60, 72, 88, 108, 128,
];
const BANDS_16K_LONG: &[u16] = &[
    0, 8, 16, 24, 32, 40, 48, 56, 64, 72, 80, 88, 100, 112, 124, 136, 148, 160, 172, 184, 196, 212,
    228, 244, 260, 280, 300, 320, 344, 368, 396, 424, 456, 492, 532, 572, 616, 664, 716, 772, 832,
    896, 960, 1024,
];
const BANDS_16K_SHORT: &[u16] = &[
    0, 4, 8, 12, 16, 20, 24, 28, 32, 40, 48, 60, 72, 88, 108, 128,
];
const BANDS_24K_LONG: &[u16] = &[
    0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 44, 52, 60, 68, 76, 84, 92, 100, 108, 116, 124, 136,
    148, 160, 172, 188, 204, 220, 240, 260, 284, 308, 336, 364, 396, 432, 468, 508, 552, 600, 652,
    704, 768, 832, 896, 960, 1024,
];
const BANDS_24K_SHORT: &[u16] = &[
    0, 4, 8, 12, 16, 20, 24, 28, 36, 44, 52, 64, 76, 92, 108, 128,
];
const BANDS_64K_LONG: &[u16] = &[
    0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 44, 48, 52, 56, 64, 72, 80, 88, 100, 112, 124, 140,
    156, 172, 192, 216, 240, 268, 304, 344, 384, 424, 464, 504, 544, 584, 624, 664, 704, 744, 784,
    824, 864, 904, 944, 984, 1024,
];
const BANDS_64K_SHORT: &[u16] = &[0, 4, 8, 12, 16, 20, 24, 32, 40, 48, 64, 92, 128];
const BANDS_96K_LONG: &[u16] = &[
    0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 44, 48, 52, 56, 64, 72, 80, 88, 96, 108, 120, 132,
    144, 156, 172, 188, 212, 240, 276, 320, 384, 448, 512, 576, 640, 704, 768, 832, 896, 960, 1024,
];
