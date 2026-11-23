use serde_json::Value;
use std::{
    io::{BufRead, BufReader},
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
};
use string_lyrics::Song;

pub fn script(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join(name)
}
pub struct Fake {
    pub base: String,
    child: Child,
}
impl Fake {
    pub fn new(kind: &str, scenario: &str) -> Self {
        let child = Command::new("node")
            .creation_flags(0x08000000)
            .arg(script("service.cjs"))
            .args([kind, scenario])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut fake = Self {
            base: String::new(),
            child,
        };
        let mut port = String::new();
        BufReader::new(fake.child.stdout.take().unwrap())
            .read_line(&mut port)
            .unwrap();
        let port: u16 = port
            .trim()
            .parse()
            .expect("fake printed its listening port");
        fake.base = format!("http://127.0.0.1:{port}");
        fake
    }
    pub fn read(&self, path: &str) -> Value {
        let http: ureq::Agent = ureq::Agent::config_builder().proxy(None).build().into();
        http.get(format!("{}{path}", self.base))
            .call()
            .unwrap()
            .body_mut()
            .read_json()
            .unwrap()
    }
}
impl Drop for Fake {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
pub fn song(path: &Path) -> Song<'_> {
    Song {
        id: "track-🎵",
        path,
        title: "Glass Orbit",
        artist: "Test Signal",
        album: "Synthetic Weather",
        duration: 24.49,
    }
}
