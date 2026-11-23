//! Read local metadata and embedded lyrics without changing a media file.

mod duration;

use lofty::{
    config::ParseOptions,
    file::{AudioFile, FileType, TaggedFile, TaggedFileExt},
    flac::FlacFile,
    id3::v2::{AttachedPictureFrame, Id3v2Version},
    mpeg::MpegFile,
    ogg::{OpusFile, VorbisFile, tag::VorbisComments},
    picture::PictureType,
    prelude::{Accessor, ItemKey},
    probe::Probe,
};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{self, BufRead, BufReader, Cursor, Read, Seek, SeekFrom},
    path::Path,
};

pub(crate) fn zero<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Tags {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub format: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub artist: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub album: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub album_artist: String,
    #[serde(skip_serializing_if = "zero", deserialize_with = "lenient_u32")]
    pub track: u32,
    #[serde(skip_serializing_if = "zero", deserialize_with = "lenient_u32")]
    pub disc: u32,
    #[serde(skip_serializing_if = "zero", deserialize_with = "lenient_u32")]
    pub year: u32,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub genre: String,
    #[serde(skip_serializing_if = "zero", deserialize_with = "lenient_f64")]
    pub duration: f64,
}
// String 1.x reads these numbers from the songs' own tags with no upper limit (a WAV can say track 4294967296), and
// one value that doesn't fit would otherwise stop the whole library cache from opening. Too big becomes the largest
// that fits; anything that isn't a number becomes 0.
fn lenient_u32<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    Ok(match serde_json::Value::deserialize(d)? {
        serde_json::Value::Number(n) => n
            .as_u64()
            .map(|v| v.min(u64::from(u32::MAX)) as u32)
            .or_else(|| n.as_f64().map(|v| v.clamp(0., f64::from(u32::MAX)) as u32))
            .unwrap_or(0),
        serde_json::Value::String(s) => s.trim().parse().unwrap_or(0),
        _ => 0,
    })
}
fn lenient_f64<'de, D: serde::Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    Ok(match serde_json::Value::deserialize(d)? {
        serde_json::Value::Number(n) => n.as_f64().filter(|v| v.is_finite() && *v >= 0.).unwrap_or(0.),
        serde_json::Value::String(s) => s.trim().parse().ok().filter(|v: &f64| v.is_finite() && *v >= 0.).unwrap_or(0.),
        _ => 0.,
    })
}
#[derive(Debug, Default)]
pub struct Metadata {
    pub tags: Tags,
    pub cover_mime: Option<String>,
    pub lyrics: Option<String>,
}

pub fn read(path: &Path) -> io::Result<Metadata> {
    let mut reader = BufReader::new(File::open(path)?);
    if reader.fill_buf()?.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]) {
        return Ok(Metadata {
            tags: Tags {
                format: "webm".into(),
                duration: webm_duration(&mut reader)?.unwrap_or_default(),
                ..Default::default()
            },
            ..Default::default()
        });
    }
    let starts_id3 = reader.fill_buf()?.starts_with(b"ID3");
    let options = ParseOptions::new().max_junk_bytes(65536);
    let probe = Probe::new(&mut reader).options(options).guess_file_type()?;
    let ty = probe.file_type();
    if let Some(format) = id3_format(ty, starts_id3) {
        return Ok(read_id3_audio(probe.into_inner(), format, false)?.meta);
    }
    let mut lyrics = None;
    // Preserve written Vorbis numbers (including vinyl sides). Other formats still use
    // their usual conversions, e.g. MP4's binary gnre atom to a text genre.
    let mut comments = options;
    comments.implicit_conversions(false);
    let file: TaggedFile = match ty {
        Some(FileType::Flac) => {
            let f = FlacFile::read_from(probe.into_inner(), comments).map_err(io::Error::other)?;
            lyrics = f.vorbis_comments().and_then(vorbis_lyrics);
            f.into()
        }
        Some(FileType::Opus) => {
            let f = OpusFile::read_from(probe.into_inner(), comments).map_err(io::Error::other)?;
            lyrics = vorbis_lyrics(f.vorbis_comments());
            f.into()
        }
        Some(FileType::Vorbis) => {
            let f =
                VorbisFile::read_from(probe.into_inner(), comments).map_err(io::Error::other)?;
            lyrics = vorbis_lyrics(f.vorbis_comments());
            f.into()
        }
        _ => probe.read().map_err(io::Error::other)?,
    };
    let format = match file.file_type() {
        FileType::Mp4 => "m4a",
        FileType::Vorbis => "ogg",
        FileType::Opus => "opus",
        FileType::Flac => "flac",
        FileType::Wav => "wav",
        _ => "",
    };
    let mut tags = Tags {
        format: format.into(),
        duration: file.properties().duration().as_secs_f64(),
        ..Default::default()
    };
    if file.file_type() == FileType::Mp4 {
        tags.duration = duration::movie(&mut reader)?.unwrap_or(tags.duration);
    }
    let mut cover_mime = None;
    let primary = file.primary_tag();
    for tag in primary.into_iter().chain(
        file.tags()
            .iter()
            .filter(|t| !primary.is_some_and(|p| std::ptr::eq(p, *t))),
    ) {
        for (dest, src) in [
            (&mut tags.title, tag.title()),
            (&mut tags.artist, tag.artist()),
            (&mut tags.album, tag.album()),
        ] {
            if dest.is_empty()
                && let Some(s) = src
            {
                *dest = s.trim().into();
            }
        }
        if tags.album_artist.is_empty() {
            tags.album_artist = tag
                .get_string(ItemKey::AlbumArtist)
                .unwrap_or("")
                .trim()
                .into();
        }
        if tags.track == 0 {
            tags.track = tag
                .get_strings(ItemKey::TrackNumber)
                .map(number)
                .find(|&n| n != 0)
                .unwrap_or(0);
        }
        if tags.disc == 0 {
            tags.disc = tag
                .get_strings(ItemKey::DiscNumber)
                .map(number)
                .find(|&n| n != 0)
                .unwrap_or(0);
        }
        if tags.year == 0 {
            tags.year = tag
                .items()
                .filter(|i| matches!(i.key(), ItemKey::Year | ItemKey::RecordingDate))
                .filter_map(|i| i.value().text())
                .map(year)
                .find(|&n| n != 0)
                .unwrap_or(0);
        }
        if tags.genre.is_empty() {
            tags.genre = tag
                .get_strings(ItemKey::Genre)
                .map(genre)
                .find(|s| !s.is_empty())
                .unwrap_or_default()
                .into();
        }
        if cover_mime.is_none()
            && let Some(pic) = tag
                .pictures()
                .iter()
                .find(|p| p.pic_type() == PictureType::CoverFront)
                .or_else(|| tag.pictures().first())
        {
            cover_mime = Some(pic.mime_type().map_or("image/jpeg", |m| m.as_str()).into());
        }
        if lyrics.is_none() {
            lyrics = [ItemKey::Lyrics, ItemKey::UnsyncLyrics]
                .iter()
                .find_map(|k| tag.get_string(*k).and_then(clean));
        }
    }
    Ok(Metadata {
        tags,
        cover_mime,
        lyrics,
    })
}
/// The picture inside a song (its front cover, else the first one), for songs whose picture can't be cut straight out
/// of the file: Ogg Opus and Vorbis keep it base64-encoded in a comment.
pub fn picture(path: &Path) -> io::Result<Option<Vec<u8>>> {
    let mut reader = BufReader::new(File::open(path)?);
    let starts_id3 = reader.fill_buf()?.starts_with(b"ID3");
    let options = ParseOptions::new()
        .max_junk_bytes(65536)
        .read_properties(false);
    let probe = Probe::new(&mut reader).options(options).guess_file_type()?;
    if let Some(format) = id3_format(probe.file_type(), starts_id3) {
        return Ok(read_id3_audio(probe.into_inner(), format, true)?.picture);
    }
    let file = probe.read().map_err(io::Error::other)?;
    let pictures: Vec<_> = file.tags().iter().flat_map(|t| t.pictures()).collect();
    Ok(pictures
        .iter()
        .find(|p| p.pic_type() == PictureType::CoverFront)
        .or(pictures.first())
        .map(|p| p.data().to_vec()))
}
fn clean(s: &str) -> Option<String> {
    let s = s
        .replace('\0', "")
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let text = s.trim();
    (!text.is_empty()).then(|| text.into())
}

fn vorbis_lyrics(tag: &VorbisComments) -> Option<String> {
    tag.items()
        .filter(|(key, _)| {
            ["LYRICS", "UNSYNCEDLYRICS", "SYNCEDLYRICS"]
                .iter()
                .any(|k| key.eq_ignore_ascii_case(k))
        })
        .find_map(|(_, value)| clean(value))
}

fn number(value: &str) -> u32 {
    value
        .split(|c: char| !c.is_ascii_digit())
        .find(|s| !s.is_empty())
        .map_or(0, |digits| digits.parse().unwrap_or(u32::MAX))
}

fn year(value: &str) -> u32 {
    // The shared reader looks for a whole ASCII word, even in a date written in prose.
    value
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|s| s.len() == 4 && s.bytes().all(|b| b.is_ascii_digit()))
        .filter_map(|s| s.parse().ok())
        .find(|n| (1800..=2999).contains(n))
        .unwrap_or(0)
}

// String 1.x uses the original 80 ID3 genre names and spellings.
const GENRES: [&str; 80] = [
    "Blues",
    "Classic Rock",
    "Country",
    "Dance",
    "Disco",
    "Funk",
    "Grunge",
    "Hip-Hop",
    "Jazz",
    "Metal",
    "New Age",
    "Oldies",
    "Other",
    "Pop",
    "R&B",
    "Rap",
    "Reggae",
    "Rock",
    "Techno",
    "Industrial",
    "Alternative",
    "Ska",
    "Death Metal",
    "Pranks",
    "Soundtrack",
    "Euro-Techno",
    "Ambient",
    "Trip-Hop",
    "Vocal",
    "Jazz+Funk",
    "Fusion",
    "Trance",
    "Classical",
    "Instrumental",
    "Acid",
    "House",
    "Game",
    "Sound Clip",
    "Gospel",
    "Noise",
    "Alternative Rock",
    "Bass",
    "Soul",
    "Punk",
    "Space",
    "Meditative",
    "Instrumental Pop",
    "Instrumental Rock",
    "Ethnic",
    "Gothic",
    "Darkwave",
    "Techno-Industrial",
    "Electronic",
    "Pop-Folk",
    "Eurodance",
    "Dream",
    "Southern Rock",
    "Comedy",
    "Cult",
    "Gangsta",
    "Top 40",
    "Christian Rap",
    "Pop/Funk",
    "Jungle",
    "Native American",
    "Cabaret",
    "New Wave",
    "Psychedelic",
    "Rave",
    "Showtunes",
    "Trailer",
    "Lo-Fi",
    "Tribal",
    "Acid Punk",
    "Acid Jazz",
    "Polka",
    "Retro",
    "Musical",
    "Rock & Roll",
    "Hard Rock",
];
fn genre(value: &str) -> &str {
    let value = value.trim();
    let (index, suffix) = value
        .strip_prefix('(')
        .and_then(|s| s.split_once(')'))
        .unwrap_or((value, ""));
    if index.is_empty() || !index.bytes().all(|b| b.is_ascii_digit()) {
        return value;
    }
    if !suffix.trim().is_empty() {
        return suffix.trim();
    }
    index
        .parse::<usize>()
        .ok()
        .and_then(|i| GENRES.get(i))
        .copied()
        .unwrap_or("")
}

fn windows_text(bytes: &[u8]) -> String {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž',
        '\u{8f}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}',
        'ž', 'Ÿ',
    ];
    bytes
        .iter()
        .map(|&b| match b {
            0x80..=0x9f => HIGH[usize::from(b - 0x80)],
            _ => char::from(b),
        })
        .collect()
}

fn decode_text(enc: u8, bytes: &[u8]) -> String {
    match enc {
        0 => windows_text(bytes),
        3 => String::from_utf8_lossy(bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes))
            .into_owned(),
        _ => {
            let (big, bytes) = match bytes {
                [0xfe, 0xff, rest @ ..] => (true, rest),
                [0xff, 0xfe, rest @ ..] if enc != 2 => (false, rest),
                _ => (enc == 2, bytes),
            };
            let (pairs, tail) = bytes.as_chunks::<2>();
            let units = pairs.iter().map(|&p| {
                if big {
                    u16::from_be_bytes(p)
                } else {
                    u16::from_le_bytes(p)
                }
            });
            char::decode_utf16(units)
                .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
                .chain((!tail.is_empty()).then_some(char::REPLACEMENT_CHARACTER))
                .collect()
        }
    }
}

fn split_description(enc: u8, bytes: &[u8]) -> Option<(&[u8], &[u8])> {
    let width = if matches!(enc, 1 | 2) { 2 } else { 1 };
    let at = bytes
        .chunks_exact(width)
        .position(|c| c.iter().all(|&b| b == 0))?
        * width;
    Some((&bytes[..at], &bytes[at + width..]))
}

fn syncsafe(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0, |n, &b| (n << 7) | u32::from(b & 127))
}

fn unsync(bytes: &mut Vec<u8>) {
    let mut prev_ff = false;
    bytes.retain(|&b| {
        let keep = !(prev_ff && b == 0);
        prev_ff = b == 255;
        keep
    });
}

#[derive(Default)]
struct Id3 {
    meta: Metadata,
    picture: Option<Vec<u8>>,
    front_cover: bool,
}

impl Id3 {
    fn attached_picture(&mut self, version: u8, mut body: &[u8], want_picture: bool) {
        let version = match version {
            2 => Id3v2Version::V2,
            3 => Id3v2Version::V3,
            _ => Id3v2Version::V4,
        };
        let Ok(frame) = AttachedPictureFrame::parse(&mut body, Default::default(), version) else {
            return;
        };
        let pic = frame.picture;
        let front = pic.pic_type() == PictureType::CoverFront;
        if !pic.data().is_empty() && (front || self.meta.cover_mime.is_none()) {
            self.front_cover = front;
            self.meta.cover_mime =
                Some(pic.mime_type().map_or("image/jpeg", |m| m.as_str()).into());
            if want_picture {
                self.picture = Some(pic.into_owned().into_data());
            }
        }
    }

    fn text(&mut self, id: &[u8], body: &[u8]) {
        let Some((&enc, data)) = body.split_first() else {
            return;
        };
        let tags = &mut self.meta.tags;
        if matches!(id, b"USLT" | b"ULT" | b"TXXX") {
            let named = id == b"TXXX";
            let Some((desc, value)) = data
                .get(if named { 0 } else { 3 }..)
                .and_then(|s| split_description(enc, s))
            else {
                return;
            };
            let name = decode_text(enc, desc).trim().to_ascii_uppercase();
            if named
                && matches!(name.as_str(), "ALBUM ARTIST" | "ALBUMARTIST")
                && tags.album_artist.is_empty()
            {
                tags.album_artist = decode_text(enc, value)
                    .trim_end_matches('\0')
                    .replace('\0', "; ")
                    .trim()
                    .into();
            } else if self.meta.lyrics.is_none()
                && (!named
                    || matches!(
                        name.as_str(),
                        "LYRICS" | "SYNCEDLYRICS" | "UNSYNCEDLYRICS" | "USLT"
                    ))
            {
                self.meta.lyrics = clean(&decode_text(enc, value));
            }
            return;
        }
        let text = decode_text(enc, data)
            .trim_end_matches('\0')
            .replace('\0', "; ");
        let value = text.trim();
        match id {
            b"TIT2" | b"TT2" if tags.title.is_empty() => tags.title = value.into(),
            b"TPE1" | b"TP1" if tags.artist.is_empty() => tags.artist = value.into(),
            b"TPE2" | b"TP2" if tags.album_artist.is_empty() => tags.album_artist = value.into(),
            b"TALB" | b"TAL" if tags.album.is_empty() => tags.album = value.into(),
            b"TCON" | b"TCO" if tags.genre.is_empty() => tags.genre = genre(value).into(),
            b"TRCK" | b"TRK" if tags.track == 0 => tags.track = number(value),
            b"TPOS" | b"TPA" if tags.disc == 0 => tags.disc = number(value),
            b"TYER" | b"TDRC" | b"TDOR" | b"TYE" if tags.year == 0 => tags.year = year(value),
            b"TLEN" | b"TLE" if tags.duration == 0. => {
                tags.duration = f64::from(number(value)) / 1000.
            }
            _ => {}
        }
    }

    fn frames(
        &mut self,
        r: &mut (impl Read + Seek),
        end: u64,
        version: u8,
        flags: u8,
        want_picture: bool,
    ) -> io::Result<()> {
        let mut pos = r.stream_position()?;
        if flags & 0x40 != 0 && version > 2 {
            if end.saturating_sub(pos) < 4 {
                return Ok(());
            }
            let mut ext = [0; 4];
            r.read_exact(&mut ext)?;
            pos += if version == 4 {
                u64::from(syncsafe(&ext))
            } else {
                4 + u64::from(u32::from_be_bytes(ext))
            };
            r.seek(SeekFrom::Start(pos))?;
        }
        let (compressed_or_encrypted, grouped) = if version == 3 {
            (0xc0, 0x20)
        } else {
            (0x0c, 0x40)
        };
        let mut skip = 0;
        let header_size = if version == 2 { 6 } else { 10 };
        let mut header = [0; 10];
        while end.saturating_sub(pos) >= header_size {
            // Keep short consecutive fields in the same buffered read. Seek past large
            // artwork and fields we do not use without allocating their payloads.
            r.seek_relative(i64::from(skip))?;
            r.read_exact(&mut header[..header_size as usize])?;
            let (id, size, frame_flags) = match version {
                2 => (
                    &header[..3],
                    u32::from_be_bytes([0, header[3], header[4], header[5]]),
                    0,
                ),
                3 => (
                    &header[..4],
                    u32::from_be_bytes(header[4..8].try_into().unwrap()),
                    header[9],
                ),
                _ => (&header[..4], syncsafe(&header[4..8]), header[9]),
            };
            pos += header_size + u64::from(size);
            if !id
                .iter()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
                || pos > end
            {
                break;
            }
            skip = size;
            if size == 0 || frame_flags & compressed_or_encrypted != 0 {
                continue;
            }
            let prefix = u32::from(frame_flags & grouped != 0)
                + 4 * u32::from(version == 4 && frame_flags & 1 != 0);
            let Some(n) = size.checked_sub(prefix) else {
                continue;
            };
            let is_pic = matches!(id, b"APIC" | b"PIC");
            let limit = match id {
                b"APIC" | b"PIC" if !self.front_cover => {
                    if want_picture {
                        n
                    } else {
                        n.min(512)
                    }
                }
                b"USLT" | b"ULT" | b"TXXX" if !want_picture && n <= 1 << 20 => n,
                [b'T', ..] if !want_picture && n <= 4096 => n,
                _ => continue,
            };
            skip = n - limit;
            r.seek_relative(i64::from(prefix))?;
            let mut body = vec![0; limit as usize];
            r.read_exact(&mut body)?;
            if version == 4 && (frame_flags & 2 != 0 || flags & 0x80 != 0) {
                unsync(&mut body);
            }
            if is_pic {
                self.attached_picture(version, &body, want_picture);
            } else {
                self.text(id, &body);
            }
        }
        Ok(())
    }

    fn id3v1(&mut self, tail: &[u8; 128]) {
        if &tail[..3] != b"TAG" {
            return;
        }
        let t = &mut self.meta.tags;
        let field = |bytes: &[u8]| {
            windows_text(bytes.split(|&b| b == 0).next().unwrap_or_default())
                .trim()
                .to_owned()
        };
        if t.title.is_empty() {
            t.title = field(&tail[3..33]);
        }
        if t.artist.is_empty() {
            t.artist = field(&tail[33..63]);
        }
        if t.album.is_empty() {
            t.album = field(&tail[63..93]);
        }
        if t.year == 0 {
            t.year = year(&field(&tail[93..97]));
        }
        if t.track == 0 && tail[125] == 0 {
            t.track = u32::from(tail[126]);
        }
        if t.genre.is_empty() {
            t.genre = GENRES
                .get(usize::from(tail[127]))
                .copied()
                .unwrap_or("")
                .into();
        }
    }
}

fn id3_format(ty: Option<FileType>, header: bool) -> Option<&'static str> {
    match ty {
        Some(FileType::Mpeg) => Some("mp3"),
        Some(FileType::Aac) => Some("aac"),
        None if header => Some("mp3"),
        _ => None,
    }
}

fn read_id3_audio(r: &mut BufReader<File>, format: &str, want_picture: bool) -> io::Result<Id3> {
    let mut id3 = Id3::default();
    let size = r.get_ref().metadata()?.len();
    let mut audio_start = 0;
    let mut h = [0; 10];
    r.rewind()?;
    if size >= 10 {
        r.read_exact(&mut h)?;
        if &h[..3] == b"ID3" {
            let (ver, flags) = (h[3], h[5]);
            let len = u64::from(syncsafe(&h[6..]));
            let end = (10 + len).min(size);
            audio_start = 10 + len + 10 * u64::from(ver == 4 && flags & 16 != 0);
            if (2..=4).contains(&ver) {
                if ver < 4 && flags & 0x80 != 0 {
                    // v2.2/2.3 frame sizes refer to the tag after removing inserted zeroes.
                    let mut decoded = Vec::new();
                    r.take((end - 10).min(16 << 20)).read_to_end(&mut decoded)?;
                    unsync(&mut decoded);
                    let length = decoded.len() as u64;
                    id3.frames(&mut Cursor::new(decoded), length, ver, flags, want_picture)?;
                } else {
                    id3.frames(r, end, ver, flags, want_picture)?;
                }
            }
        }
    }
    if !want_picture {
        if size >= 128 {
            let mut tail = [0; 128];
            r.seek(SeekFrom::End(-128))?;
            r.read_exact(&mut tail)?;
            id3.id3v1(&tail);
        }
        let t = &mut id3.meta.tags;
        t.format = format.into();
        if format == "aac" {
            t.duration = adts_duration(r, audio_start, size)?;
        } else if let Some(seconds) = duration::mpeg(r, (t.duration > 0.).then_some(t.duration))? {
            t.duration = seconds;
        } else {
            // Retain the library's MPEG property fallback independently of tag parsing.
            r.rewind()?;
            let options = ParseOptions::new().max_junk_bytes(65536).read_tags(false);
            if let Ok(audio) = MpegFile::read_from(r, options) {
                t.duration = audio.properties().duration().as_secs_f64();
            }
        }
    }
    Ok(id3)
}

fn adts_duration(r: &mut BufReader<File>, start: u64, end: u64) -> io::Result<f64> {
    const RATES: [u32; 13] = [
        96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
    ];
    let mut samples = [0u64; RATES.len()];
    let mut pos = start;
    r.seek(SeekFrom::Start(pos))?;
    let mut h = [0; 7];
    while end.saturating_sub(pos) >= 7 {
        r.read_exact(&mut h)?;
        let index = usize::from((h[2] >> 2) & 15);
        let len = (u64::from(h[3] & 3) << 11) | (u64::from(h[4]) << 3) | u64::from(h[5] >> 5);
        let header = if h[1] & 1 == 0 { 9 } else { 7 };
        if h[0] != 255
            || h[1] & 0xf6 != 0xf0
            || index >= RATES.len()
            || len < header
            || len > end - pos
        {
            break;
        }
        samples[index] += 1024 * (1 + u64::from(h[6] & 3));
        pos += len;
        r.seek_relative((len - 7) as i64)?;
    }
    Ok(samples
        .iter()
        .zip(RATES)
        .map(|(&n, rate)| n as f64 / f64::from(rate))
        .sum())
}

// Matroska IDs retain their width marker; element sizes omit it. An all-ones size
// denotes an open-ended master (common on the Segment in streamed WebM files).
fn ebml_uint(r: &mut BufReader<File>, remaining: &mut u64, id: bool) -> io::Result<Option<u64>> {
    if *remaining == 0 {
        return Ok(None);
    }
    let mut bytes = [0; 8];
    r.read_exact(&mut bytes[..1])?;
    let width = (bytes[0].leading_zeros() + 1) as usize;
    if width > (if id { 4 } else { 8 }) || width as u64 > *remaining {
        return Ok(None);
    }
    r.read_exact(&mut bytes[1..width])?;
    if !id {
        bytes[0] &= (0xffu16 >> width) as u8;
    }
    let value = bytes[..width]
        .iter()
        .fold(0u64, |n, &b| (n << 8) | u64::from(b));
    *remaining -= width as u64;
    Ok(Some(if !id && value == (1 << (7 * width)) - 1 {
        u64::MAX
    } else {
        value
    }))
}

fn webm_duration(r: &mut BufReader<File>) -> io::Result<Option<f64>> {
    let mut end = r.get_ref().metadata()?.len();
    let mut scale = 1_000_000;
    let mut duration = None;
    // Descend only into Segment, then Info. Skip media payloads by their declared size.
    let mut parent = 0;
    let mut pos = 0;
    while pos < end {
        r.seek(SeekFrom::Start(pos))?;
        let mut remaining = end - pos;
        let Some(id) = ebml_uint(r, &mut remaining, true)? else {
            break;
        };
        let Some(size) = ebml_uint(r, &mut remaining, false)? else {
            break;
        };
        let size = if size == u64::MAX { remaining } else { size };
        if size > remaining {
            break;
        }
        let data = end - remaining;
        pos = data + size;
        match (parent, id) {
            (0, 0x18538067) | (0x18538067, 0x1549a966) => {
                parent = id;
                end = pos;
                pos = data;
            }
            (0x1549a966, 0x2ad7b1) if (1..=8).contains(&size) => {
                let mut b = [0; 8];
                r.read_exact(&mut b[8 - size as usize..])?;
                scale = u64::from_be_bytes(b);
            }
            (0x1549a966, 0x4489) if size == 4 => {
                let mut b = [0; 4];
                r.read_exact(&mut b)?;
                duration = Some(f64::from(f32::from_be_bytes(b)));
            }
            (0x1549a966, 0x4489) if size == 8 => {
                let mut b = [0; 8];
                r.read_exact(&mut b)?;
                duration = Some(f64::from_be_bytes(b));
            }
            _ => {}
        }
    }
    Ok(duration
        .map(|ticks| ticks * scale as f64 / 1e9)
        .filter(|n| n.is_finite() && *n > 0.))
}
