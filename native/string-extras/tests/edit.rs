use base64::{Engine, engine::general_purpose::STANDARD};
use lofty::{
    file::TaggedFileExt,
    prelude::{Accessor, ItemKey},
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    io::Write,
    os::windows::fs::OpenOptionsExt,
    path::Path,
    process::{Command, Output},
};
use string_core::{library::Track, state::State, tags::Tags};
use string_extras::edit::*;
use tempfile::tempdir;

#[test]
fn overrides_reset_and_swap_display_names() {
    let t = Track {
        path: r"M:\m\01 - File_name [ABCDEF01234].opus".into(),
        tags: Tags {
            artist: "Artist".into(),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut s = State::default();
    let id = t.id();
    assert_eq!(saved_info(&s, &id), Info::default());
    assert_eq!(display_names(&s, &t).title, "File name");
    s.fields["edits"][&id] = json!({"auto":true,"extra":4,"album":"Album","art":"cover.png"});
    swap_names(&mut s, &t);
    assert_eq!(
        s.fields["edits"][&id],
        json!({"title":"Artist","artist":"File name","album":"Album","art":"cover.png","extra":4})
    );
    edit_info(
        &mut s,
        &id,
        &Info {
            title: "  New  ".into(),
            artist: "  ".into(),
            art: "art.png".into(),
            ..Default::default()
        },
    );
    assert_eq!(
        s.fields["edits"][&id],
        json!({"title":"New","art":"art.png"})
    );
    assert_eq!(display_names(&s, &t).artist, "Artist");
    edit_info(&mut s, &id, &Info::default());
    assert!(s.fields["edits"].get(&id).is_none());
}
#[test]
fn atomic_replacement_keeps_first_original_and_rejects_failed_writes() {
    let tmp = tempdir().unwrap();
    let data = tmp.path().join("profile");
    let path = tmp.path().join("Track.FLAC");
    fs::write(&path, vec![1; 5000]).unwrap();
    let first = replace_song(&data, &path, &vec![2; 5000]).unwrap();
    let digest = Sha256::digest(path.to_str().unwrap().to_lowercase().as_bytes());
    let prefix = u32::from_be_bytes(digest[..4].try_into().unwrap());
    assert_eq!(
        first.backup,
        data.join("tag-backup")
            .join(format!("{prefix:08X} Track.FLAC"))
    );
    replace_song(&data, &path, &vec![3; 5000]).unwrap();
    assert_eq!(fs::read(&first.backup).unwrap(), vec![1; 5000]);
    let lock = fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&path)
        .unwrap();
    assert!(replace_song(&data, &path, &vec![4; 5000]).is_err());
    drop(lock);
    assert!(replace_song(&data, &path, &[0; 4095]).is_err());
    assert!(replace_song(&data, &tmp.path().join("absent.mp3"), &[0; 4096]).is_err());
    let other = tmp.path().join("track.txt");
    fs::write(&other, b"keep").unwrap();
    assert!(replace_song(&data, &other, &[0; 4096]).is_err());
    assert_eq!(fs::read(&path).unwrap(), vec![3; 5000]);
    assert_eq!(fs::read_dir(data.join("tag-backup")).unwrap().count(), 1);
    assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 3);
}
fn ff() -> Command {
    let mut c = Command::new(env::var_os("FFMPEG").unwrap_or_else(|| "ffmpeg".into()));
    c.args(["-hide_banner", "-nostdin", "-v", "error", "-threads", "1"]);
    c
}
fn run(c: &mut Command) -> Output {
    let out = c.output().unwrap();
    assert!(
        out.status.success(),
        "{c:?}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}
fn audio_hash(path: &Path, copy: bool) -> Vec<u8> {
    let mut cmd = ff();
    cmd.arg("-i").arg(path).args(["-map", "0:a:0"]);
    if copy {
        cmd.args(["-c:a", "copy"]);
    }
    run(cmd.args(["-f", "hash", "-hash", "sha256", "-"])).stdout
}

#[test]
fn incomplete_corrupt_and_chained_ogg_stay_byte_for_byte_untouched() {
    let tmp = tempdir().unwrap();
    let path = tmp.path().join("track.opus");
    run(ff()
        .args([
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=330:duration=2",
            "-c:a",
            "libopus",
        ])
        .arg(&path));
    let good = fs::read(&path).unwrap();
    let mut damaged = good.clone();
    *damaged.last_mut().unwrap() ^= 1;
    for bytes in [
        good[..good.len() - 3].to_vec(),
        damaged,
        [good.as_slice(), good.as_slice()].concat(),
    ] {
        fs::write(&path, &bytes).unwrap();
        assert!(
            write_tags(
                tmp.path(),
                &path,
                &TagChanges {
                    title: Some("New"),
                    ..Default::default()
                }
            )
            .is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert!(!tmp.path().join("tag-backup").exists());
    }
}
#[test]
fn named_field_writes_preserve_every_other_tag_pictures_lyrics_and_audio() {
    let tmp = tempdir().unwrap();
    let art = tmp.path().join("cover.png");
    run(ff()
        .args([
            "-f",
            "lavfi",
            "-i",
            "color=c=navy:s=32x32",
            "-frames:v",
            "1",
            "-threads",
            "1",
        ])
        .arg(&art));
    let png = fs::read(&art).unwrap();
    let mut pic = vec![];
    for n in [3u32, 9] {
        pic.write_all(&n.to_be_bytes()).unwrap();
    }
    pic.extend(b"image/png");
    for n in [0, 32, 32, 24, 0, png.len() as u32] {
        pic.extend(n.to_be_bytes());
    }
    pic.extend(&png);
    let picture = format!("METADATA_BLOCK_PICTURE={}", STANDARD.encode(pic));
    for (ext, codec) in [
        ("opus", "libopus"),
        ("ogg", "libvorbis"),
        ("flac", "flac"),
        ("mp3", "libmp3lame"),
        ("m4a", "aac"),
        ("aac", "aac"),
        ("wav", "pcm_s16le"),
    ] {
        let path = tmp.path().join(format!("track.{ext}"));
        let mut c = ff();
        c.args(["-f", "lavfi", "-i", "sine=frequency=587:duration=2"]);
        if matches!(ext, "mp3" | "m4a" | "flac") {
            c.arg("-i").arg(&art).args([
                "-map",
                "0:a:0",
                "-map",
                "1:v:0",
                "-c:v",
                "copy",
                "-disposition:v",
                "attached_pic",
            ]);
        }
        if ext == "aac" {
            c.args(["-write_id3v2", "1"]);
        }
        if ext == "mp3" {
            c.args(["-id3v2_version", "3"]);
        }
        if matches!(ext, "opus" | "ogg") {
            c.arg("-metadata").arg(&picture);
        }
        c.args(["-c:a", codec]);
        for v in [
            "title=Original",
            "artist=Band",
            "album=Record",
            "album_artist=Collection",
            "genre=Electronic",
            "track=2/5",
            "date=2020",
            "comment=Note",
            "lyrics=First line\nSecond line",
            "CUSTOM_FIELD=private annotation",
        ] {
            c.arg("-metadata").arg(v);
        }
        run(c.arg(&path));
        let before = fs::read(&path).unwrap();
        let tags = lofty::read_from_path(&path).unwrap();
        let packet_hash = audio_hash(&path, true);
        let pcm_hash = audio_hash(&path, false);
        let result = write_tags(
            tmp.path(),
            &path,
            &TagChanges {
                title: Some("Revised"),
                artist: Some("New band"),
                album: None,
            },
        )
        .unwrap_or_else(|e| panic!("{ext}: {e}"));
        assert_eq!(fs::read(result.backup).unwrap(), before, "{ext}");
        assert_eq!(audio_hash(&path, true), packet_hash, "{ext} packets");
        assert_eq!(audio_hash(&path, false), pcm_hash, "{ext} samples");
        let after = lofty::read_from_path(&path).unwrap();
        let main = after.primary_tag().unwrap();
        assert_eq!(main.title().as_deref(), Some("Revised"), "{ext}");
        assert_eq!(main.artist().as_deref(), Some("New band"), "{ext}");
        for old in tags.tags() {
            let new = after.tag(old.tag_type()).unwrap();
            let rest = |tag: &lofty::tag::Tag| {
                let mut items = tag
                    .items()
                    .filter(|i| !matches!(i.key(), ItemKey::TrackTitle | ItemKey::TrackArtist))
                    .map(|i| format!("{i:?}"))
                    .collect::<Vec<_>>();
                items.sort();
                items
            };
            assert_eq!(rest(new), rest(old), "{ext} unrelated metadata");
            assert_eq!(new.pictures(), old.pictures(), "{ext} cover metadata");
        }
        if matches!(ext, "opus" | "ogg" | "flac" | "mp3" | "m4a") {
            assert_eq!(main.pictures()[0].data(), png, "{ext} cover bytes");
            assert!(
                string_core::tags::read(&path)
                    .unwrap()
                    .lyrics
                    .unwrap()
                    .contains("Second line"),
                "{ext} lyrics"
            );
        }
    }
}
