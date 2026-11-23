//! Read local metadata and embedded lyrics without changing a media file.

mod duration;

use lofty::{
    config::ParseOptions,
    file::{AudioFile, FileType, TaggedFile, TaggedFileExt},
    id3::v2::Frame,
    mpeg::MpegFile,
    picture::PictureType,
    prelude::{Accessor, ItemKey},
    probe::Probe,
};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{self, BufRead, BufReader},
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
                ..Default::default()
            },
            ..Default::default()
        });
    }
    let options = ParseOptions::new().max_junk_bytes(65536);
    let probe = Probe::new(&mut reader).options(options).guess_file_type()?;
    let mut lyrics = None;
    let mut tag_seconds = None;
    let file: TaggedFile = if probe.file_type() == Some(FileType::Mpeg) {
        let f = MpegFile::read_from(&mut probe.into_inner(), options).map_err(io::Error::other)?;
        if let Some(tag) = f.id3v2() {
            tag_seconds = tag.into_iter().find_map(|f| match f {
                Frame::Text(x) if x.id().as_str() == "TLEN" => x
                    .value
                    .parse::<u64>()
                    .ok()
                    .filter(|n| *n > 0)
                    .map(|n| n as f64 / 1000.),
                _ => None,
            });
            lyrics = tag.into_iter().find_map(|frame| match frame {
                Frame::UnsynchronizedText(x) => clean(&x.content),
                Frame::UserText(x)
                    if matches!(
                        x.description.to_ascii_uppercase().as_str(),
                        "USLT" | "LYRICS" | "UNSYNCEDLYRICS" | "SYNCEDLYRICS"
                    ) =>
                {
                    clean(&x.content)
                }
                _ => None,
            });
        }
        f.into()
    } else {
        probe.read().map_err(io::Error::other)?
    };
    let format = match file.file_type() {
        FileType::Mpeg => "mp3",
        FileType::Mp4 => "m4a",
        FileType::Aac => "aac",
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
    tags.duration = match file.file_type() {
        FileType::Mpeg => duration::mpeg(&mut reader, tag_seconds)?.unwrap_or(tags.duration),
        FileType::Mp4 => duration::movie(&mut reader)?.unwrap_or(tags.duration),
        FileType::Aac => 0.,
        _ => tags.duration,
    };
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
            (&mut tags.genre, tag.genre()),
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
            tags.track = tag.track().unwrap_or(0);
        }
        if tags.disc == 0 {
            tags.disc = tag.disk().unwrap_or(0);
        }
        if tags.year == 0 {
            tags.year = tag.date().map_or(0, |d| d.year.into());
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
    let options = ParseOptions::new()
        .max_junk_bytes(65536)
        .read_properties(false);
    let file = Probe::new(&mut reader)
        .options(options)
        .guess_file_type()?
        .read()
        .map_err(io::Error::other)?;
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
