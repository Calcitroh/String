//! Display overrides and explicit edits to existing song files.
mod ogg;
use crate::{
    object,
    text::{file_title, js_trim},
};
use lofty::{
    aac::AacFile,
    config::{ParseOptions, ParsingMode, WriteOptions},
    file::{AudioFile, FileType, TaggedFile, TaggedFileExt},
    flac::FlacFile,
    id3::v2::{Id3v2Tag, Id3v2Version},
    iff::wav::WavFile,
    mp4::Mp4File,
    mpeg::MpegFile,
    ogg::{OggPictureStorage, OpusFile, VorbisFile},
    prelude::{Accessor, ItemKey, TagExt},
    probe::Probe,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    ffi::c_void,
    fs::{self, File, OpenOptions},
    io::{self, BufReader, Seek, Write},
    os::windows::{ffi::OsStrExt, fs::OpenOptionsExt},
    path::{Path, PathBuf},
    ptr,
    sync::Mutex,
};
use string_core::{Fields, library::Track, state::State};
use tempfile::{Builder, NamedTempFile};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Info {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub art: String,
}
/// Explicit dialog values. Empty boxes follow the song's own metadata.
pub fn saved_info(state: &State, id: &str) -> Info {
    let e = &state.fields["edits"][id];
    let value = |key: &str| e[key].as_str().unwrap_or("").to_owned();
    Info {
        title: value("title"),
        artist: value("artist"),
        album: value("album"),
        art: value("art"),
    }
}
pub fn display_names(state: &State, track: &Track) -> Info {
    let mut info = saved_info(state, &track.id());
    if info.title.is_empty() {
        info.title = if track.tags.title.is_empty() {
            file_title(&track.path)
        } else {
            track.tags.title.clone()
        };
    }
    if info.artist.is_empty() {
        info.artist.clone_from(&track.tags.artist);
    }
    if info.album.is_empty() {
        info.album.clone_from(&track.tags.album);
    }
    info
}
/// Save the dialog values. Empty text restores the embedded tag. An empty form resets all overrides.
pub fn edit_info(state: &mut State, id: &str, info: &Info) {
    let mut out = Fields::new();
    for (key, value) in [
        ("title", &info.title),
        ("artist", &info.artist),
        ("album", &info.album),
    ] {
        let value = js_trim(value);
        if !value.is_empty() {
            out.insert(key.into(), value.into());
        }
    }
    if !info.art.is_empty() {
        out.insert("art".into(), info.art.clone().into());
    }
    let edits = object(&mut state.fields, "edits");
    if out.is_empty() {
        edits.remove(id);
    } else {
        edits.insert(id.into(), Value::Object(out));
    }
}
pub fn swap_names(state: &mut State, track: &Track) {
    let names = display_names(state, track);
    let edit = object(object(&mut state.fields, "edits"), &track.id());
    edit.insert("title".into(), names.artist.into());
    edit.insert("artist".into(), names.title.into());
    edit.remove("auto");
}

#[derive(Debug)]
pub struct Replacement {
    pub path: PathBuf,
    pub backup: PathBuf,
}
#[derive(Debug, Default)]
pub struct TagChanges<'a> {
    pub title: Option<&'a str>,
    pub artist: Option<&'a str>,
    pub album: Option<&'a str>,
}

fn invalid(s: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, s)
}
fn check_song(path: &Path) -> io::Result<PathBuf> {
    if !path.is_absolute() {
        return Err(invalid("A full song path is required"));
    }
    let path = std::path::absolute(path)?;
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !matches!(
        ext.as_str(),
        "mp3" | "wav" | "ogg" | "oga" | "opus" | "flac" | "m4a" | "m4b" | "aac" | "weba"
    ) {
        return Err(invalid("Only song files can be rewritten"));
    }
    if !fs::metadata(&path)?.is_file() {
        return Err(invalid("Expected an existing song file"));
    }
    Ok(path)
}
fn backup_path(data: &Path, path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    // .NET invariant casing uses simple mappings and keeps capital dotted I.
    let lower: String = s
        .chars()
        .map(|c| {
            if c == 'İ' {
                c
            } else {
                c.to_lowercase().next().unwrap()
            }
        })
        .collect();
    let hash = Sha256::digest(lower.as_bytes());
    let prefix = u32::from_be_bytes(hash[..4].try_into().unwrap());
    let mut name = std::ffi::OsString::from(format!("{prefix:08X} "));
    name.push(path.file_name().unwrap());
    data.join("tag-backup").join(name)
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn ReplaceFileW(
        target: *const u16,
        source: *const u16,
        backup: *const u16,
        flags: u32,
        exclude: *mut c_void,
        reserved: *mut c_void,
    ) -> i32;
}
fn atomic_replace(source: &Path, target: &Path) -> io::Result<()> {
    let wide = |p: &Path| {
        p.as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>()
    };
    let (source, target) = (wide(source), wide(target));
    // Both names are NUL-terminated, and the replacement must already exist on this volume.
    let ok = unsafe {
        ReplaceFileW(
            target.as_ptr(),
            source.as_ptr(),
            ptr::null(),
            0,
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
fn replace(
    data: &Path,
    path: &Path,
    prepare: impl FnOnce(&mut File, &mut File) -> io::Result<()>,
) -> io::Result<Replacement> {
    static WRITER: Mutex<()> = Mutex::new(());
    let _guard = WRITER.lock().unwrap_or_else(|e| e.into_inner());
    let path = check_song(path)?;
    let mut original = OpenOptions::new().read(true).share_mode(1).open(&path)?;
    let mut staged = Builder::new()
        .prefix(".string-")
        .make_in(path.parent().unwrap(), |p| {
            OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(p)
        })?;
    prepare(&mut original, staged.as_file_mut())?;
    if staged.as_file().metadata()?.len() < 4096 {
        return Err(invalid("That isn't a whole song"));
    }
    staged.as_file().sync_all()?;
    let backup = backup_path(data, &path);
    let dir = backup.parent().unwrap();
    fs::create_dir_all(dir)?;
    if !backup.try_exists()? {
        let tmp = NamedTempFile::new_in(dir)?.into_temp_path();
        // CopyFile also keeps the source's last-write time and NTFS streams.
        fs::copy(&path, &tmp)?;
        OpenOptions::new().write(true).open(&tmp)?.sync_all()?;
        match tmp.persist_noclobber(&backup) {
            Ok(()) => {}
            Err(e) if e.error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.error),
        }
    }
    if !backup.is_file() {
        return Err(invalid("The original backup is not a file"));
    }
    drop(original);
    let tmp = staged.into_temp_path();
    atomic_replace(&tmp, &path)?;
    Ok(Replacement { path, backup })
}
pub fn replace_song(data: &Path, path: &Path, bytes: &[u8]) -> io::Result<Replacement> {
    replace(data, path, |_, out| out.write_all(bytes))
}
fn apply<T: Accessor>(tag: &mut T, changes: &TagChanges<'_>) {
    if let Some(s) = changes.title {
        if s.is_empty() {
            tag.remove_title()
        } else {
            tag.set_title(s.into())
        }
    }
    if let Some(s) = changes.artist {
        if s.is_empty() {
            tag.remove_artist()
        } else {
            tag.set_artist(s.into())
        }
    }
    if let Some(s) = changes.album {
        if s.is_empty() {
            tag.remove_album()
        } else {
            tag.set_album(s.into())
        }
    }
}
fn change_tag<T: TagExt>(
    mut tag: T,
    changes: &TagChanges<'_>,
    file: &mut File,
    options: WriteOptions,
) -> io::Result<()> {
    apply(&mut tag, changes);
    file.rewind()?;
    tag.save_to(file, options).map_err(io::Error::other)
}
fn write_id3(
    tag: Option<Id3v2Tag>,
    changes: &TagChanges<'_>,
    file: &mut File,
    mut options: WriteOptions,
) -> io::Result<()> {
    let tag = tag.unwrap_or_default();
    let options = options.use_id3v23(tag.original_version() == Id3v2Version::V3);
    change_tag(tag, changes, file, options)
}

fn read_tags(file: &mut File) -> io::Result<TaggedFile> {
    file.rewind()?;
    Probe::new(BufReader::new(file))
        .guess_file_type()?
        .options(
            ParseOptions::new()
                .parsing_mode(ParsingMode::Strict)
                .read_properties(false),
        )
        .read()
        .map_err(io::Error::other)
}
fn verify_tags(
    before: &TaggedFile,
    after: &TaggedFile,
    changes: &TagChanges<'_>,
) -> io::Result<()> {
    let primary = after.primary_tag();
    for (wanted, got) in [
        (changes.title, primary.and_then(Accessor::title)),
        (changes.artist, primary.and_then(Accessor::artist)),
        (changes.album, primary.and_then(Accessor::album)),
    ] {
        if let Some(wanted) = wanted
            && got.as_deref().unwrap_or("") != wanted
        {
            return Err(invalid(
                "The tag format did not preserve the requested name",
            ));
        }
    }
    let items = |tag: &lofty::tag::Tag| {
        let mut counts = HashMap::new();
        for item in tag.items() {
            let changed = match item.key() {
                ItemKey::TrackTitle => changes.title.is_some(),
                ItemKey::TrackArtist => changes.artist.is_some(),
                ItemKey::AlbumTitle => changes.album.is_some(),
                _ => false,
            };
            if !changed {
                *counts.entry(item.clone()).or_insert(0usize) += 1;
            }
        }
        counts
    };
    for old in before.tags() {
        let new = after.tag(old.tag_type());
        if old.pictures() != new.map_or(&[][..], |tag| tag.pictures())
            || items(old) != new.map(items).unwrap_or_default()
        {
            return Err(invalid(
                "Rewriting this file would change unrelated metadata",
            ));
        }
    }
    Ok(())
}
/// None leaves a field alone. An empty value removes the field. Edit the native
/// tag type directly so unrelated frames, atoms and comments keep their data.
pub fn write_tags(data: &Path, path: &Path, changes: &TagChanges<'_>) -> io::Result<Replacement> {
    if changes.title.is_none() && changes.artist.is_none() && changes.album.is_none() {
        return Err(invalid("Choose a tag field to change"));
    }
    replace(data, path, |original, out| {
        io::copy(original, out)?;
        out.rewind()?;
        let before = read_tags(original)?;
        let ty = before.file_type();
        original.rewind()?;
        let mut reader = BufReader::new(original);
        let audio = if matches!(ty, FileType::Opus | FileType::Vorbis) {
            let hash = ogg::packets(&mut reader)?;
            reader.rewind()?;
            Some(hash)
        } else {
            None
        };
        let opts = ParseOptions::new()
            .parsing_mode(ParsingMode::Strict)
            .read_properties(false)
            .implicit_conversions(false);
        let write = WriteOptions::new().lossy_text_encoding(false);
        macro_rules! read {
            ($ty:ty) => {
                <$ty>::read_from(&mut reader, opts).map_err(io::Error::other)?
            };
        }
        match ty {
            FileType::Mpeg => write_id3(read!(MpegFile).remove_id3v2(), changes, out, write),
            FileType::Aac => write_id3(read!(AacFile).remove_id3v2(), changes, out, write),
            FileType::Wav => {
                let mut wav = read!(WavFile);
                if let Some(info) = wav.remove_riff_info() {
                    change_tag(info, changes, out, write)?;
                }
                // ID3 takes priority for WAV files in the library reader.
                write_id3(wav.remove_id3v2(), changes, out, write)
            }
            FileType::Mp4 => change_tag(
                read!(Mp4File).remove_ilst().unwrap_or_default(),
                changes,
                out,
                write,
            ),
            FileType::Opus => change_tag(
                read!(OpusFile).remove_vorbis_comments(),
                changes,
                out,
                write,
            ),
            FileType::Vorbis => change_tag(
                read!(VorbisFile).remove_vorbis_comments(),
                changes,
                out,
                write,
            ),
            FileType::Flac => {
                let mut flac = read!(FlacFile);
                let mut comments = flac.remove_vorbis_comments().unwrap_or_default();
                for (pic, info) in flac.remove_pictures() {
                    comments
                        .insert_picture(pic, Some(info))
                        .map_err(io::Error::other)?;
                }
                change_tag(comments, changes, out, write)
            }
            _ => Err(invalid("This song format has no supported tag writer")),
        }?;
        if let Some(expected) = audio {
            out.rewind()?;
            if ogg::packets(BufReader::new(&mut *out))? != expected {
                return Err(invalid(
                    "Rewriting this file would change its audio packets",
                ));
            }
        }
        verify_tags(&before, &read_tags(out)?, changes)
    })
}
