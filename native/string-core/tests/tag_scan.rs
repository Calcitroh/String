//! Made-song regressions for the library reader. The encoded fixtures contain a quarter second of silence.
use std::{
    fs,
    path::{Path, PathBuf},
};
use string_core::{library::Library, tags};
use tempfile::tempdir;

fn fixture(extension: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/tag-scan")
        .join(format!("tone.{extension}"))
}

fn mp3(path: &Path, frames: &[(&[u8; 4], &[u8])], tail: &[u8]) {
    let mut body = Vec::new();
    for (id, data) in frames {
        body.extend_from_slice(*id);
        body.extend_from_slice(&(data.len() as u32).to_be_bytes());
        body.extend([0, 0]);
        body.extend_from_slice(data);
    }
    let mut song = Vec::new();
    if !body.is_empty() {
        song.extend_from_slice(b"ID3\x03\0\0");
        song.extend([21, 14, 7, 0].map(|shift| ((body.len() >> shift) & 127) as u8));
        song.extend(body);
    }
    let mut frame = [0; 417];
    frame[..4].copy_from_slice(&[0xff, 0xfb, 0x90, 0]);
    for _ in 0..10 {
        song.extend(frame);
    }
    song.extend(tail);
    fs::write(path, song).unwrap();
}

#[test]
fn malformed_mp3_fields_preserve_tags_and_length() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("song.mp3");
    for (id, data) in [
        (b"TDRC", &b"\0May 2009"[..]),
        (b"TYER", b"\0unknown"),
        (b"COMM", b"\0e"),
        (b"USLT", b"\0"),
        (b"APIC", b"\0im"),
        (b"TCOM", b"\x07bad"),
        (b"TCOM", b"\x01A\0"),
        (b"TCOM", b"\x03\xff"),
    ] {
        mp3(
            &path,
            &[(b"TIT2", b"\0Before"), (id, data), (b"TPE1", b"\0After")],
            &[],
        );
        let t = tags::read(&path).unwrap().tags;
        assert_eq!((t.title.as_str(), t.artist.as_str()), ("Before", "After"));
        assert_eq!(t.duration, 4170. * 8. / 128000.);
    }
}

#[test]
fn id3_decodes_old_text_and_joins_within_one_frame() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("song.mp3");
    mp3(
        &path,
        &[
            (b"TIT2", b"\0Don\x92t\x85"),
            (b"TPE1", b"\0First\0Second\0"),
            (b"TALB", b"\x01L\0E\0"),
        ],
        &[],
    );
    let t = tags::read(&path).unwrap().tags;
    assert_eq!(
        (t.title.as_str(), t.artist.as_str(), t.album.as_str()),
        ("Don’t…", "First; Second", "LE")
    );
    let mut tail = [0; 128];
    tail[..7].copy_from_slice(b"TAG\x80One");
    mp3(&path, &[], &tail);
    assert_eq!(tags::read(&path).unwrap().tags.title, "€One");
}

#[test]
fn dates_numbers_and_genres_follow_the_written_text() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("song.mp3");
    for (value, year) in [
        ("20120315", 0),
        ("May 2009", 2009),
        ("(2009)", 2009),
        ("2009/01/01", 2009),
        ("01.02.2009", 2009),
        ("2009-13-01", 2009),
        ("2009;2010", 2009),
        ("unknown", 0),
        ("x2009", 0),
        ("_2009", 0),
        ("1799", 0),
        ("1800", 1800),
        ("2999", 2999),
    ] {
        let date = format!("\0{value}");
        mp3(
            &path,
            &[
                (b"TDRC", date.as_bytes()),
                (b"TRCK", b"\0A1"),
                (b"TPOS", b"\0CD2"),
                (b"TCON", b"\0(17)"),
            ],
            &[],
        );
        let t = tags::read(&path).unwrap().tags;
        assert_eq!(
            (t.year, t.track, t.disc, t.genre.as_str()),
            (year, 1, 2, "Rock"),
            "{value}"
        );
    }
    for (ext, year) in [("flac", 2009), ("opus", 0), ("ogg", 0), ("m4a", 0)] {
        let t = tags::read(&fixture(ext)).unwrap().tags;
        assert_eq!(t.year, year, "{ext}");
        if ext != "m4a" {
            assert_eq!((t.track, t.disc, t.genre.as_str()), (1, 2, "Rock"), "{ext}");
        }
    }
}

#[test]
fn synced_vorbis_comment_is_available_in_each_container() {
    for ext in ["flac", "ogg", "opus"] {
        assert_eq!(
            tags::read(&fixture(ext)).unwrap().lyrics.as_deref(),
            Some("[00:00.10]First line"),
            "{ext}"
        );
    }
}

#[test]
fn mp4_binary_genre_remains_one_based() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("genre.m4a");
    let mut bytes = fs::read(fixture("m4a")).unwrap();
    let at = bytes.windows(4).position(|w| w == b"\xa9gen").unwrap();
    bytes[at..at + 4].copy_from_slice(b"gnre");
    assert_eq!(&bytes[at + 8..at + 12], b"data");
    bytes[at + 12..at + 16].fill(0);
    bytes[at + 20..at + 24].copy_from_slice(&[0, 18, 0, 0]);
    fs::write(&file, bytes).unwrap();
    assert_eq!(tags::read(&file).unwrap().tags.genre, "Rock");
}

#[test]
fn picture_and_lyrics_survive_a_rejected_neighbor() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("song.mp3");
    mp3(
        &file,
        &[
            (b"COMM", b"\0e"),
            (b"APIC", b"\0image/png\0\x03\0\x89PNG"),
            (b"USLT", b"\0eng\0Line one\r\nLine two"),
            (b"APIC", b"\0im"),
        ],
        &[],
    );
    let meta = tags::read(&file).unwrap();
    assert_eq!(meta.cover_mime.as_deref(), Some("image/png"));
    assert_eq!(meta.lyrics.as_deref(), Some("Line one\nLine two"));
    assert_eq!(
        tags::picture(&file).unwrap().as_deref(),
        Some(b"\x89PNG".as_slice())
    );
}

#[test]
fn scan_gets_adts_and_web_audio_lengths() {
    let dir = tempdir().unwrap();
    for (source, ext) in [("aac", "aac"), ("webm", "webm"), ("webm", "weba")] {
        fs::copy(fixture(source), dir.path().join(format!("song.{ext}"))).unwrap();
    }
    let mut library = Library::default();
    let report = library.scan(&[dir.path().to_owned()], &[]);
    assert!(report.issues.is_empty());
    assert_eq!(library.tracks.len(), 3);
    for t in library.tracks {
        let expected = if t.tags.format == "aac" {
            13. * 1024. / 48000.
        } else {
            0.258
        };
        assert!(
            (t.tags.duration - expected).abs() < 0.00001,
            "{} {}",
            t.tags.format,
            t.tags.duration
        );
    }
}

fn assert_siblings(root: &Path) {
    fs::copy(fixture("flac"), root.join("a.flac")).unwrap();
    fs::copy(fixture("flac"), root.join("z.flac")).unwrap();
    let mut library = Library::default();
    let roots = [root.to_owned()];
    assert_eq!(library.scan(&roots, &[]).read, 2);
    fs::copy(fixture("flac"), root.join("zz.flac")).unwrap();
    let warm = library.scan(&roots, &[]);
    assert_eq!((warm.read, warm.reused, library.tracks.len()), (1, 2, 3));
}

#[test]
fn dangling_directory_link_does_not_hide_siblings() {
    let dir = tempdir().unwrap();
    let target = dir.path().join("target");
    fs::create_dir(&target).unwrap();
    let link = dir.path().join("m");
    #[cfg(windows)]
    assert!(
        std::process::Command::new("cmd")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&link)
            .arg(&target)
            .output()
            .unwrap()
            .status
            .success()
    );
    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &link).unwrap();
    fs::remove_dir(&target).unwrap();
    assert_siblings(dir.path());
}

#[test]
fn undecodable_name_does_not_hide_siblings() {
    let dir = tempdir().unwrap();
    #[cfg(windows)]
    let name = {
        use std::os::windows::ffi::OsStringExt;
        std::ffi::OsString::from_wide(&[109, 0xd800])
    };
    #[cfg(unix)]
    let name = {
        use std::os::unix::ffi::OsStringExt;
        std::ffi::OsString::from_vec(vec![109, 255])
    };
    fs::write(dir.path().join(name), b"").unwrap();
    assert_siblings(dir.path());
}
