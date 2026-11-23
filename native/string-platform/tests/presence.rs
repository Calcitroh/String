//! The Discord presence against a fake Discord (a named pipe of the test's own, never the real `discord-ipc-N`, so a
//! real Discord status is never touched) and fake Apple and Deezer searches on 127.0.0.1 (never the real ones): what
//! tests\test-presence.js checked of String 1.x's helper exe.

use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};
use string_platform::presence::{
    BadAppId, Display, Event, NowPlaying, Options, Presence, Sent, unix_ms,
};
use windows::Win32::Foundation::{CloseHandle, ERROR_NO_DATA, ERROR_PIPE_CONNECTED, HANDLE};
use windows::Win32::Storage::FileSystem::{PIPE_ACCESS_DUPLEX, ReadFile, WriteFile};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_NOWAIT, PIPE_READMODE_BYTE,
    PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES,
};
use windows::core::HSTRING;

const ID: &str = "123456789012345678";

/* ---------- a fake Discord ---------- */

#[derive(Default)]
struct Seen {
    sets: Vec<(Instant, Value)>,
    handshakes: usize,
    hello: Option<Value>,
    pongs: Vec<Value>,
    connected: bool,
}

enum Cmd {
    Ping(Value),
    Kick,
    Stop,
}

struct FakeDiscord {
    name: String,
    seen: Arc<Mutex<Seen>>,
    cmd: mpsc::Sender<Cmd>,
    thread: Option<thread::JoinHandle<()>>,
}

impl FakeDiscord {
    fn start(refuse: bool) -> FakeDiscord {
        static N: AtomicUsize = AtomicUsize::new(0);
        let name = format!(
            "string-platform-fake-discord-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        );
        assert!(!name.starts_with("discord-ipc"));
        let seen = Arc::new(Mutex::new(Seen::default()));
        let (cmd, rx) = mpsc::channel();
        let path = format!(r"\\.\pipe\{name}");
        let state = seen.clone();
        let thread = thread::spawn(move || serve(&path, refuse, &state, &rx));
        FakeDiscord {
            name,
            seen,
            cmd,
            thread: Some(thread),
        }
    }

    fn sets(&self) -> Vec<(Instant, Value)> {
        self.seen.lock().unwrap().sets.clone()
    }

    /// The last SET_ACTIVITY's activity: `Some(Null)` when it cleared the status.
    fn last(&self) -> Option<Value> {
        self.seen
            .lock()
            .unwrap()
            .sets
            .last()
            .map(|(_, f)| f["args"]["activity"].clone())
    }

    fn last_pid(&self) -> Option<u64> {
        self.seen
            .lock()
            .unwrap()
            .sets
            .last()
            .and_then(|(_, f)| f["args"]["pid"].as_u64())
    }
}

impl Drop for FakeDiscord {
    fn drop(&mut self) {
        let _ = self.cmd.send(Cmd::Stop);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn serve(path: &str, refuse: bool, seen: &Mutex<Seen>, cmds: &mpsc::Receiver<Cmd>) {
    // PIPE_NOWAIT: every call returns at once, so one thread can poll for connections, frames and commands
    let h = unsafe {
        CreateNamedPipeW(
            &HSTRING::from(path),
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_NOWAIT,
            PIPE_UNLIMITED_INSTANCES,
            65536,
            65536,
            0,
            None,
        )
    };
    assert!(!h.is_invalid(), "fake Discord pipe");
    let mut connected = false;
    let mut buf: Vec<u8> = Vec::new();
    let drop_client = |connected: &mut bool, buf: &mut Vec<u8>| {
        unsafe {
            let _ = DisconnectNamedPipe(h);
        }
        *connected = false;
        buf.clear();
        seen.lock().unwrap().connected = false;
    };
    loop {
        while let Ok(c) = cmds.try_recv() {
            match c {
                Cmd::Stop => {
                    unsafe {
                        let _ = CloseHandle(h);
                    }
                    return;
                }
                Cmd::Kick if connected => drop_client(&mut connected, &mut buf),
                Cmd::Ping(v) if connected => send(h, 3, &v),
                _ => {}
            }
        }
        if !connected {
            match unsafe { ConnectNamedPipe(h, None) } {
                Ok(()) => connected = true,
                Err(e) if e.code() == ERROR_PIPE_CONNECTED.to_hresult() => connected = true,
                Err(e) if e.code() == ERROR_NO_DATA.to_hresult() => {
                    drop_client(&mut connected, &mut buf)
                }
                Err(_) => {} // still listening
            }
            if connected {
                seen.lock().unwrap().connected = true;
            }
        } else {
            let mut chunk = [0u8; 65536];
            let mut n = 0u32;
            match unsafe { ReadFile(h, Some(&mut chunk), Some(&mut n), None) } {
                Ok(()) => buf.extend_from_slice(&chunk[..n as usize]),
                Err(e) if e.code() == ERROR_NO_DATA.to_hresult() => {}
                Err(_) => drop_client(&mut connected, &mut buf), // the client went away
            }
            while buf.len() >= 8 {
                let op = u32::from_le_bytes(buf[0..4].try_into().unwrap());
                let len = u32::from_le_bytes(buf[4..8].try_into().unwrap()) as usize;
                if buf.len() < 8 + len {
                    break;
                }
                let j: Value = serde_json::from_slice(&buf[8..8 + len]).unwrap();
                buf.drain(..8 + len);
                match op {
                    0 => {
                        {
                            let mut s = seen.lock().unwrap();
                            s.handshakes += 1;
                            s.hello = Some(j);
                        }
                        if refuse {
                            send(
                                h,
                                2,
                                &json!({ "code": 4000, "message": "Invalid Client ID" }),
                            );
                        } else {
                            send(
                                h,
                                1,
                                &json!({ "cmd": "DISPATCH", "evt": "READY", "data": { "v": 1,
                                "user": { "id": "1", "username": "tester", "global_name": "Tester" } } }),
                            );
                        }
                    }
                    1 => {
                        send(
                            h,
                            1,
                            &json!({ "cmd": j["cmd"], "nonce": j["nonce"], "data": {} }),
                        );
                        if j["cmd"] == "SET_ACTIVITY" {
                            seen.lock().unwrap().sets.push((Instant::now(), j));
                        }
                    }
                    4 => seen.lock().unwrap().pongs.push(j),
                    _ => {}
                }
            }
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn send(h: HANDLE, op: u32, v: &Value) {
    let body = v.to_string();
    let mut frame = op.to_le_bytes().to_vec();
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.extend_from_slice(body.as_bytes());
    let mut written = 0u32;
    unsafe {
        let _ = WriteFile(h, Some(&frame), Some(&mut written), None);
    }
}

/* ---------- fake cover searches: Apple knows one song, Deezer another ---------- */

struct FakeSearch {
    base: String,
    asks: Arc<AtomicUsize>,
    deezer_asks: Arc<AtomicUsize>,
}

impl FakeSearch {
    fn start() -> FakeSearch {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (asks, deezer_asks) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let (a, d) = (asks.clone(), deezer_asks.clone());
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                answer(stream, &a, &d);
            }
        });
        FakeSearch {
            base,
            asks,
            deezer_asks,
        }
    }
    fn itunes(&self) -> String {
        format!("{}/search", self.base)
    }
    fn deezer(&self) -> String {
        format!("{}/deezer", self.base)
    }
}

fn answer(mut stream: TcpStream, asks: &AtomicUsize, deezer_asks: &AtomicUsize) {
    let mut request = Vec::new();
    let mut byte = [0u8; 1];
    while !request.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap_or(0) == 1 {
        request.push(byte[0]);
    }
    let request = String::from_utf8_lossy(&request);
    let target = request.split(' ').nth(1).unwrap_or("/");
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let param = |name: &str| {
        query
            .split('&')
            .find_map(|kv| kv.strip_prefix(name).and_then(|v| v.strip_prefix('=')))
            .map(decode)
            .unwrap_or_default()
    };
    asks.fetch_add(1, Ordering::SeqCst);
    let body = if path == "/deezer" {
        deezer_asks.fetch_add(1, Ordering::SeqCst);
        let data = if param("q").to_lowercase().contains("deezer only") {
            json!([
                { "title": "Deezer Only Song", "artist": { "name": "Someone Else" }, "album": { "cover_xl": "https://example.test/dz/wrong.jpg" } },
                { "title": "Deezer Only Song (Live)", "artist": { "name": "Test Signal" }, "album": { "cover_xl": "https://example.test/dz/1000x1000.jpg" } }
            ])
        } else {
            json!([])
        };
        json!({ "data": data, "total": data.as_array().unwrap().len() })
    } else {
        let results = if param("term").to_lowercase().contains("glass orbit") {
            json!([
                { "artistName": "Someone Else", "trackName": "Glass Orbit", "artworkUrl100": "https://example.test/wrong/100x100bb.jpg" },
                { "artistName": "Test Signal", "trackName": "Glass Orbit (Remastered)", "artworkUrl100": "https://example.test/art/100x100bb.jpg" }
            ])
        } else {
            json!([])
        };
        json!({ "resultCount": results.as_array().unwrap().len(), "results": results })
    }
    .to_string();
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let hex = b
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        if let (b'%', Some(x)) = (b[i], hex) {
            out.push(x);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/* ---------- helpers ---------- */

fn wait_for<T>(ms: u64, mut f: impl FnMut() -> Option<T>) -> Option<T> {
    let end = Instant::now() + Duration::from_millis(ms);
    loop {
        if let Some(v) = f() {
            return Some(v);
        }
        if Instant::now() >= end {
            return None;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn playing() -> NowPlaying {
    let now = unix_ms();
    NowPlaying {
        playing: true,
        title: "Glass Orbit".into(),
        artist: "Test Signal".into(),
        album: "Synthetic Weather".into(),
        started_at: now - 10_000,
        ends_at: now + 170_000,
        covers: false,
        display: Display::Song,
    }
}

fn options(dir: &Path, pipe: &str, search: Option<&FakeSearch>) -> Options {
    let mut o = Options::new(ID, dir);
    o.pipe = Some(pipe.to_string());
    // never the real searches: a fake, or nowhere at all
    o.itunes = search.map_or("http://127.0.0.1:9/none".into(), FakeSearch::itunes);
    o.deezer = search.map_or("http://127.0.0.1:9/none".into(), FakeSearch::deezer);
    o.min_interval = Duration::from_millis(300);
    o.retry_after = Duration::from_millis(200);
    o.reconnect_after = Duration::from_millis(200);
    o
}

type Events = Arc<Mutex<Vec<Event>>>;

fn recorder() -> (Events, impl FnMut(Event) + Send + 'static) {
    let events: Events = Arc::default();
    let e = events.clone();
    (events, move |ev| e.lock().unwrap().push(ev))
}

fn has(events: &Events, e: &Event) -> bool {
    events.lock().unwrap().contains(e)
}

fn cache(dir: &Path) -> Value {
    std::fs::read_to_string(dir.join("discord-covers.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(Value::Null)
}

fn activity_where(d: &FakeDiscord, f: impl Fn(&Value) -> bool) -> Option<Value> {
    d.last().filter(|a| !a.is_null() && f(a))
}

/* ---------- the tests ---------- */

#[test]
fn a_bad_application_id_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    for id in ["not-a-number", "12345", "1234567890123456789012345", ""] {
        let o = Options {
            app_id: id.into(),
            ..options(dir.path(), "string-platform-no-such-pipe", None)
        };
        assert_eq!(Presence::start(o, |_| {}).err(), Some(BadAppId), "{id}");
    }
}

#[test]
fn the_normal_life_of_it() {
    let dir = tempfile::tempdir().unwrap();
    let search = FakeSearch::start();
    let d = FakeDiscord::start(false);
    let (events, on_event) = recorder();
    let p = Presence::start(options(dir.path(), &d.name, Some(&search)), on_event).unwrap();
    p.set(Some(NowPlaying {
        playing: false,
        ..playing()
    }));

    assert!(
        wait_for(6000, || has(&events, &Event::Connected("Tester".into()))
            .then_some(()))
        .is_some(),
        "connects and says who to"
    );
    let hello = d.seen.lock().unwrap().hello.clone().unwrap();
    assert_eq!(
        (hello["v"].as_u64(), hello["client_id"].as_str()),
        (Some(1), Some(ID)),
        "handshake carries the application ID"
    );
    assert!(
        wait_for(3000, || d.last().filter(Value::is_null)).is_some(),
        "paused: the status is cleared"
    );
    assert_eq!(d.last_pid(), Some(std::process::id() as u64), "pid is sent");
    assert!(has(&events, &Event::Sent(Sent::Clear)));

    let doc = playing();
    p.set(Some(doc.clone()));
    let a1 = wait_for(3000, || activity_where(&d, |_| true)).expect("playing is sent");
    assert_eq!(
        (a1["type"].as_u64(), a1["status_display_type"].as_u64()),
        (Some(2), Some(2)),
        "Listening, the song"
    );
    assert_eq!(
        (a1["details"].as_str(), a1["state"].as_str()),
        (Some("Glass Orbit"), Some("Test Signal"))
    );
    assert_eq!(
        a1["timestamps"],
        json!({ "start": doc.started_at, "end": doc.ends_at }),
        "a time bar from start to end"
    );
    assert!(
        a1.get("assets").is_none() && search.asks.load(Ordering::SeqCst) == 0,
        "no cover without the setting"
    );
    assert!(
        wait_for(2000, || has(
            &events,
            &Event::Sent(Sent::Playing("Glass Orbit".into()))
        )
        .then_some(()))
        .is_some()
    );

    p.set(Some(NowPlaying {
        covers: true,
        ..playing()
    }));
    let a2 = wait_for(8000, || {
        activity_where(&d, |a| a["assets"]["large_image"].is_string())
    })
    .expect("a cover");
    assert_eq!(
        a2["assets"],
        json!({ "large_image": "https://example.test/art/600x600bb.jpg", "large_text": "Synthetic Weather" }),
        "the right one, 600 px"
    );
    let key = "v2\ntest signal\nglass orbit";
    assert!(
        wait_for(2000, || (cache(dir.path())[key]
            == "https://example.test/art/600x600bb.jpg")
            .then_some(()))
        .is_some(),
        "remembered on disk"
    );
    assert_eq!(
        search.deezer_asks.load(Ordering::SeqCst),
        0,
        "Apple had it: Deezer not asked"
    );

    p.set(Some(NowPlaying {
        covers: true,
        title: "Deezer Only Song".into(),
        ..playing()
    }));
    let a2b = wait_for(8000, || {
        activity_where(&d, |a| {
            a["details"] == "Deezer Only Song" && a["assets"]["large_image"].is_string()
        })
    })
    .expect("a cover from Deezer");
    assert_eq!(
        a2b["assets"]["large_image"], "https://example.test/dz/1000x1000.jpg",
        "the right one"
    );

    p.set(Some(NowPlaying {
        covers: true,
        title: "Unknown Song".into(),
        artist: "Nobody".into(),
        ..playing()
    }));
    assert!(
        wait_for(3000, || activity_where(&d, |a| a["details"]
            == "Unknown Song"))
        .is_some()
    );
    assert!(
        wait_for(8000, || (cache(dir.path())["v2\nnobody\nunknown song"]
            == "")
            .then_some(()))
        .is_some(),
        "remembered as having none (both asked)"
    );
    thread::sleep(Duration::from_millis(500));
    let a3 = d.last().unwrap();
    assert!(
        a3["details"] == "Unknown Song" && a3.get("assets").is_none(),
        "shows without a cover: {a3}"
    );

    // an artist of symbols only has nothing left once punctuation goes: it must not take whoever Apple lists first
    p.set(Some(NowPlaying {
        covers: true,
        artist: "!!!".into(),
        ..playing()
    }));
    assert!(wait_for(3000, || activity_where(&d, |a| a["state"] == "!!!")).is_some());
    assert!(
        wait_for(8000, || (cache(dir.path())["v2\n!!!\nglass orbit"] == "")
            .then_some(()))
        .is_some()
    );
    thread::sleep(Duration::from_millis(500));
    let a5 = d.last().unwrap();
    assert!(
        a5["state"] == "!!!" && a5.get("assets").is_none(),
        "no other artist's cover: {a5}"
    );

    // skipping through songs: never faster than the interval, and the newest wins
    thread::sleep(Duration::from_millis(400));
    let before = d.sets().len();
    for i in 1..=6 {
        p.set(Some(NowPlaying {
            title: format!("Skip {i}"),
            ..playing()
        }));
        thread::sleep(Duration::from_millis(50));
    }
    thread::sleep(Duration::from_millis(1000));
    let all = d.sets();
    let burst = &all[before - 1..];
    assert!(
        burst
            .windows(2)
            .all(|w| w[1].0 - w[0].0 >= Duration::from_millis(270)),
        "rate limit"
    );
    assert!(
        burst.len() - 1 <= 3,
        "{} updates for 6 skips",
        burst.len() - 1
    );
    assert_eq!(
        d.last().unwrap()["details"],
        "Skip 6",
        "the newest song wins"
    );

    p.set(Some(NowPlaying {
        title: "X".into(),
        display: Display::Artist,
        ..playing()
    }));
    let a4 = wait_for(3000, || {
        activity_where(&d, |a| {
            a["details"].as_str().is_some_and(|t| t.starts_with('X'))
        })
    })
    .unwrap();
    assert_eq!(
        a4["details"].as_str().unwrap().encode_utf16().count(),
        2,
        "one-letter title padded to Discord's minimum"
    );
    assert_eq!(
        a4["status_display_type"], 1,
        "the member list can show the artist instead"
    );

    let now = unix_ms();
    p.set(Some(NowPlaying {
        started_at: now - 300_000,
        ends_at: now - 60_000,
        ..playing()
    }));
    assert!(
        wait_for(3000, || d.last().filter(Value::is_null)).is_some(),
        "a stale \"playing\" (song long over) clears"
    );

    d.cmd.send(Cmd::Ping(json!({ "ping": 42 }))).unwrap();
    assert!(
        wait_for(3000, || d
            .seen
            .lock()
            .unwrap()
            .pongs
            .iter()
            .any(|x| x["ping"] == 42)
            .then_some(()))
        .is_some(),
        "answers a ping with a pong"
    );

    // Discord restarts: it notices, reconnects and sends the current song again
    p.set(Some(NowPlaying {
        title: "After Restart".into(),
        ..playing()
    }));
    assert!(
        wait_for(3000, || activity_where(&d, |a| a["details"]
            == "After Restart"))
        .is_some()
    );
    let connected_at = events.lock().unwrap().len();
    d.cmd.send(Cmd::Kick).unwrap();
    assert!(
        wait_for(3000, || events.lock().unwrap()[connected_at..]
            .contains(&Event::Waiting)
            .then_some(()))
        .is_some(),
        "notices Discord going away"
    );
    let n0 = d.sets().len();
    assert!(
        wait_for(5000, || (d.seen.lock().unwrap().handshakes >= 2)
            .then_some(()))
        .is_some(),
        "reconnects by itself"
    );
    assert!(
        wait_for(3000, || (d.sets().len() > n0
            && d.last().unwrap()["details"] == "After Restart")
            .then_some(()))
        .is_some(),
        "and sends the current song again"
    );

    // String closing: the pipe closes with it, so no status outlives String
    assert!(d.seen.lock().unwrap().connected);
    drop(p);
    assert!(
        wait_for(3000, || (!d.seen.lock().unwrap().connected).then_some(())).is_some(),
        "closes the pipe when stopped"
    );
}

#[test]
fn at_most_one_update_per_4_2_s() {
    let dir = tempfile::tempdir().unwrap();
    let d = FakeDiscord::start(false);
    let mut o = options(dir.path(), &d.name, None);
    o.min_interval = Options::new(ID, dir.path()).min_interval;
    let p = Presence::start(o, |_| {}).unwrap();
    p.set(Some(NowPlaying {
        title: "Start".into(),
        ..playing()
    }));
    assert!(wait_for(6000, || activity_where(&d, |a| a["details"] == "Start")).is_some());
    let t0 = Instant::now();
    for i in 1..=6 {
        p.set(Some(NowPlaying {
            title: format!("Skip {i}"),
            ..playing()
        }));
        thread::sleep(Duration::from_millis(250));
    }
    thread::sleep(Duration::from_millis(6500) - t0.elapsed().min(Duration::from_millis(1500)));
    let sets = d.sets();
    assert!(
        sets.windows(2)
            .all(|w| w[1].0 - w[0].0 >= Duration::from_millis(4000)),
        "never faster than Discord allows"
    );
    assert!(sets.len() <= 3, "{} updates", sets.len());
    assert_eq!(
        d.last().unwrap()["details"],
        "Skip 6",
        "the newest song wins"
    );
}

#[test]
fn a_refused_application_id_stops_it() {
    let dir = tempfile::tempdir().unwrap();
    let d = FakeDiscord::start(true);
    let (events, on_event) = recorder();
    let p = Presence::start(options(dir.path(), &d.name, None), on_event).unwrap();
    let refused = wait_for(6000, || {
        events.lock().unwrap().iter().find_map(|e| match e {
            Event::Refused(t) => Some(t.clone()),
            _ => None,
        })
    });
    assert!(
        refused.is_some_and(|t| t.contains("application ID")),
        "says so"
    );
    assert!(
        wait_for(2000, || (!p.is_running()).then_some(())).is_some(),
        "and stops"
    );
    thread::sleep(Duration::from_millis(500));
    assert_eq!(d.seen.lock().unwrap().handshakes, 1, "never tries again");
}

#[test]
fn no_discord_waits() {
    let dir = tempfile::tempdir().unwrap();
    let (events, on_event) = recorder();
    let pipe = format!("string-platform-no-such-pipe-{}", std::process::id());
    let p = Presence::start(options(dir.path(), &pipe, None), on_event).unwrap();
    assert!(wait_for(3000, || has(&events, &Event::Waiting).then_some(())).is_some());
    assert!(p.is_running());
    thread::sleep(Duration::from_millis(600));
    assert_eq!(
        events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| **e == Event::Waiting)
            .count(),
        1,
        "said once, not every try"
    );
}
