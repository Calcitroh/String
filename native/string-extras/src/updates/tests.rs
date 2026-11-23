use super::*;
use std::{
    fs::File,
    net::{TcpListener, TcpStream},
    process::{Child, Stdio},
    thread,
};
use tempfile::tempdir;
use zip::{ZipWriter, write::SimpleFileOptions};

fn archive(file: File, version: &str, symlink: bool) -> File {
    let mut z = ZipWriter::new(file);
    let opts = SimpleFileOptions::default();
    for (name, bytes) in [
        ("app/setup/version.txt", version),
        ("app/String.exe", "fixture"),
        ("app/setup/install.ps1", "fixture"),
    ] {
        z.start_file(name, opts).unwrap();
        z.write_all(bytes.as_bytes()).unwrap();
    }
    if symlink {
        z.add_symlink("app/alias", "elsewhere", opts).unwrap();
    }
    let mut f = z.finish().unwrap();
    f.rewind().unwrap();
    f
}
struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn config(install: &Path, origin: &str, repo: &str, key: &str) -> Updater {
    Updater::for_test_feed(install, "1.1.0", &format!("{origin}/repos/{repo}"), key).unwrap()
}
#[test]
fn signed_release_feed_end_to_end() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    for (name, ver) in [("good.zip", "9.9.9"), ("liar.zip", "9.9.8")] {
        archive(File::create(root.join(name)).unwrap(), ver, false);
    }
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fake-github.js");
    let mut server = Server(
        Command::new("node")
            .arg(script)
            .arg(root)
            .arg(address.port().to_string())
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while TcpStream::connect(address).is_err() {
        assert!(
            server.0.try_wait().unwrap().is_none() && Instant::now() < deadline,
            "fixture failed to start"
        );
        thread::sleep(Duration::from_millis(20));
    }
    let key = fs::read_to_string(root.join("update-key.txt")).unwrap();
    let origin = format!("http://{address}");
    let install = root.join("String & % '");
    let data = root.join("profile");
    fs::create_dir_all(install.join("setup")).unwrap();
    let good = config(&install, &origin, "good", &key);
    assert!(!good.check().unwrap().installed);
    let marker = format!("\u{feff}{}", install.display())
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    fs::write(install.join("setup/installed.txt"), marker).unwrap();
    let check = good.check().unwrap();
    assert!(check.installed && check.newer);
    let latest = check.latest.unwrap();
    assert_eq!(latest.notes, "Fake notes for v9.9.9");
    assert_eq!(
        latest.size,
        fs::metadata(root.join("good.zip")).unwrap().len()
    );
    assert!(good.prepare(&data, "9.9.8", |_| {}).is_err());
    let mut events = vec![];
    let plan = good.prepare(&data, "9.9.9", |p| events.push(p)).unwrap();
    assert!(
        matches!(events.first(),Some(Progress::Receiving{got,total}) if got> &0 && *total==latest.size)
    );
    assert!(matches!(events.last(), Some(Progress::Verifying)));
    assert_eq!(plan.ready, data.join(r"updates\String-9.9.9"));
    assert_eq!(fs::read_to_string(&plan.script).unwrap(), "fixture");
    assert_eq!(plan.to, install);
    assert!(
        plan.command()
            .get_args()
            .all(|s| !s.to_string_lossy().contains(install.to_str().unwrap()))
    );
    for (repo, message) in [
        ("tampered", "signature"),
        ("liar", "version inside"),
        ("old", "up to date"),
        ("none", "no release"),
    ] {
        let u = config(&install, &origin, repo, &key);
        let check = u.check().unwrap();
        assert_eq!(check.newer, matches!(repo, "liar" | "tampered"));
        assert_eq!(check.latest.is_none(), repo == "none");
        let error = u.prepare(&data, "", |_| {}).unwrap_err().to_string();
        assert!(error.contains(message), "{repo}: {error}");
        assert!(plan.script.exists());
    }
    // Preparing the same good release twice also cleans up an older preparation.
    let old = data.join("updates/previous");
    fs::create_dir(&old).unwrap();
    good.prepare(&data, "", |_| {}).unwrap();
    assert!(!old.exists());
}
#[test]
fn archive_paths_and_links_are_refused() {
    for path in [
        "../x",
        "a/../b",
        "d:/x",
        "\\server\\share",
        "/x",
        "a:x",
        "con.txt",
        "a/aux .bin",
        "a//b",
        "x.",
    ] {
        assert!(unpack::entry_path(path).is_err(), "{path}");
    }
    assert!(unpack::entry_path("app/setup/install.ps1").is_ok());
    let tmp = tempdir().unwrap();
    let folder = unpack::UpdateFolder::open(tmp.path()).unwrap();
    let f = archive(tempfile::tempfile().unwrap(), "9.9.9", true);
    assert!(
        folder
            .unpack(f, "9.9.9".parse().unwrap())
            .unwrap_err()
            .to_string()
            .contains("link")
    );
}
#[test]
fn updates_directory_junction_is_refused() {
    let t = tempdir().unwrap();
    let dest = t.path().join("outside");
    fs::create_dir(&dest).unwrap();
    fs::write(dest.join("sentinel"), "kept").unwrap();
    let output=Command::new("powershell.exe").args(["-NoProfile","-NonInteractive","-Command",
        "New-Item -ItemType Junction -Path $env:LINK_PATH -Target $env:LINK_TARGET -ErrorAction Stop | Out-Null"])
        .env("LINK_PATH",t.path().join("updates")).env("LINK_TARGET",&dest).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(unpack::UpdateFolder::open(t.path()).is_err());
    assert_eq!(fs::read_dir(dest).unwrap().count(), 1);
}
fn response(body: &'static [u8], size: Option<usize>) -> Response<Body> {
    let mut r = Response::builder();
    if let Some(size) = size {
        r = r.header("content-length", size);
    }
    r.body(Body::builder().data(body)).unwrap()
}
#[test]
fn versions_limits_and_transport() {
    assert_eq!(
        "2.3".parse::<Version>().unwrap(),
        "2.3.0.42".parse().unwrap()
    );
    for bad in ["v2.3", "2", "2.3.4.5.6", "2.-1", "2.2147483648"] {
        assert!(bad.parse::<Version>().is_err());
    }
    assert!(small(response(b"12345", None), 4).is_err());
    assert!(small(response(b"x", Some(10)), 4).is_err());
    assert_eq!(small(response(b"1234", None), 4).unwrap(), b"1234");
    let t = tempdir().unwrap();
    let u = Updater::new(t.path(), "1.0").unwrap();
    assert!(matches!(
        u.get("http://127.0.0.1:1/no", METADATA),
        Err(ureq::Error::RequireHttpsOnly(_))
    ));
    assert!(VerifyingKey::from_public_key_der(&decode(KEY.as_bytes()).unwrap()).is_ok());
}

fn serve_once(send: impl FnOnce(TcpStream) + Send + 'static) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let task = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = vec![];
        let mut byte = [0];
        while !request.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
        }
        send(stream);
    });
    (format!("http://{address}"), task)
}

#[test]
fn drafts_and_prereleases_are_skipped_before_version_and_asset_checks() {
    let tmp = tempdir().unwrap();
    for flag in ["draft", "prerelease"] {
        let (origin, task) = serve_once(move |mut s| {
            let body = serde_json::json!({flag:true}).to_string();
            write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        let u = config(tmp.path(), &origin, "recent", KEY);
        assert!(u.latest().unwrap().is_none());
        task.join().unwrap();
    }
}

#[test]
fn body_deadlines_cover_a_silent_server_and_one_that_never_finishes() {
    let t = tempdir().unwrap();
    for trickle in [false, true] {
        let (done, wait) = std::sync::mpsc::channel::<()>();
        let (origin, task) = serve_once(move |mut s| {
            s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 20000\r\n\r\na")
                .unwrap();
            if trickle {
                for _ in 0..200 {
                    if wait.recv_timeout(Duration::from_millis(10)).is_ok()
                        || s.write_all(b"a").is_err()
                    {
                        break;
                    }
                }
            } else {
                let _ = wait.recv_timeout(Duration::from_secs(3));
            }
        });
        let mut u = config(t.path(), &origin, "recent", KEY);
        let (silence, overall) = if trickle { (1000, 180) } else { (150, 1500) };
        u.agent = agent(false, Duration::from_millis(silence));
        let res = u
            .get(&format!("{origin}/asset"), Duration::from_millis(overall))
            .unwrap();
        let err = small(res, 20000).unwrap_err();
        assert!(
            matches!(
                err.kind(),
                io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
            ),
            "{err:?}"
        );
        let _ = done.send(());
        task.join().unwrap();
    }
}
