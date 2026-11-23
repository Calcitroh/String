mod common;
use common::{Fake, song};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    thread,
};
use string_lyrics::{Lyrics, Source};
use tempfile::tempdir;

fn record(dir: &Path) -> (PathBuf, Value) {
    let path = fs::read_dir(dir.join("lyrics"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let json = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    (path, json)
}
#[test]
fn consent_concurrency_and_cached_online_hit() {
    let tmp = tempdir().unwrap();
    let fake = Fake::new("lrclib", "normal");
    let lyrics = Lyrics::with_base_url(tmp.path(), &fake.base);
    let path = tmp.path().join("音.mp3");
    let t = song(&path);
    assert_eq!(lyrics.get(&t, false, false).source, Source::None);
    assert_eq!(fake.read("/api/_seen"), json!([]));
    thread::scope(|s| {
        for _ in 0..6 {
            s.spawn(|| {
                let hits = lyrics.get(&t, true, false);
                assert!(hits.online && hits.synced);
                assert_eq!(hits.lines[0].text, "Glass orbit online one");
            });
        }
    });
    lyrics.forget(t.id);
    assert_eq!(lyrics.get(&t, false, false).source, Source::Lrclib);
    assert_eq!(
        fake.read("/api/_seen"),
        json!([{"path":"/api/get","query":{"track_name":"Glass Orbit","artist_name":"Test Signal","album_name":"Synthetic Weather","duration":"24"},"ua":"String v1.0 (personal music player)","client":"String v1.0"}])
    );
    let (_, r) = record(tmp.path());
    assert_eq!(
        (r["source"].as_str(), r["asked"].as_bool()),
        (Some("lrclib"), Some(true))
    );
}
#[test]
fn retry_a_miss_only_after_thirty_days() {
    let tmp = tempdir().unwrap();
    let fake = Fake::new("lrclib", "normal");
    let lyrics = Lyrics::with_base_url(tmp.path(), &fake.base);
    let path = tmp.path().join("absent.mp3");
    let mut t = song(&path);
    t.title = "雨 & 雪 + 50%?";
    let r = lyrics.get(&t, true, false);
    assert!(r.online && r.source == Source::None);
    let seen = fake.read("/api/_seen");
    assert_eq!(seen[0]["path"], "/api/get");
    assert_eq!(seen[1]["query"]["track_name"], t.title);
    let (file, mut r) = record(tmp.path());
    assert_eq!(r["asked"], true);
    for (age, requests) in [(29, 2), (31, 4)] {
        r["at"] = json!(chrono::Utc::now().timestamp_millis() - age * 86_400_000i64);
        fs::write(&file, r.to_string()).unwrap();
        lyrics.forget(t.id);
        lyrics.get(&t, true, false);
        assert_eq!(fake.read("/api/_seen").as_array().unwrap().len(), requests);
    }
    t.artist = "";
    lyrics.get(&t, true, true);
    assert_eq!(fake.read("/api/_seen")[4]["query"], json!({"q":t.title}));
}
#[test]
fn busy_offline_and_invalid_replies_never_persist_an_ask() {
    let tmp = tempdir().unwrap();
    let path = tmp.path().join("a.mp3");
    let t = song(&path);
    for scenario in ["429", "503", "invalid"] {
        let fake = Fake::new("lrclib", scenario);
        let lyrics = Lyrics::with_base_url(tmp.path(), &fake.base);
        let r = lyrics.get(&t, true, true);
        assert!(r.source == Source::None && !r.online);
        assert_eq!(r.busy, scenario == "429");
        assert!(!tmp.path().join("lyrics").exists());
    }
    fs::write(path.with_extension("lrc"), "a plain verse").unwrap();
    let fake = Fake::new("lrclib", "normal");
    let base = fake.base.clone();
    drop(fake);
    let lyrics = Lyrics::with_base_url(tmp.path(), &base);
    let r = lyrics.get(&t, true, false);
    assert!(r.retry && r.source == Source::Lrc && !r.busy);
    assert!(!tmp.path().join("lyrics").exists());
}
#[test]
fn sidecar_precedes_embedded_tags_and_plain_lyrics_can_be_upgraded() {
    let tmp = tempdir().unwrap();
    let fake = Fake::new("lrclib", "normal");
    let path = tmp.path().join("a.mp3");
    // ID3v2.3 USLT followed by three silent MPEG-1 Layer III frames.
    let content = b"\0eng\0embedded plain lyrics";
    let mut tag = b"USLT".to_vec();
    tag.extend((content.len() as u32).to_be_bytes());
    tag.extend([0, 0]);
    tag.extend(content);
    let mut audio = b"ID3\x03\0\0\0\0\0".to_vec();
    audio.push(tag.len() as u8);
    audio.extend(tag);
    let mut frame = vec![0; 417];
    frame[..4].copy_from_slice(&[0xff, 0xfb, 0x90, 0x00]);
    audio.extend(frame.repeat(3));
    fs::write(&path, audio).unwrap();
    let t = song(&path);
    let lyrics = Lyrics::with_base_url(tmp.path(), &fake.base);
    let r = lyrics.get(&t, false, false);
    assert_eq!(
        (r.source, r.text.as_str()),
        (Source::File, "embedded plain lyrics")
    );
    let mut utf16 = vec![0xff, 0xfe];
    utf16.extend("[00:01] sidecar".encode_utf16().flat_map(u16::to_le_bytes));
    fs::write(path.with_extension("lrc"), utf16).unwrap();
    assert_eq!(lyrics.get(&t, true, true).source, Source::Lrc);
    assert_eq!(fake.read("/api/_seen"), json!([]));
    fs::write(path.with_extension("lrc"), "plain").unwrap();
    assert_eq!(lyrics.get(&t, true, true).source, Source::Lrclib);
}
#[test]
fn plain_local_after_successful_miss_and_instrumental_hit() {
    let tmp = tempdir().unwrap();
    let path = tmp.path().join("absent.mp3");
    let mut t = song(&path);
    t.title = "Other";
    fs::write(path.with_extension("lrc"), "one line").unwrap();
    let fake = Fake::new("lrclib", "normal");
    let lyrics = Lyrics::with_base_url(tmp.path(), &fake.base);
    let r = lyrics.get(&t, true, false);
    assert_eq!(r.source, Source::Lrc);
    assert!(!r.online);
    let (_, c) = record(tmp.path());
    assert_eq!(c["asked"], true);
    lyrics.forget(t.id);
    lyrics.get(&t, true, false);
    assert_eq!(fake.read("/api/_seen").as_array().unwrap().len(), 2);
    let fake = Fake::new("lrclib", "instrumental");
    let r = Lyrics::with_base_url(tmp.path(), &fake.base).get(&t, true, true);
    assert!(r.instrumental && r.online && r.lines.is_empty());
}

#[test]
fn legacy_unasked_record_upgrades_through_search() {
    let tmp = tempdir().unwrap();
    let path = tmp.path().join("song.mp3");
    let t = song(&path);
    let fake = Fake::new("lrclib", "search");
    let lyrics = Lyrics::with_base_url(tmp.path(), &fake.base);
    fs::write(path.with_extension("lrc"), "plain").unwrap();
    lyrics.get(&t, false, false);
    let (file, mut c) = record(tmp.path());
    c.as_object_mut().unwrap().remove("asked");
    fs::write(&file, c.to_string()).unwrap();
    lyrics.forget(t.id);
    let found = lyrics.get(&t, true, false);
    assert!(found.online && found.synced && found.source == Source::Lrclib);
    assert_eq!(found.lines[0].text, "match");
    assert_eq!(record(tmp.path()).1["asked"], true);
}
