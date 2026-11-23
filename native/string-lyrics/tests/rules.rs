use serde_json::json;
use std::time::{Duration, Instant};
use string_lyrics::*;

#[test]
fn synced_lrc() {
    let p = parse_lrc(
        "[ar: Test Signal]\n[ti: Glass Orbit]\n[length: 00:24]\n\n[00:00.00]\n[00:01.50]First line\n[00:04.20]<00:04.20>Second <00:04.80>line with word times\n[00:08.00][00:16.00]A chorus, sung twice\n[00:12.345]Third line, three decimals\n[00:20]No decimals\n",
    );
    assert!(p.synced);
    assert_eq!(
        p.lines.iter().map(|l| l.t.unwrap()).collect::<Vec<_>>(),
        [1.5, 4.2, 8., 12.345, 16., 20.]
    );
    assert_eq!(
        p.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(),
        [
            "First line",
            "Second line with word times",
            "A chorus, sung twice",
            "Third line, three decimals",
            "A chorus, sung twice",
            "No decimals"
        ]
    );
    for (t, want) in [(0.5, None), (4.2, Some(1)), (10., Some(2)), (300., Some(5))] {
        assert_eq!(line_at(&p.lines, t), want);
    }
}
#[test]
fn offsets_and_plain_text() {
    let p = parse_lrc("[offset:+500]\n[00:02.00]a\n[00:03.00]b");
    assert_eq!((p.lines[0].t, p.lines[1].t), (Some(1.5), Some(2.5)));
    let p = parse_lrc("[00:05.00]Only line");
    assert!(p.synced);
    assert_eq!(
        p.lines[0],
        Line {
            t: Some(5.),
            text: "Only line".into()
        }
    );
    let p = parse_lrc("[00:01.00]one\r\n[00:02.00]two\r\n");
    assert!(p.synced && p.lines.len() == 2 && p.lines[1].text == "two");
    for (s, want) in [
        ("[00:05.00]one\ntwo", "one|two"),
        (
            "Verse one\nline two\n\n\n\nChorus\n[ar: Someone]",
            "Verse one|line two||Chorus",
        ),
        ("", ""),
    ] {
        let p = parse_lrc(s);
        assert!(!p.synced && p.lines.iter().all(|l| l.t.is_none()));
        assert_eq!(
            p.lines
                .iter()
                .map(|l| l.text.as_str())
                .collect::<Vec<_>>()
                .join("|"),
            want
        );
    }
    assert!(parse_lrc("").lines.is_empty());
}
#[test]
fn lrclib_match_and_normalization() {
    let rows: Vec<Candidate> = serde_json::from_value(json!([
        {"trackName":"Glass Orbit","artistName":"Other","duration":240,"plainLyrics":"x"},
        {"trackName":"Glass Orbit (Remastered 2011)","artistName":"Test Signal","duration":181,"plainLyrics":"plain"},
        {"trackName":"Glass Orbit","artistName":"Test Signal","duration":182,"syncedLyrics":"synced"},
        {"trackName":"Glass Orbit","artistName":"Test Signal","duration":300,"syncedLyrics":"far"}
    ])).unwrap();
    let pick = |rows, title, artist, secs| pick_best(rows, title, artist, secs);
    assert!(std::ptr::eq(
        pick(&rows, "Glass Orbit", "Test Signal", 180.).unwrap(),
        &rows[2]
    ));
    assert!(pick(&rows[3..], "Glass Orbit", "Test Signal", 180.).is_none());
    assert!(pick(&rows, "Low Tide", "Test Signal", 180.).is_none());
    assert!(std::ptr::eq(
        pick(&rows, "Glass Orbit", "", 240.).unwrap(),
        &rows[0]
    ));
    assert!(pick(&rows[..1], "Glass Orbit", "Test Signal", 0.).is_none());
    assert_eq!(
        norm("Low Tide (feat. Somebody) - Remastered!"),
        "low tide remastered"
    );
    assert_eq!(norm("Café  Déjà-vu"), "cafe deja vu");
    assert_eq!(norm("ＡＢＣ 𝟠 𐐀"), "abc 8 𐐨");
}
#[test]
fn translation_plan_and_prompt() {
    let p = plan(&[
        "君の名前を",
        "♪",
        "",
        "La la la",
        "Oh, oh!",
        "君の名前を",
        "  空を見て  ",
        "Yeah yeah",
    ]);
    assert_eq!(p.uniq, ["君の名前を", "空を見て"]);
    assert_eq!(
        p.map,
        [Some(0), None, None, None, None, Some(0), Some(1), None]
    );
    let m = messages(&p.uniq, "English", "Song Title", "Someone");
    assert_eq!(m.as_array().unwrap().len(), 2);
    assert_eq!(m[0]["role"], "system");
    assert!(m[0]["content"].as_str().unwrap().contains("into English"));
    assert_eq!(
        m[1],
        json!({"role":"user","content":"Song: \"Song Title\" by Someone\n\n1| 君の名前を\n2| 空を見て"})
    );
}
#[test]
fn odd_numbering_quotes_and_thinking() {
    for (s, n, want) in [
        (
            "<think>\n</think>\n\n1| Your name\n2. Looking at the sky\n",
            2,
            json!(["Your name", "Looking at the sky"]),
        ),
        (
            "**1**) \"Your name\"\n2 - sky\n3| ignored",
            2,
            json!(["Your name", "sky"]),
        ),
        ("2| only two", 3, json!([null, "only two", null])),
        ("<think>1| no", 1, json!([null])),
        (
            "1| a\n<think>x</think>\n2| b\n<think>y",
            2,
            json!(["a", "b"]),
        ),
        (
            "1| pre<think>hidden\nline</think>fix\n2: “quoted”",
            2,
            json!(["prefix", "quoted"]),
        ),
        ("1| a\n1| \"\"\n2– last", 2, json!([null, "last"])),
    ] {
        assert_eq!(json!(parse_reply(s, n)), want);
    }
}
#[test]
fn pathological_model_reply_stays_linear() {
    let wall = " ".repeat(60_000);
    let quotes = "\"".repeat(40_000);
    let odd = format!(
        "1{wall}x\n2|{wall}word{wall}!\n3| {quotes}q{quotes}\n{}\n{}4| z",
        "<think>".repeat(20_000),
        "*".repeat(30_000)
    );
    let t = Instant::now();
    let got = parse_reply(&odd, 4);
    assert!(t.elapsed() < Duration::from_millis(300));
    assert_eq!(
        got,
        [None, Some(format!("word{wall}!")), Some("q".into()), None]
    );
    assert_eq!(plan(&[format!("{}x", "mmmm".repeat(10_000))]).uniq.len(), 1);
}
#[test]
fn installed_model_preferences() {
    assert!(thinks("qwen3:14b") && thinks("deepseek-r1:8b") && !thinks("llama3.1:8b"));
    assert_eq!(
        pick_model(&["llama3.1:8b", "qwen3:32b", "qwen3:14b", "qwen3:8b"]),
        Some("qwen3:14b")
    );
    assert_eq!(pick_model(&["llama3.1:8b", "qwen3:8b"]), Some("qwen3:8b"));
    assert_eq!(pick_model(&["phi3:mini"]), Some("phi3:mini"));
    assert_eq!(pick_model::<&str>(&[]), None);
    assert_eq!(
        pick_model(&["qwen3:14b-cloud", "foo@cloud", "bar:CLOUD"]),
        None
    );
}
