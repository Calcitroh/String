//! Integration with the real JavaScript codecs and generated audio fixtures.

use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Command,
};
use string_core::{data::DataFolder, tags};
use tempfile::tempdir;

fn media() -> &'static Path {
    static MEDIA: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    MEDIA.get_or_init(|| serde_json::from_slice(&reference("media", Path::new("."))).unwrap())
}

fn reference(mode: &str, dir: &Path) -> Vec<u8> {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/reference.cjs");
    let out = Command::new("node")
        .arg(script)
        .arg(mode)
        .arg(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

#[test]
fn javascript_rust_javascript_profile() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    reference("write", dir);
    std::fs::copy(dir.join("state.json"), dir.join("original-state")).unwrap();
    // Node compares full decoded records and wire values (including lone surrogate fragments).
    let before = reference("read", dir);
    let data = DataFolder::new(dir, dir.join("copies"));
    let mut p = data.open().unwrap();
    assert_eq!(p.state.value.queue.items().len(), 4);
    assert!(p.state.value.queue.items()[1].manual);
    assert_eq!(p.state.value.queue.context(Some(0))["kind"], "liked");
    assert_eq!(p.library.value.visible(&p.state.value).count(), 0);
    assert_eq!(p.library.value.tracks[0].id(), "m:\\music\\one.mp3");
    assert_eq!(p.listening.value.track_count(), 2);
    p.state.save().unwrap();
    p.listening.save().unwrap();
    p.library.save().unwrap();
    let after = reference("read", dir);
    std::fs::write(dir.join("before"), before).unwrap();
    std::fs::write(dir.join("after"), after).unwrap();
    reference("compare", dir);
    let mut p = data.open().unwrap();
    p.state.value.queue.seek(24.5);
    p.library.value.tracks[0].tags.title = "Retitled".into();
    let at = "2026-03-08T03:00:00".parse().unwrap();
    p.listening.value.add(
        &string_core::stats::Track("extra".into(), "One".into(), "A".into()),
        2.,
        at,
    );
    p.state.save().unwrap();
    p.library.save().unwrap();
    p.listening.save().unwrap();
    let out = reference("read", dir);
    let out = String::from_utf8(out).unwrap();
    assert!(
        out.contains("\"pos\":24.5")
            && out.contains("Retitled")
            && out.contains("[\"extra\",\"One\",\"A\"]")
    );
}

#[test]
fn generated_media_matches_javascript() {
    media();
    let rows: Vec<Value> = serde_json::from_slice(&reference("tags", Path::new("."))).unwrap();
    assert_eq!(rows.len(), 8);
    let mut differences = vec![];
    for row in rows {
        let path = PathBuf::from(row["path"].as_str().unwrap());
        let actual = tags::read(&path).unwrap();
        let fields = serde_json::to_value(&actual.tags).unwrap();
        for (k, expected) in row["tags"].as_object().unwrap() {
            let fallback = if expected.is_number() {
                json!(0)
            } else {
                json!("")
            };
            let got = fields.get(k).unwrap_or(&fallback);
            let equal = if expected.is_number() {
                expected.as_f64() == got.as_f64()
            } else {
                expected == got
            };
            if !equal {
                differences.push(format!(
                    "{} {k}: JS {expected}, Rust {got}",
                    path.file_name().unwrap().to_string_lossy()
                ));
            }
        }
        for (k, got) in [
            ("hasCover", json!(actual.cover_mime.is_some())),
            ("cover", json!(actual.cover_mime)),
            ("lyrics", json!(actual.lyrics)),
        ] {
            if row[k] != got {
                differences.push(format!("{} {k}: JS {}, Rust {got}", path.display(), row[k]));
            }
        }
    }
    assert!(differences.is_empty(), "\n{}", differences.join("\n"));
}

#[test]
fn both_scanners_reuse_each_others_cache() {
    let fixtures = media().to_owned();
    let js = tempdir().unwrap();
    let initial: Value = serde_json::from_slice(&reference("scan", js.path())).unwrap();
    assert_eq!(initial["reads"], 8);
    let folder = DataFolder::new(js.path(), js.path().join("backup"));
    let mut cache = folder.read::<string_core::library::Library>().unwrap();
    let warm = cache.value.scan(std::slice::from_ref(&fixtures), &[]);
    assert_eq!((warm.read, warm.reused, warm.covers_changed), (0, 8, 0));
    let rs = tempdir().unwrap();
    let folder = DataFolder::new(rs.path(), rs.path().join("backup"));
    let mut cache = folder.read::<string_core::library::Library>().unwrap();
    let cold = cache.value.scan(&[fixtures], &[]);
    assert_eq!((cold.read, cold.issues.len()), (8, 0));
    cache.save().unwrap();
    let loaded: Value = serde_json::from_slice(&reference("scan", rs.path())).unwrap();
    assert_eq!(loaded["reads"], 0);
    assert_eq!(initial["tracks"], loaded["tracks"]);
}

#[test]
fn all_three_id3_lyric_frames_match_javascript() {
    let audio = std::fs::read(media().join("Loose/098. no tags at all.mp3")).unwrap();
    let tmp = tempdir().unwrap();
    for (frame, desc) in [("USLT", "eng"), ("TXXX", "LYRICS"), ("TXXX", "USLT")] {
        let mut content = vec![0];
        content.extend(desc.as_bytes());
        content.push(0);
        content.extend(b" First line\r\nSecond line ");
        let mut body = frame.as_bytes().to_vec();
        body.extend((content.len() as u32).to_be_bytes());
        body.extend([0, 0]);
        body.extend(content);
        assert!(body.len() < 128);
        let mut bytes = b"ID3\x03\0\0\0\0\0".to_vec();
        bytes.push(body.len() as u8);
        bytes.extend(body);
        bytes.extend_from_slice(&audio);
        let file = tmp.path().join(format!("{frame}-{desc}.mp3"));
        std::fs::write(&file, bytes).unwrap();
        let expected: Value = serde_json::from_slice(&reference("song", &file)).unwrap();
        let actual = tags::read(&file).unwrap().lyrics;
        assert_eq!(actual.as_deref(), Some("First line\nSecond line"));
        assert_eq!(json!(actual), expected["lyrics"]);
    }
}

#[test]
fn mpeg_sync_after_leading_padding_matches_javascript() {
    let audio = std::fs::read(media().join("Loose/098. no tags at all.mp3")).unwrap();
    let tmp = tempdir().unwrap();
    let path = tmp.path().join("padded.mp3");
    let mut bytes = vec![0; 12288];
    bytes.extend_from_slice(&audio);
    std::fs::write(&path, bytes).unwrap();
    let expected: Value = serde_json::from_slice(&reference("song", &path)).unwrap();
    let actual = tags::read(&path).unwrap();
    assert_eq!(
        actual.tags.duration,
        expected["tags"]["duration"].as_f64().unwrap()
    );
}
