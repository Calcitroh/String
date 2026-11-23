//! What Discord is told, from what plays (presence\Program.cs's Build): "Listening to <song>" (activity type 2), the
//! artist, the time bar, what the member list shows (`status_display_type`), and the cover when one is known. Nothing
//! (the status cleared) while paused, without a title, or when a song's time bar ran out long ago.

use super::{Display, NowPlaying, Sent};
use serde_json::{Map, Value, json};

/// The activity to send (`None` clears the status) and what to say about it. `cover(artist, title)` gives a cover's
/// address, or "" (none known yet); it's asked only when covers are on.
pub(crate) fn build(
    now: Option<&NowPlaying>,
    now_ms: i64,
    cover: impl FnOnce(&str, &str) -> String,
) -> (Option<Value>, Sent) {
    let Some(d) = now.filter(|d| d.playing) else {
        return (None, Sent::Clear);
    };
    // left over from a song that ended without anyone saying so
    if d.ends_at > 0 && d.ends_at < now_ms - 3000 {
        return (None, Sent::Clear);
    }
    let title = fit(&d.title);
    if title.is_empty() {
        return (None, Sent::Clear);
    }
    let artist = fit(&d.artist);
    let album = fit(&d.album);
    let mut a = Map::new();
    a.insert("type".into(), json!(2)); // Listening
    a.insert("status_display_type".into(), json!(d.display.code()));
    a.insert("details".into(), json!(title));
    if !artist.is_empty() {
        a.insert("state".into(), json!(artist));
    }
    if d.started_at > 0 && d.ends_at > d.started_at {
        a.insert(
            "timestamps".into(),
            json!({ "start": d.started_at, "end": d.ends_at }),
        );
    }
    let picture = if d.covers && !artist.is_empty() {
        cover(&d.artist, &d.title)
    } else {
        String::new()
    };
    if !picture.is_empty() {
        let text = if album.is_empty() { &title } else { &album };
        a.insert(
            "assets".into(),
            json!({ "large_image": picture, "large_text": text }),
        );
    }
    (Some(Value::Object(a)), Sent::Playing(title))
}

impl Display {
    pub(crate) fn code(self) -> u8 {
        match self {
            Display::App => 0,
            Display::Artist => 1,
            Display::Song => 2,
        }
    }
}

/// Discord wants 2 to 128 characters (UTF-16 units, as JavaScript counts): one is padded with a blank braille cell,
/// more than 128 are cut to 127 and "…". Never cuts a character in half.
pub(crate) fn fit(s: &str) -> String {
    let s = s.trim();
    let units = s.encode_utf16().count();
    if units == 0 {
        return String::new();
    }
    if units < 2 {
        return format!("{s}\u{2800}");
    }
    if units <= 128 {
        return s.to_string();
    }
    let mut out = String::new();
    let mut n = 0;
    for c in s.chars() {
        if n + c.len_utf16() > 127 {
            break;
        }
        n += c.len_utf16();
        out.push(c);
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000_000;

    fn song() -> NowPlaying {
        NowPlaying {
            playing: true,
            title: "Glass Orbit".into(),
            artist: "Test Signal".into(),
            album: "Synthetic Weather".into(),
            started_at: NOW - 10_000,
            ends_at: NOW + 170_000,
            covers: false,
            display: Display::Song,
        }
    }

    fn no_cover(_: &str, _: &str) -> String {
        panic!("covers are off")
    }

    #[test]
    fn playing() {
        let (a, s) = build(Some(&song()), NOW, no_cover);
        let a = a.unwrap();
        assert_eq!(s, Sent::Playing("Glass Orbit".into()));
        assert_eq!(a["type"], 2);
        assert_eq!(a["status_display_type"], 2);
        assert_eq!(a["details"], "Glass Orbit");
        assert_eq!(a["state"], "Test Signal");
        assert_eq!(
            a["timestamps"],
            json!({ "start": NOW - 10_000, "end": NOW + 170_000 })
        );
        assert!(a.get("assets").is_none());
    }

    #[test]
    fn cleared() {
        assert_eq!(build(None, NOW, no_cover), (None, Sent::Clear));
        assert_eq!(
            build(
                Some(&NowPlaying {
                    playing: false,
                    ..song()
                }),
                NOW,
                no_cover
            ),
            (None, Sent::Clear)
        );
        assert_eq!(
            build(
                Some(&NowPlaying {
                    title: "  ".into(),
                    ..song()
                }),
                NOW,
                no_cover
            ),
            (None, Sent::Clear)
        );
        // a stale "playing": over more than 3 s ago
        let stale = NowPlaying {
            started_at: NOW - 300_000,
            ends_at: NOW - 3001,
            ..song()
        };
        assert_eq!(build(Some(&stale), NOW, no_cover), (None, Sent::Clear));
        let just_over = NowPlaying {
            started_at: NOW - 300_000,
            ends_at: NOW - 2000,
            ..song()
        };
        assert!(build(Some(&just_over), NOW, no_cover).0.is_some());
    }

    #[test]
    fn display_and_parts() {
        for (d, code) in [(Display::App, 0), (Display::Artist, 1), (Display::Song, 2)] {
            let (a, _) = build(
                Some(&NowPlaying {
                    display: d,
                    ..song()
                }),
                NOW,
                no_cover,
            );
            assert_eq!(a.unwrap()["status_display_type"], code);
        }
        let (a, _) = build(
            Some(&NowPlaying {
                artist: "".into(),
                covers: true,
                ..song()
            }),
            NOW,
            no_cover,
        );
        let a = a.unwrap();
        assert!(
            a.get("state").is_none() && a.get("assets").is_none(),
            "no artist: no state, no cover lookup"
        );
        let (a, _) = build(
            Some(&NowPlaying {
                started_at: 0,
                ..song()
            }),
            NOW,
            no_cover,
        );
        assert!(a.unwrap().get("timestamps").is_none());
    }

    #[test]
    fn covers() {
        let on = NowPlaying {
            covers: true,
            ..song()
        };
        let (a, _) = build(Some(&on), NOW, |artist, title| {
            assert_eq!((artist, title), ("Test Signal", "Glass Orbit"));
            "https://example.test/a.jpg".into()
        });
        assert_eq!(
            a.unwrap()["assets"],
            json!({ "large_image": "https://example.test/a.jpg", "large_text": "Synthetic Weather" })
        );
        let (a, _) = build(
            Some(&NowPlaying {
                album: "".into(),
                ..on.clone()
            }),
            NOW,
            |_, _| "https://x.test/b.jpg".into(),
        );
        assert_eq!(a.unwrap()["assets"]["large_text"], "Glass Orbit");
        let (a, _) = build(Some(&on), NOW, |_, _| String::new());
        assert!(a.unwrap().get("assets").is_none(), "not known yet");
    }

    #[test]
    fn fitting() {
        assert_eq!(fit("  Glass Orbit "), "Glass Orbit");
        assert_eq!(fit("X"), "X\u{2800}");
        assert_eq!(fit("X").encode_utf16().count(), 2);
        assert_eq!(fit(""), "");
        let long = "a".repeat(200);
        assert_eq!(fit(&long), format!("{}…", "a".repeat(127)));
        assert_eq!(fit(&"b".repeat(128)), "b".repeat(128));
        // an emoji (two UTF-16 units) across the cut stays whole: dropped, not halved
        let edge = format!("{}😀tail{}", "c".repeat(126), "d".repeat(10));
        let f = fit(&edge);
        assert_eq!(f, format!("{}…", "c".repeat(126)));
        assert!(f.encode_utf16().count() <= 128);
    }
}
