mod common;
use common::{Fake, script, song};
use serde_json::json;
use std::{
    fs, os::windows::process::CommandExt, path::Path, process::Command, sync::mpsc, thread,
    time::Duration,
};
use string_lyrics::{Lyrics, Ollama, Source, TranslationOptions};
use tempfile::tempdir;
const OPT: TranslationOptions<'static> = TranslationOptions {
    lang: "English",
    model: "fake-translator:1b",
};

#[test]
fn streams_complete_rows_verifies_locality_and_leaves_external_server_running() {
    let tmp = tempdir().unwrap();
    let fake = Fake::new("ollama", "normal");
    let api = Ollama::with_base_url(tmp.path(), &fake.base).unwrap();
    assert_eq!(api.models().unwrap(), ["fake-translator:1b"]);
    let path = tmp.path().join("song.mp3");
    let t = song(&path);
    for name in ["cloud-alias", "unverified-model", "qwen3:14b-cloud"] {
        let r = api.translate(
            &t,
            &["a line"],
            TranslationOptions { model: name, ..OPT },
            |_, _| panic!("refused model"),
        );
        assert!(r.error.unwrap().contains("installed local model"));
    }
    assert!(
        fake.read("/_requests")
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["path"] != "/api/chat")
    );
    let mut updates = Vec::new();
    let r = api.translate(&t, &["first", "♪", "first", "last"], OPT, |i, s| {
        updates.push((i, s.unwrap().to_owned()))
    });
    assert!(r.error.is_none() && !r.cached && !r.same);
    assert_eq!(
        json!(updates),
        json!([[0, "[EN] first"], [2, "[EN] first"], [3, "[EN] last"]])
    );
    assert_eq!(
        json!(r.lines),
        json!(["[EN] first", null, "[EN] first", "[EN] last"])
    );
    api.shutdown();
    assert_eq!(fake.read("/api/version")["version"], "0.0.0-fake");
}
#[test]
fn whole_song_request_partial_threshold_and_same_language() {
    for (scenario, filled) in [("two", 2), ("four", 4)] {
        let tmp = tempdir().unwrap();
        let fake = Fake::new("ollama", scenario);
        let api = Ollama::with_base_url(tmp.path(), &fake.base).unwrap();
        let path = tmp.path().join("song.mp3");
        let t = song(&path);
        let model = if filled == 4 { "qwen3:14b" } else { "gemma3" };
        let options = TranslationOptions { model, ..OPT };
        let lines = ["one", "Two", "one", "Three", "Four", "Five", "Oh, oh!"];
        let r = api.translate(&t, &lines, options, |_, _| {});
        assert_eq!(r.error.as_deref(), Some("fixture ended"));
        assert!(r.same);
        assert_eq!(r.lines.iter().flatten().count(), filled + 1);
        let requests = fake.read("/_requests");
        let chats: Vec<_> = requests
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["path"] == "/api/chat")
            .collect();
        assert_eq!(chats.len(), 1);
        let body = &chats[0]["body"];
        assert_eq!(body.get("think"), (filled == 4).then_some(&json!(false)));
        assert_eq!(body["stream"], true);
        assert_eq!(body["keep_alive"], "3m");
        assert_eq!(body["options"], json!({"temperature":0.2,"num_ctx":8192}));
        assert_eq!(
            body["messages"][1]["content"],
            "Song: \"Glass Orbit\" by Test Signal\n\n1| one\n2| Two\n3| Three\n4| Four\n5| Five"
        );
        assert_eq!(
            api.translate(&t, &lines, options, |_, _| {}).cached,
            filled == 4
        );
    }
}
#[test]
fn cancel_interrupts_stalled_stream_and_replacement_discards_partial_output() {
    let tmp = tempdir().unwrap();
    let fake = Fake::new("ollama", "stall");
    let api = Ollama::with_base_url(tmp.path(), &fake.base).unwrap();
    let path = tmp.path().join("song.mp3");
    let t = song(&path);
    for replace in [false, true] {
        let (updated, updates) = mpsc::channel();
        let (done, result) = mpsc::channel();
        thread::scope(|s| {
            s.spawn(|| {
                let r = api.translate(&t, &["one", "two"], OPT, |_, line| {
                    updated.send(line.unwrap().to_owned()).unwrap()
                });
                done.send(r).unwrap();
            });
            assert_eq!(
                updates.recv_timeout(Duration::from_secs(5)).unwrap(),
                "First"
            );
            if replace {
                let next = api.translate(&t, &["♪"], OPT, |_, _| {});
                assert!(next.same && !next.stopped);
            } else {
                api.stop();
            }
            assert!(result.recv_timeout(Duration::from_secs(2)).unwrap().stopped);
        });
    }
    assert!(!tmp.path().join("translations").exists());
}
#[test]
fn alternate_origin_is_loopback_only() {
    let tmp = tempdir().unwrap();
    for bad in [
        "http://example.invalid",
        "http://localhost@example.invalid",
        "http://127.0.0.1/path",
        "http://127.0.0.1?q=x",
        "https://localhost",
        "http://localhost:8080@127.0.0.1",
    ] {
        assert!(Ollama::with_base_url(tmp.path(), bad).is_err(), "{bad}");
    }
    for good in [
        "http://localhost:11111/",
        "http://[::1]:11111",
        "http://127.0.0.1:11111",
    ] {
        assert!(Ollama::with_base_url(tmp.path(), good).is_ok(), "{good}");
    }
}
fn js_cache(mode: &str, dir: &Path) {
    let r = Command::new("node")
        .creation_flags(0x08000000)
        .arg(script("cache.cjs"))
        .arg(mode)
        .arg(dir)
        .output()
        .unwrap();
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
}
#[test]
fn javascript_and_rust_read_each_others_actual_cache_files() {
    let fake = Fake::new("ollama", "normal");
    let lrclib = Fake::new("lrclib", "normal");
    for js_first in [true, false] {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("song.mp3");
        let t = song(&path);
        if js_first {
            js_cache("write", tmp.path());
        } else {
            fs::write(path.with_extension("lrc"), "[00:02]音 🎵\n[00:03]空").unwrap();
        }
        let r = Lyrics::with_base_url(tmp.path(), &lrclib.base).get(&t, false, false);
        assert_eq!(r.source, Source::Lrc);
        assert_eq!(r.lines[0].text, "音 🎵");
        let api = Ollama::with_base_url(tmp.path(), &fake.base).unwrap();
        let tr = api.translate(&t, &["音 🎵", "♪", "音 🎵", "空"], OPT, |_, _| {});
        assert_eq!(tr.cached, js_first);
        assert_eq!(
            json!(tr.lines),
            json!(["[EN] 音 🎵", null, "[EN] 音 🎵", "[EN] 空"])
        );
        if !js_first {
            fs::remove_file(path.with_extension("lrc")).unwrap();
            js_cache("read", tmp.path());
        }
    }
}
